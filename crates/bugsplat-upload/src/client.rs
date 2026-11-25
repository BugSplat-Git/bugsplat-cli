use std::sync::Arc;
use std::time::Duration;

use bugsplat_core::{Config, Error, Result};
use futures_util::future::BoxFuture;
use reqwest::header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, RETRY_AFTER};
use reqwest::multipart::Form;
use reqwest::{Body, Response};
use serde_json::Value;

use crate::retry::Retry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub database: String,
    pub application: String,
    pub version: String,
}

/// Returns a current bearer token. Called before every request, so a run that outlives
/// one access token keeps working when the provider refreshes it.
pub type TokenProvider = Arc<dyn Fn() -> BoxFuture<'static, Result<String>> + Send + Sync>;

enum Token {
    Fixed(String),
    Provider(TokenProvider),
}

/// BugSplat's symbol endpoints, with the request shapes `@bugsplat/js-api-client` uses.
pub struct Client {
    http: reqwest::Client,
    config: Config,
    token: Token,
}

// Not derived: the token must never be printed.
impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("api_url", &self.config.api_url.as_str())
            .field("access_token", &"***")
            .finish()
    }
}

// A transfer gets the usual request time plus time for its size at a floor of 16 KiB a
// second: generous for a slow connection, and still an end for one that has stalled.
fn transfer_timeout(size: u64) -> Duration {
    bugsplat_core::REQUEST_TIMEOUT + Duration::from_secs(size / (16 * 1024))
}

pub(crate) struct Failure {
    pub error: Error,
    pub retry: Retry,
}

pub(crate) struct FileFields<'a> {
    pub name: &'a str,
    pub module_name: &'a str,
    pub debug_id: &'a str,
    pub size: u64,
    pub last_modified: &'a str,
}

impl Client {
    pub fn new(http: reqwest::Client, config: &Config, access_token: impl Into<String>) -> Self {
        Self {
            http,
            config: config.clone(),
            token: Token::Fixed(access_token.into()),
        }
    }

    pub fn with_token_provider(
        http: reqwest::Client,
        config: &Config,
        provider: TokenProvider,
    ) -> Self {
        Self {
            http,
            config: config.clone(),
            token: Token::Provider(provider),
        }
    }

    async fn token(&self) -> Result<String> {
        match &self.token {
            Token::Fixed(token) => Ok(token.clone()),
            Token::Provider(provider) => provider().await,
        }
    }

    pub async fn delete_symbols(&self, target: &Target) -> Result<()> {
        let what = format!(
            "Error deleting symbols for {}-{}-{}",
            target.database, target.application, target.version
        );
        let token = self.token().await?;
        let response = self
            .http
            .delete(self.config.url("/api/v2/versions"))
            .query(&[
                ("database", &target.database),
                ("appName", &target.application),
                ("appVersion", &target.version),
            ])
            .bearer_auth(&token)
            .send()
            .await;
        self.read(response, &what, "delete symbols from", target)
            .await
            .map(|_| ())
            .map_err(|failure| failure.error)
    }

    pub(crate) async fn symbol_server_upload_url(
        &self,
        target: &Target,
        file: &FileFields<'_>,
    ) -> std::result::Result<String, Failure> {
        let what = format!("Error getting an upload URL for {}", file.name);
        let json = self
            .post(
                "/symsrv/uploadUrl",
                symbol_server_form(target, file),
                &what,
                target,
            )
            .await?;
        upload_url(&json, &what)
    }

    pub(crate) async fn symbol_server_upload_complete(
        &self,
        target: &Target,
        file: &FileFields<'_>,
    ) -> std::result::Result<(), Failure> {
        let what = format!("Error completing the upload of {}", file.name);
        self.post(
            "/symsrv/uploadComplete",
            symbol_server_form(target, file),
            &what,
            target,
        )
        .await
        .map(|_| ())
    }

    pub(crate) async fn versions_upload_url(
        &self,
        target: &Target,
        file: &FileFields<'_>,
    ) -> std::result::Result<String, Failure> {
        let what = format!("Error getting an upload URL for {}", file.name);
        let mut form = target_form(target)
            .text("size", file.size.to_string())
            .text("symFileName", file.name.to_string());
        if !file.debug_id.is_empty() {
            form = form.text("dbgId", file.debug_id.to_string());
        }
        form = form.text("lastModified", file.last_modified.to_string());
        if !file.module_name.is_empty() {
            form = form.text("moduleName", file.module_name.to_string());
        }
        let json = self.post("/api/v2/versions", form, &what, target).await?;
        upload_url(&json, &what)
    }

    pub(crate) async fn put(
        &self,
        url: &str,
        name: &str,
        body: Body,
        size: u64,
        gzip: bool,
    ) -> std::result::Result<(), Failure> {
        let mut request = self
            .http
            .put(url)
            .header(CONTENT_TYPE, "application/octet-stream")
            .header(CONTENT_LENGTH, size)
            .timeout(transfer_timeout(size))
            .body(body);
        if gzip {
            request = request.header(CONTENT_ENCODING, "gzip");
        }
        // The presigned URL carries a signature, so it stays out of every error message.
        let response = request.send().await.map_err(|error| Failure {
            error: Error::Http(error.without_url()),
            retry: Retry::Yes,
        })?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(Failure {
                error: Error::api(
                    status,
                    format!("Error uploading {name} to storage: status {status}"),
                ),
                retry: Retry::for_status(status, retry_after(&response)),
            });
        }
        Ok(())
    }

    async fn post(
        &self,
        path: &str,
        form: Form,
        what: &str,
        target: &Target,
    ) -> std::result::Result<Value, Failure> {
        let token = self.token().await.map_err(|error| Failure {
            error,
            retry: Retry::No,
        })?;
        let response = self
            .http
            .post(self.config.url(path))
            .bearer_auth(&token)
            .multipart(form)
            .send()
            .await;
        self.read(response, what, "upload symbols to", target).await
    }

    /// Turns a response into its JSON body, or into the failure it stands for: a status other
    /// than 200, or a 200 whose body says `"Status": "Failed"`.
    async fn read(
        &self,
        response: reqwest::Result<Response>,
        what: &str,
        action: &str,
        target: &Target,
    ) -> std::result::Result<Value, Failure> {
        let response = response.map_err(|error| Failure {
            error: Error::Http(error),
            retry: Retry::Yes,
        })?;
        let status = response.status().as_u16();
        let retry_after = retry_after(&response);
        let body = response.text().await.map_err(|error| Failure {
            error: Error::Http(error),
            retry: Retry::Yes,
        })?;

        if status != 200 {
            let detail = server_message(&body)
                .map(|message| format!(": {message}"))
                .unwrap_or_default();
            let message = match status {
                401 => format!(
                    "{what}: BugSplat did not accept the credentials (status 401). Sign in again with `bugsplat login`, or check the Client ID and Client Secret{detail}"
                ),
                403 => format!(
                    "Not authorized to {action} {}, check the database name and that your credentials have access to it{detail}",
                    target.database
                ),
                429 => format!("{what}: too many requests"),
                _ => format!("{what}: status {status}{detail}"),
            };
            return Err(Failure {
                error: Error::api(status, message),
                retry: Retry::for_status(status, retry_after),
            });
        }

        let json: Value = serde_json::from_str(&body).map_err(|_| Failure {
            error: Error::api(status, format!("{what}: the response was not JSON")),
            retry: Retry::Yes,
        })?;
        if json.get("Status").and_then(Value::as_str) == Some("Failed") {
            let message = json
                .get("Error")
                .and_then(Value::as_str)
                .unwrap_or("BugSplat did not say why")
                .to_string();
            let retry = if is_permanent_message(&message) {
                Retry::No
            } else {
                Retry::Yes
            };
            return Err(Failure {
                error: Error::api(status, format!("{what}: {message}")),
                retry,
            });
        }
        Ok(json)
    }
}

fn target_form(target: &Target) -> Form {
    Form::new()
        .text("database", target.database.clone())
        .text("appName", target.application.clone())
        .text("appVersion", target.version.clone())
}

fn symbol_server_form(target: &Target, file: &FileFields<'_>) -> Form {
    target_form(target)
        .text("size", file.size.to_string())
        .text("symFileName", file.name.to_string())
        .text("moduleName", file.module_name.to_string())
        .text("dbgId", file.debug_id.to_string())
        .text("lastModified", file.last_modified.to_string())
        .text("SendPdbsVersion", "spdbsv2")
}

fn upload_url(json: &Value, what: &str) -> std::result::Result<String, Failure> {
    json.get("url")
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
        .map(str::to_string)
        .ok_or_else(|| Failure {
            error: Error::api(200, format!("{what}: the response had no upload URL")),
            retry: Retry::No,
        })
}

/// `"Status": "Failed"` arrives with status 200 for both lasting and passing problems, so only
/// the answers known to be final stop the retries.
fn is_permanent_message(message: &str) -> bool {
    [
        "Symbol file max size",
        "Symbol table max size",
        "Unsupported symbol file type",
    ]
    .iter()
    .any(|permanent| message.contains(permanent))
}

fn retry_after(response: &Response) -> Option<Duration> {
    response
        .headers()
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
}

fn server_message(body: &str) -> Option<String> {
    let json: Value = serde_json::from_str(body).ok()?;
    let message = ["message", "Error", "error_description", "error"]
        .iter()
        .find_map(|key| json.get(key).and_then(Value::as_str))?
        .trim();
    (!message.is_empty()).then(|| message.chars().take(300).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transfer_gets_time_for_its_size() {
        let base = bugsplat_core::REQUEST_TIMEOUT;
        assert_eq!(transfer_timeout(0), base);
        assert_eq!(
            transfer_timeout(1024 * 1024 * 1024),
            base + Duration::from_secs(65536)
        );
    }

    #[test]
    fn debug_hides_the_token() {
        let client = Client::new(
            reqwest::Client::new(),
            &Config::new(None).unwrap(),
            "hunter2",
        );
        assert!(!format!("{client:?}").contains("hunter2"));
    }

    #[test]
    fn reads_the_server_message_from_json_bodies_only() {
        assert_eq!(
            server_message(r#"{"message":"Empty dbgId not supported"}"#).as_deref(),
            Some("Empty dbgId not supported")
        );
        assert_eq!(
            server_message(r#"{"Status":"Failed","Error":"nope"}"#).as_deref(),
            Some("nope")
        );
        assert_eq!(server_message("<html>502 Bad Gateway</html>"), None);
        assert_eq!(server_message(""), None);
    }

    #[test]
    fn size_limit_answers_are_final() {
        assert!(is_permanent_message(
            "Symbol file max size 2048.5 > 2048 mb exceeded fred, app-1.0"
        ));
        assert!(is_permanent_message("Symbol table max size exceeded"));
        assert!(!is_permanent_message("SQLSTATE[HY000]: gone away"));
    }
}

//! The requests of the OAuth flow MCP clients use: discovery, dynamic client
//! registration, PKCE, and the token endpoint.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bugsplat_core::{Config, Error, Result};
use reqwest::{Client, Response, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use url::Url;

const CLIENT_NAME: &str = "BugSplat CLI";

/// Registered without a port: the server accepts any port on a loopback redirect URI
/// (RFC 8252 section 7.3), and the listener's port is different on every sign-in.
pub(crate) const REGISTERED_REDIRECT_URI: &str = "http://127.0.0.1/callback";

#[derive(Deserialize)]
struct ProtectedResource {
    resource: Option<String>,
    #[serde(default)]
    authorization_servers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ServerMetadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    #[serde(default)]
    pub code_challenge_methods_supported: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Discovery {
    pub resource: String,
    pub server: ServerMetadata,
}

#[derive(Deserialize)]
pub(crate) struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub email: Option<String>,
}

pub(crate) enum Refresh {
    Tokens(TokenResponse),
    /// The server will never accept this refresh token again.
    Rejected {
        client_unknown: bool,
    },
}

#[derive(Deserialize, Default)]
struct ErrorBody {
    error: Option<String>,
    error_description: Option<String>,
    message: Option<String>,
}

impl ErrorBody {
    async fn read(response: Response) -> Self {
        response.json().await.unwrap_or_default()
    }

    fn describe(&self, status: StatusCode) -> String {
        self.error_description
            .clone()
            .or_else(|| self.message.clone())
            .or_else(|| self.error.clone())
            .unwrap_or_else(|| format!("status {}", status.as_u16()))
    }
}

fn same_url(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

async fn get_json<T: DeserializeOwned>(http: &Client, url: Url) -> Result<T> {
    let response = http.get(url.clone()).send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::api(
            status.as_u16(),
            format!("{url} answered status {}", status.as_u16()),
        ));
    }
    Ok(response.json().await?)
}

/// RFC 8414 section 3: the well-known segment goes between the host and the issuer's path.
fn metadata_url(issuer: &str) -> Result<Url> {
    let mut url = Url::parse(issuer).map_err(|e| {
        Error::api(
            None,
            format!("Invalid authorization server `{issuer}`: {e}"),
        )
    })?;
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(&format!("/.well-known/oauth-authorization-server{path}"));
    Ok(url)
}

pub(crate) async fn discover(http: &Client, config: &Config) -> Result<Discovery> {
    let resource = config.mcp_url().to_string();
    let resource_metadata = config.url("/.well-known/oauth-protected-resource/mcp");
    let protected: ProtectedResource = get_json(http, resource_metadata.clone()).await?;
    if let Some(described) = &protected.resource
        && !same_url(described, &resource)
    {
        return Err(Error::api(
            None,
            format!("{resource_metadata} describes {described}, not {resource}"),
        ));
    }
    let issuer = protected.authorization_servers.first().ok_or_else(|| {
        Error::api(
            None,
            format!("{resource_metadata} names no authorization server"),
        )
    })?;

    let server: ServerMetadata = get_json(http, metadata_url(issuer)?).await?;
    if !same_url(&server.issuer, issuer) {
        return Err(Error::api(
            None,
            format!(
                "The authorization server at {issuer} says it is {}",
                server.issuer
            ),
        ));
    }
    let methods = &server.code_challenge_methods_supported;
    if !methods.is_empty() && !methods.iter().any(|m| m == "S256") {
        return Err(Error::api(
            None,
            format!("The authorization server at {issuer} does not support PKCE with S256"),
        ));
    }
    Ok(Discovery { resource, server })
}

pub(crate) async fn register(http: &Client, registration_endpoint: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Registered {
        client_id: String,
    }

    let response = http
        .post(registration_endpoint)
        .json(&serde_json::json!({
            "client_name": CLIENT_NAME,
            "redirect_uris": [REGISTERED_REDIRECT_URI],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }))
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let detail = ErrorBody::read(response).await.describe(status);
        return Err(Error::api(
            status.as_u16(),
            format!("Could not register the CLI with BugSplat: {detail}"),
        ));
    }
    Ok(response.json::<Registered>().await?.client_id)
}

/// Whether the server still knows a cached client id. `http` must not follow redirects.
///
/// The server shows an error page for an unknown client instead of redirecting, which in
/// the browser would leave `login` waiting for a callback that never comes. Asking
/// without a `state` starts nothing: a known client is answered with an error redirect.
pub(crate) async fn client_is_registered(
    http: &Client,
    authorization_endpoint: &str,
    client_id: &str,
) -> Result<bool> {
    let response = http
        .get(authorization_endpoint)
        .query(&[
            ("response_type", "code"),
            ("client_id", client_id),
            ("redirect_uri", REGISTERED_REDIRECT_URI),
        ])
        .send()
        .await?;
    let status = response.status();
    if status.is_redirection() {
        Ok(true)
    } else if status == StatusCode::BAD_REQUEST {
        Ok(false)
    } else {
        Err(Error::api(
            status.as_u16(),
            format!(
                "{authorization_endpoint} answered status {}",
                status.as_u16()
            ),
        ))
    }
}

/// 32 random bytes, base64url: a PKCE verifier (RFC 7636 section 4.1) or a `state`.
pub(crate) fn random_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| Error::Other(format!("Could not get random bytes: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub(crate) fn code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub(crate) fn authorization_url(
    discovery: &Discovery,
    client_id: &str,
    redirect_uri: &str,
    code_challenge: &str,
    state: &str,
) -> Result<Url> {
    let endpoint = &discovery.server.authorization_endpoint;
    let mut url = Url::parse(endpoint).map_err(|e| {
        Error::api(
            None,
            format!("Invalid authorization endpoint `{endpoint}`: {e}"),
        )
    })?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .append_pair("resource", &discovery.resource);
    Ok(url)
}

pub(crate) async fn exchange_code(
    http: &Client,
    discovery: &Discovery,
    client_id: &str,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<TokenResponse> {
    let response = http
        .post(&discovery.server.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("code_verifier", code_verifier),
            ("resource", &discovery.resource),
        ])
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let detail = ErrorBody::read(response).await.describe(status);
        return Err(Error::api(
            status.as_u16(),
            format!("The sign-in could not be completed: {detail}"),
        ));
    }
    Ok(response.json().await?)
}

pub(crate) async fn refresh(
    http: &Client,
    token_endpoint: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<Refresh> {
    let response = http
        .post(token_endpoint)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ])
        .send()
        .await?;
    let status = response.status();
    if status.is_success() {
        return Ok(Refresh::Tokens(response.json().await?));
    }
    let body = ErrorBody::read(response).await;
    match body.error.as_deref() {
        Some("invalid_grant") => Ok(Refresh::Rejected {
            client_unknown: false,
        }),
        Some("invalid_client") => Ok(Refresh::Rejected {
            client_unknown: true,
        }),
        // Anything else (the identity provider is down, a refresh is already running) may
        // work on the next try, so the session stays.
        _ => Err(Error::api(
            status.as_u16(),
            format!("Could not refresh the session: {}", body.describe(status)),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_challenge_is_the_s256_of_the_verifier() {
        // RFC 7636 appendix B.
        assert_eq!(
            code_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn random_tokens_fit_the_verifier_grammar() {
        let a = random_token().unwrap();
        let b = random_token().unwrap();
        assert_eq!(a.len(), 43);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_ne!(a, b);
    }

    #[test]
    fn metadata_url_puts_the_well_known_segment_before_the_issuer_path() {
        assert_eq!(
            metadata_url("http://localhost:8085").unwrap().as_str(),
            "http://localhost:8085/.well-known/oauth-authorization-server"
        );
        assert_eq!(
            metadata_url("https://auth.example.com/tenant/")
                .unwrap()
                .as_str(),
            "https://auth.example.com/.well-known/oauth-authorization-server/tenant"
        );
    }
}

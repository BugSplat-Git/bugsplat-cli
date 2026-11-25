use std::path::PathBuf;

use url::Url;

use crate::{Error, Result};

pub const DEFAULT_API_URL: &str = "https://api.bugsplat.com";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
}

/// What every command needs to know, resolved once from flags and the environment.
#[derive(Clone)]
pub struct Config {
    /// Base URL of the BugSplat API, without a trailing slash.
    pub api_url: Url,
    pub database: Option<String>,
    /// OAuth client credentials for one database, used in CI instead of a signed-in session.
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub output: OutputFormat,
    pub quiet: bool,
    pub verbose: bool,
}

impl Config {
    pub fn new(api_url: Option<&str>) -> Result<Self> {
        let raw = api_url.unwrap_or(DEFAULT_API_URL).trim_end_matches('/');
        let api_url = Url::parse(raw).map_err(|e| Error::Usage(format!("Invalid API URL: {e}")))?;
        // The URL is printed by `whoami`, `--verbose` and these errors, so it must not carry
        // a secret, and no error here repeats the input.
        if !api_url.username().is_empty() || api_url.password().is_some() {
            return Err(Error::Usage(
                "Invalid API URL: it can't contain a user name or a password".to_string(),
            ));
        }
        if !matches!(api_url.scheme(), "http" | "https") || api_url.host_str().is_none() {
            return Err(Error::Usage(
                "Invalid API URL: expected http(s)://host".to_string(),
            ));
        }
        // Endpoints are appended to this URL, which would land inside a query or fragment.
        if api_url.query().is_some() || api_url.fragment().is_some() {
            return Err(Error::Usage(
                "Invalid API URL: it can't have a query or a fragment".to_string(),
            ));
        }
        Ok(Self {
            api_url,
            database: None,
            client_id: None,
            client_secret: None,
            output: OutputFormat::Text,
            quiet: false,
            verbose: false,
        })
    }

    /// `path` is joined to the API URL; it may start with `/`.
    pub fn url(&self, path: &str) -> Url {
        let base = self.api_url.as_str().trim_end_matches('/');
        let path = path.trim_start_matches('/');
        Url::parse(&format!("{base}/{path}")).expect("API URL joined with a path is a URL")
    }

    /// The hosted MCP endpoint, which is also the OAuth resource.
    pub fn mcp_url(&self) -> Url {
        self.url("mcp")
    }

    pub fn database(&self) -> Result<&str> {
        self.database.as_deref().ok_or_else(|| {
            Error::Usage("No database. Pass --database or set BUGSPLAT_DATABASE.".to_string())
        })
    }

    pub fn is_json(&self) -> bool {
        self.output == OutputFormat::Json
    }

    /// Where the CLI keeps its files: `$BUGSPLAT_CONFIG_DIR`, or `bugsplat` under the
    /// platform's config directory.
    pub fn config_dir() -> Result<PathBuf> {
        if let Some(dir) = std::env::var_os("BUGSPLAT_CONFIG_DIR").filter(|d| !d.is_empty()) {
            return Ok(PathBuf::from(dir));
        }
        dirs::config_dir()
            .map(|dir| dir.join("bugsplat"))
            .ok_or_else(|| Error::Other("Could not find a config directory".to_string()))
    }
}

// Not derived: the client secret must never be printed.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("api_url", &self.api_url.as_str())
            .field("database", &self.database)
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.as_ref().map(|_| "***"))
            .field("output", &self.output)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_api_url() {
        let config = Config::new(None).unwrap();
        assert_eq!(config.mcp_url().as_str(), "https://api.bugsplat.com/mcp");
    }

    #[test]
    fn joins_paths_under_a_custom_api_url() {
        let config = Config::new(Some("http://localhost:8085/")).unwrap();
        assert_eq!(
            config.url("/symsrv/uploadUrl").as_str(),
            "http://localhost:8085/symsrv/uploadUrl"
        );
    }

    #[test]
    fn rejects_a_bad_api_url() {
        assert!(matches!(
            Config::new(Some("not a url")),
            Err(Error::Usage(_))
        ));
        assert!(matches!(Config::new(Some("ftp://x")), Err(Error::Usage(_))));
        for url in ["http://localhost:8085?x=1", "http://localhost:8085/#mcp"] {
            assert!(
                matches!(Config::new(Some(url)), Err(Error::Usage(_))),
                "{url}"
            );
        }
        for url in [
            "https://user:hunter2@example.com",
            "https://user:hunter2@example.com/?x=1",
            "https://hunter2@example.com",
            "ftp://user:hunter2@example.com",
            "https://user:hunter2@exa mple.com",
        ] {
            let Err(Error::Usage(message)) = Config::new(Some(url)) else {
                panic!("{url} was accepted");
            };
            assert!(!message.contains("hunter2"), "{message}");
        }
    }

    #[test]
    fn debug_hides_the_client_secret() {
        let mut config = Config::new(None).unwrap();
        config.client_secret = Some("hunter2".into());
        assert!(!format!("{config:?}").contains("hunter2"));
    }

    #[test]
    fn missing_database_is_a_usage_error() {
        assert!(matches!(
            Config::new(None).unwrap().database(),
            Err(Error::Usage(_))
        ));
    }
}

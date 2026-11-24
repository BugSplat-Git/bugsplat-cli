//! Sign-in and access tokens. See https://github.com/BugSplat-Git/bugsplat-cli/issues/3
//!
//! Every other crate gets its bearer token from [`access_token`] and nothing else.

use bugsplat_core::{Config, Error, Result};

const ISSUE: &str = "https://github.com/BugSplat-Git/bugsplat-cli/issues/3";

/// Set to use a token directly, ahead of client credentials and a stored session.
/// For development and tests; `scripts/localstack-token.py` prints one for LocalStack.
pub const ACCESS_TOKEN_ENV: &str = "BUGSPLAT_ACCESS_TOKEN";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    Environment,
    ClientCredentials,
    Session,
}

#[derive(Clone)]
pub struct AccessToken {
    pub secret: String,
    pub source: TokenSource,
}

// Not derived: the token must never be printed.
impl std::fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessToken")
            .field("secret", &"***")
            .field("source", &self.source)
            .finish()
    }
}

/// Who is signed in, for `bugsplat whoami`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub email: Option<String>,
    pub api_url: String,
    pub source: TokenSource,
}

#[derive(Debug, Clone, Default)]
pub struct LoginOptions {
    /// Print the sign-in URL instead of opening a browser.
    pub no_browser: bool,
}

/// A bearer token for `config.api_url`, refreshed if needed.
/// `Error::NotSignedIn` when there is nothing to authenticate with.
pub async fn access_token(_config: &Config) -> Result<AccessToken> {
    if let Some(secret) = std::env::var(ACCESS_TOKEN_ENV)
        .ok()
        .filter(|t| !t.is_empty())
    {
        return Ok(AccessToken {
            secret,
            source: TokenSource::Environment,
        });
    }
    Err(Error::NotSignedIn)
}

pub async fn login(_config: &Config, _options: &LoginOptions) -> Result<Identity> {
    Err(Error::NotImplemented {
        command: "login",
        issue: ISSUE,
    })
}

pub async fn logout(_config: &Config) -> Result<()> {
    Err(Error::NotImplemented {
        command: "logout",
        issue: ISSUE,
    })
}

pub async fn whoami(_config: &Config) -> Result<Identity> {
    Err(Error::NotImplemented {
        command: "whoami",
        issue: ISSUE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_the_token() {
        let token = AccessToken {
            secret: "hunter2".into(),
            source: TokenSource::Environment,
        };
        assert!(!format!("{token:?}").contains("hunter2"));
    }
}

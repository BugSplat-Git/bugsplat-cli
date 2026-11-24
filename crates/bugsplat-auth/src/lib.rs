//! Sign-in and access tokens. See https://github.com/BugSplat-Git/bugsplat-cli/issues/3
//!
//! Every other crate gets its bearer token from [`access_token`] and nothing else.

mod client_credentials;
mod clients;
mod jwt;
mod login;
mod loopback;
mod oauth;
mod session;
mod store;

#[cfg(test)]
mod tests;

use bugsplat_core::{Config, Error, Result};
use reqwest::Client;
use serde::Serialize;
use url::Url;

use crate::client_credentials::ClientCredentials;
use crate::session::Sessions;

pub use crate::store::TOKEN_STORE_ENV;

/// Set to use a token directly, ahead of client credentials and a stored session.
/// For development and tests; `scripts/localstack-token.py` prints one for LocalStack.
pub const ACCESS_TOKEN_ENV: &str = "BUGSPLAT_ACCESS_TOKEN";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    /// `None` when the access token doesn't name an email.
    pub email: Option<String>,
    pub api_url: String,
    pub source: TokenSource,
    /// The Client ID, when `source` is client credentials.
    pub client_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct LoginOptions {
    /// Print the sign-in URL instead of opening a browser.
    pub no_browser: bool,
}

/// `Url` prints a host-only URL with a trailing slash; sessions are keyed without it.
fn api_url(config: &Config) -> String {
    config.api_url.as_str().trim_end_matches('/').to_string()
}

fn env_token() -> Option<String> {
    std::env::var(ACCESS_TOKEN_ENV)
        .ok()
        .filter(|token| !token.is_empty())
}

/// A bearer token for `config.api_url`, refreshed if needed.
/// `Error::NotSignedIn` when there is nothing to authenticate with.
///
/// `BUGSPLAT_ACCESS_TOKEN` comes first, then a Client ID and Client Secret, then the
/// session `bugsplat login` stored.
pub async fn access_token(config: &Config) -> Result<AccessToken> {
    let http = bugsplat_core::http_client()?;
    resolve(config, &http, env_token(), Sessions::from_env).await
}

// The stored session is opened last and only if needed: CI with a token or client
// credentials may have no config directory or keychain at all.
async fn resolve(
    config: &Config,
    http: &Client,
    env_token: Option<String>,
    sessions: impl FnOnce(&Config) -> Result<Sessions>,
) -> Result<AccessToken> {
    if let Some(secret) = env_token {
        return Ok(AccessToken {
            secret,
            source: TokenSource::Environment,
        });
    }
    if let Some(credentials) = ClientCredentials::from_config(config)? {
        return Ok(AccessToken {
            secret: credentials.token(http, config).await?,
            source: TokenSource::ClientCredentials,
        });
    }
    match sessions(config)?.current(http).await? {
        Some(session) => Ok(AccessToken {
            secret: session.access_token,
            source: TokenSource::Session,
        }),
        None => Err(Error::NotSignedIn),
    }
}

/// Signs in through the browser and stores the session for `config.api_url`.
pub async fn login(config: &Config, options: &LoginOptions) -> Result<Identity> {
    let http = bugsplat_core::http_client()?;
    let sessions = Sessions::from_env(config)?;
    let session = login::run(config, &http, &sessions, |url| present(url, options)).await?;
    Ok(Identity {
        email: session.email,
        api_url: api_url(config),
        source: TokenSource::Session,
        client_id: None,
    })
}

fn present(url: &Url, options: &LoginOptions) {
    if options.no_browser {
        eprintln!("Open this URL to sign in to BugSplat:\n\n  {url}\n");
        return;
    }
    eprintln!(
        "Opening your browser to sign in to BugSplat. If it doesn't open, visit:\n\n  {url}\n"
    );
    // A machine without a browser still has the URL printed above.
    let _ = open::that_detached(url.as_str());
}

/// Deletes the stored session for `config.api_url`, if there is one.
pub async fn logout(config: &Config) -> Result<()> {
    Sessions::from_env(config)?.delete().await
}

/// Who [`access_token`] would authenticate as, checked against the server where that
/// takes a request (client credentials, an expired session).
pub async fn whoami(config: &Config) -> Result<Identity> {
    let http = bugsplat_core::http_client()?;
    identify(config, &http, env_token(), Sessions::from_env).await
}

async fn identify(
    config: &Config,
    http: &Client,
    env_token: Option<String>,
    sessions: impl FnOnce(&Config) -> Result<Sessions>,
) -> Result<Identity> {
    let api_url = api_url(config);
    if let Some(token) = env_token {
        return Ok(Identity {
            email: jwt::email(&token),
            api_url,
            source: TokenSource::Environment,
            client_id: None,
        });
    }
    if let Some(credentials) = ClientCredentials::from_config(config)? {
        credentials.token(http, config).await?;
        return Ok(Identity {
            email: None,
            api_url,
            source: TokenSource::ClientCredentials,
            client_id: Some(credentials.id.to_string()),
        });
    }
    match sessions(config)?.current(http).await? {
        Some(session) => Ok(Identity {
            email: session.email,
            api_url,
            source: TokenSource::Session,
            client_id: None,
        }),
        None => Err(Error::NotSignedIn),
    }
}

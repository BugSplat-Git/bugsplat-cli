//! The stored session of a browser sign-in, and keeping its access token fresh.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bugsplat_core::{Config, Error, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::oauth::{self, Refresh, TokenResponse};
use crate::store::{self, StoreChoice, TOKEN_STORE_ENV, TokenStore};
use crate::{clients, jwt};

/// A token this close to expiring is refreshed, so it outlives the request it is for.
const REFRESH_MARGIN_SECS: u64 = 60;
const DEFAULT_LIFETIME_SECS: u64 = 3600;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Session {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Seconds since the Unix epoch.
    pub expires_at: u64,
    pub client_id: String,
    pub token_endpoint: String,
    pub email: Option<String>,
}

// Not derived: the tokens must never be printed.
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("access_token", &"***")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "***"))
            .field("expires_at", &self.expires_at)
            .field("client_id", &self.client_id)
            .field("token_endpoint", &self.token_endpoint)
            .field("email", &self.email)
            .finish()
    }
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

impl Session {
    pub(crate) fn new(tokens: TokenResponse, client_id: &str, token_endpoint: &str) -> Self {
        let email = tokens.email.or_else(|| jwt::email(&tokens.access_token));
        Self {
            expires_at: now() + tokens.expires_in.unwrap_or(DEFAULT_LIFETIME_SECS),
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            client_id: client_id.to_string(),
            token_endpoint: token_endpoint.to_string(),
            email,
        }
    }

    fn refreshed(self, tokens: TokenResponse) -> Self {
        let mut next = Self::new(tokens, &self.client_id, &self.token_endpoint);
        next.refresh_token = next.refresh_token.or(self.refresh_token);
        next.email = next.email.or(self.email);
        next
    }

    fn is_fresh(&self) -> bool {
        self.expires_at > now() + REFRESH_MARGIN_SECS
    }
}

async fn blocking<T, F>(work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| Error::Other(format!("Token storage failed: {e}")))?
}

#[derive(Clone)]
pub(crate) struct Sessions {
    pub(crate) api_url: String,
    pub(crate) config_dir: PathBuf,
    pub(crate) store: Arc<dyn TokenStore>,
}

impl Sessions {
    pub(crate) fn from_env(config: &Config) -> Result<Self> {
        let config_dir = Config::config_dir()?;
        let choice = StoreChoice::parse(
            std::env::var(TOKEN_STORE_ENV).ok().as_deref(),
            cfg!(debug_assertions),
        )?;
        Ok(Self {
            api_url: crate::api_url(config),
            store: store::open(&config_dir, choice, config.quiet),
            config_dir,
        })
    }

    // The store runs off the async threads: the keychain can block on a prompt, and the
    // Secret Service client on Linux blocks on its own I/O.
    pub(crate) async fn load(&self) -> Result<Option<Session>> {
        let (store, api_url) = (self.store.clone(), self.api_url.clone());
        blocking(move || store.load(&api_url)).await
    }

    pub(crate) async fn save(&self, session: Session) -> Result<()> {
        let (store, api_url) = (self.store.clone(), self.api_url.clone());
        blocking(move || store.save(&api_url, &session)).await
    }

    pub(crate) async fn delete(&self) -> Result<()> {
        let (store, api_url) = (self.store.clone(), self.api_url.clone());
        blocking(move || store.delete(&api_url)).await
    }

    /// Deletes the session while holding the refresh lock, so a refresh that is in flight
    /// can't store a new session after sign-out has returned.
    pub(crate) async fn sign_out(&self) -> Result<()> {
        let _lock = self.lock().await?;
        self.delete().await
    }

    pub(crate) fn client_id(&self) -> Option<String> {
        clients::load(&self.config_dir, &self.api_url)
    }

    pub(crate) fn remember_client(&self, client_id: &str) -> Result<()> {
        clients::save(&self.config_dir, &self.api_url, client_id)
    }

    /// Held until the returned file is dropped. Across processes too: two commands that
    /// both find an expired token must not both spend the refresh token.
    async fn lock(&self) -> Result<std::fs::File> {
        let dir = store::sessions_dir(&self.config_dir);
        let path = dir.join(format!("{}.lock", store::file_stem(&self.api_url)));
        blocking(move || {
            store::create_private_dir(&dir)?;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(path)?;
            // Through the trait: the inherent `File::lock` is newer than our minimum Rust.
            <std::fs::File as fs4::FileExt>::lock(&file)?;
            Ok(file)
        })
        .await
    }

    /// The stored session with an access token that is good for at least a minute, or
    /// `None` when there is no session or the server no longer accepts it.
    pub(crate) async fn current(&self, http: &Client) -> Result<Option<Session>> {
        match self.load().await? {
            Some(session) if session.is_fresh() => return Ok(Some(session)),
            Some(_) => {}
            None => return Ok(None),
        }

        let _lock = self.lock().await?;
        // Whoever held the lock before us has probably refreshed already, and their
        // refresh retired the refresh token we read above.
        let Some(session) = self.load().await? else {
            return Ok(None);
        };
        if session.is_fresh() {
            return Ok(Some(session));
        }
        let Some(refresh_token) = session.refresh_token.clone() else {
            self.delete().await?;
            return Ok(None);
        };
        let refresh = oauth::refresh(
            http,
            &session.token_endpoint,
            &session.client_id,
            &refresh_token,
        )
        .await?;
        match refresh {
            Refresh::Tokens(tokens) => {
                let session = session.refreshed(tokens);
                self.save(session.clone()).await?;
                Ok(Some(session))
            }
            Refresh::Rejected { client_unknown } => {
                self.delete().await?;
                if client_unknown {
                    clients::forget(&self.config_dir, &self.api_url)?;
                }
                Ok(None)
            }
        }
    }
}

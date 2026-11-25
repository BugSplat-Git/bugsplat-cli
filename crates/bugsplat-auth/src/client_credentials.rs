//! The client credentials grant CI uses: a database's Client ID and Client Secret,
//! exchanged at `/oauth2/authorize`, with no stored session.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use bugsplat_core::{Config, Error, Result};
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const EXPIRY_MARGIN: Duration = Duration::from_secs(60);

pub(crate) struct ClientCredentials<'a> {
    pub id: &'a str,
    secret: &'a str,
}

#[derive(Deserialize)]
struct Body {
    access_token: Option<String>,
    expires_in: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
    message: Option<String>,
}

type CacheKey = (String, String, [u8; 32]);

// A command asks for a token before each request, and the endpoint is rate limited.
static CACHE: LazyLock<Mutex<HashMap<CacheKey, (String, Instant)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// Upload workers that all find the token expired must not each ask for a new one.
static ACQUIRING: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

fn cached(key: &CacheKey) -> Option<String> {
    let cache = CACHE.lock().unwrap();
    let (token, expires) = cache.get(key)?;
    (Instant::now() < *expires).then(|| token.clone())
}

impl<'a> ClientCredentials<'a> {
    pub(crate) fn from_config(config: &'a Config) -> Result<Option<Self>> {
        let set = |value: &'a Option<String>| value.as_deref().filter(|v| !v.is_empty());
        match (set(&config.client_id), set(&config.client_secret)) {
            (Some(id), Some(secret)) => Ok(Some(Self { id, secret })),
            (None, None) => Ok(None),
            _ => Err(Error::Usage(
                "A Client ID and a Client Secret must be supplied together.".to_string(),
            )),
        }
    }

    pub(crate) async fn token(&self, http: &Client, config: &Config) -> Result<String> {
        let key = (
            crate::api_url(config),
            self.id.to_string(),
            Sha256::digest(self.secret.as_bytes()).into(),
        );
        if let Some(token) = cached(&key) {
            return Ok(token);
        }
        let _acquiring = ACQUIRING.lock().await;
        if let Some(token) = cached(&key) {
            return Ok(token);
        }

        let response = http
            .post(config.url("/oauth2/authorize"))
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.id),
                ("client_secret", self.secret),
            ])
            .send()
            .await?;
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok()?.parse::<u64>().ok())
                .map(|seconds| format!(", retry after {seconds} seconds"))
                .unwrap_or_default();
            return Err(Error::api(
                429,
                format!("Could not authenticate, too many requests{retry}"),
            ));
        }
        // A proxy error page or an empty body is not JSON; the status is all it tells us.
        let Ok(body) = response.json::<Body>().await else {
            return Err(Error::api(
                status.as_u16(),
                format!(
                    "Could not authenticate with the Client ID and Client Secret: status {}",
                    status.as_u16()
                ),
            ));
        };
        // A wrong secret answers `invalid_client`, an unknown Client ID a bare `message`;
        // the missing token is what they have in common.
        let Some(token) = body.access_token.filter(|token| !token.is_empty()) else {
            let detail = body
                .error_description
                .or(body.message)
                .or(body.error)
                .unwrap_or_else(|| format!("status {}", status.as_u16()));
            return Err(Error::api(
                status.as_u16(),
                format!("Could not authenticate with the Client ID and Client Secret: {detail}"),
            ));
        };

        if let Some(lifetime) = body.expires_in.map(Duration::from_secs)
            && lifetime > EXPIRY_MARGIN
        {
            let expires = Instant::now() + lifetime - EXPIRY_MARGIN;
            CACHE.lock().unwrap().insert(key, (token.clone(), expires));
        }
        Ok(token)
    }
}

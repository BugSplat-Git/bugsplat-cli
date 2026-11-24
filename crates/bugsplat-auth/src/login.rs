//! The browser sign-in: discovery, client registration, PKCE, loopback callback.

use std::time::Duration;

use bugsplat_core::{Config, Error, Result};
use reqwest::Client;
use url::Url;

use crate::loopback::Loopback;
use crate::oauth::{self, ServerMetadata};
use crate::session::{Session, Sessions};

/// As long as the server keeps a sign-in it has started.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(600);

/// `present` is given the sign-in URL once the callback listener is up.
pub(crate) async fn run(
    config: &Config,
    http: &Client,
    sessions: &Sessions,
    present: impl FnOnce(&Url),
) -> Result<Session> {
    let discovery = oauth::discover(http, config).await?;
    let client_id = client_id(http, sessions, &discovery.server).await?;

    let loopback = Loopback::bind().await?;
    let redirect_uri = loopback.redirect_uri();
    let verifier = oauth::random_token()?;
    let state = oauth::random_token()?;
    let url = oauth::authorization_url(
        &discovery,
        &client_id,
        &redirect_uri,
        &oauth::code_challenge(&verifier),
        &state,
    )?;
    present(&url);

    let code = tokio::time::timeout(SIGN_IN_TIMEOUT, loopback.wait_for_code(&state))
        .await
        .map_err(|_| {
            Error::Other(
                "Timed out waiting for the sign-in to finish. Run `bugsplat login` again."
                    .to_string(),
            )
        })??;
    let tokens = oauth::exchange_code(
        http,
        &discovery,
        &client_id,
        &code,
        &redirect_uri,
        &verifier,
    )
    .await?;
    let session = Session::new(tokens, &client_id, &discovery.server.token_endpoint);
    sessions.save(session.clone()).await?;
    Ok(session)
}

async fn client_id(http: &Client, sessions: &Sessions, server: &ServerMetadata) -> Result<String> {
    if let Some(cached) = sessions.client_id() {
        let no_redirects = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("bugsplat-cli/", env!("CARGO_PKG_VERSION")))
            .timeout(crate::REQUEST_TIMEOUT)
            .build()?;
        if oauth::client_is_registered(&no_redirects, &server.authorization_endpoint, &cached)
            .await?
        {
            return Ok(cached);
        }
    }
    let endpoint = server.registration_endpoint.as_deref().ok_or_else(|| {
        Error::api(
            None,
            format!(
                "The authorization server at {} does not register clients",
                server.issuer
            ),
        )
    })?;
    let client_id = oauth::register(http, endpoint).await?;
    sessions.remember_client(&client_id)?;
    Ok(client_id)
}

//! The flows end to end against a mock OAuth server. Sessions live in memory and the
//! config directory is temporary; nothing here can reach the keychain.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bugsplat_core::{Config, Error, Result};
use reqwest::Client;
use serde_json::json;
use tempfile::TempDir;
use url::Url;
use wiremock::matchers::{body_string_contains, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use crate::session::{Session, Sessions, now};
use crate::store::testing::MemoryStore;
use crate::{AccessToken, Identity, TokenSource, identify, jwt, login, oauth, resolve};

struct Fixture {
    server: MockServer,
    config: Config,
    http: Client,
    sessions: Sessions,
    _dir: TempDir,
}

/// What the stand-in browser saw in the sign-in URL.
#[derive(Clone, Default)]
struct Browser {
    url: Arc<Mutex<Option<Url>>>,
}

impl Browser {
    fn param(&self, name: &str) -> String {
        let url = self
            .url
            .lock()
            .unwrap()
            .clone()
            .expect("no sign-in URL yet");
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default()
    }

    /// Follows the sign-in URL the way a browser ends up doing: a request to the
    /// redirect URI carrying `query` and the `state`.
    fn finish_with(&self, query: &'static str) -> impl FnOnce(&Url) + use<> {
        let browser = self.clone();
        move |url: &Url| {
            *browser.url.lock().unwrap() = Some(url.clone());
            let callback = format!(
                "{}?{query}&state={}",
                browser.param("redirect_uri"),
                browser.param("state")
            );
            tokio::spawn(async move { Client::new().get(callback).send().await });
        }
    }
}

fn form(request: &Request) -> HashMap<String, String> {
    url::form_urlencoded::parse(&request.body)
        .into_owned()
        .collect()
}

impl Fixture {
    async fn new() -> Self {
        let server = MockServer::start().await;
        let config = Config::new(Some(&server.uri())).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let sessions = Sessions {
            api_url: crate::api_url(&config),
            config_dir: dir.path().to_path_buf(),
            store: Arc::new(MemoryStore::default()),
        };
        Self {
            server,
            config,
            http: Client::new(),
            sessions,
            _dir: dir,
        }
    }

    fn uri(&self, path: &str) -> String {
        format!("{}{path}", self.server.uri())
    }

    fn open_sessions(&self) -> impl FnOnce(&Config) -> Result<Sessions> + use<> {
        let sessions = self.sessions.clone();
        move |_| Ok(sessions)
    }

    async fn access_token(&self, env_token: Option<&str>) -> Result<AccessToken> {
        resolve(
            &self.config,
            &self.http,
            env_token.map(str::to_string),
            self.open_sessions(),
        )
        .await
    }

    async fn whoami(&self, env_token: Option<&str>) -> Result<Identity> {
        identify(
            &self.config,
            &self.http,
            env_token.map(str::to_string),
            self.open_sessions(),
        )
        .await
    }

    async fn mount_discovery(&self) {
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-protected-resource/mcp"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "resource": self.uri("/mcp"),
                "authorization_servers": [self.server.uri()],
            })))
            .mount(&self.server)
            .await;
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issuer": self.server.uri(),
                "authorization_endpoint": self.uri("/oauth/authorize"),
                "token_endpoint": self.uri("/oauth/token"),
                "registration_endpoint": self.uri("/oauth/register"),
                "code_challenge_methods_supported": ["S256"],
            })))
            .mount(&self.server)
            .await;
    }

    async fn mount_registration(&self, client_id: &str, times: u64) {
        Mock::given(method("POST"))
            .and(path("/oauth/register"))
            .and(body_string_contains(
                "\"token_endpoint_auth_method\":\"none\"",
            ))
            .and(body_string_contains("http://127.0.0.1/callback"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"client_id": client_id})))
            .expect(times)
            .mount(&self.server)
            .await;
    }

    /// The token endpoint for the code `the-code`, checking the request the way the real
    /// server does, PKCE included.
    async fn mount_code_exchange(&self, browser: &Browser, client_id: &'static str) {
        let browser = browser.clone();
        let resource = self.uri("/mcp");
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .and(body_string_contains("grant_type=authorization_code"))
            .respond_with(move |request: &Request| {
                let form = form(request);
                let field = |name: &str| form.get(name).cloned().unwrap_or_default();
                let valid = field("code") == "the-code"
                    && field("client_id") == client_id
                    && field("redirect_uri") == browser.param("redirect_uri")
                    && field("resource") == resource
                    && oauth::code_challenge(&field("code_verifier"))
                        == browser.param("code_challenge");
                if !valid {
                    return ResponseTemplate::new(400).set_body_json(json!({
                        "error": "invalid_grant",
                        "error_description": "PKCE verification failed.",
                    }));
                }
                ResponseTemplate::new(200).set_body_json(json!({
                    "token_type": "Bearer",
                    "access_token": "access-1",
                    "expires_in": 3600,
                    "scope": "",
                    "refresh_token": "refresh-1",
                }))
            })
            .mount(&self.server)
            .await;
    }

    async fn mount_client_probe(&self, client_id: &str, status: u16) {
        Mock::given(method("GET"))
            .and(path("/oauth/authorize"))
            .and(query_param("client_id", client_id))
            .respond_with(ResponseTemplate::new(status).insert_header(
                "Location",
                "http://127.0.0.1/callback?error=invalid_request",
            ))
            .expect(1)
            .mount(&self.server)
            .await;
    }

    async fn mount_client_credentials(&self, times: u64) {
        Mock::given(method("POST"))
            .and(path("/oauth2/authorize"))
            .and(body_string_contains("grant_type=client_credentials"))
            .and(body_string_contains(format!(
                "client_id={}&",
                self.ci_client()
            )))
            .and(body_string_contains("client_secret=ci-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "token_type": "Bearer",
                "expires_in": 86400,
                "access_token": "ci-token",
            })))
            .expect(times)
            .mount(&self.server)
            .await;
    }

    // A Client ID of its own: tokens from client credentials are cached for the life of
    // the process, and wiremock hands a finished test's address to the next one.
    fn with_client_credentials(mut self) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        self.config.client_id = Some(format!("ci-client-{id}"));
        self.config.client_secret = Some("ci-secret".into());
        self
    }

    fn ci_client(&self) -> String {
        self.config.client_id.clone().unwrap()
    }

    async fn store(&self, access_token: &str, expires_in: i64) {
        let session = Session {
            access_token: access_token.into(),
            refresh_token: Some("refresh-1".into()),
            expires_at: now().saturating_add_signed(expires_in),
            client_id: "client-1".into(),
            token_endpoint: self.uri("/oauth/token"),
            email: Some("fred@example.com".into()),
        };
        self.sessions.save(session).await.unwrap();
    }

    async fn stored(&self) -> Option<Session> {
        self.sessions.load().await.unwrap()
    }

    fn refresh_request(&self, refresh_token: &str) -> wiremock::MockBuilder {
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(body_string_contains(format!(
                "refresh_token={refresh_token}"
            )))
            .and(body_string_contains("client_id=client-1"))
    }
}

fn rotated_tokens() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "token_type": "Bearer",
        "access_token": "access-2",
        "expires_in": 3600,
        "scope": "",
        "refresh_token": "refresh-2",
    }))
}

#[tokio::test]
async fn discovery_reads_the_resource_and_server_documents() {
    let fixture = Fixture::new().await;
    fixture.mount_discovery().await;

    let discovery = oauth::discover(&fixture.http, &fixture.config)
        .await
        .unwrap();

    assert_eq!(discovery.resource, fixture.uri("/mcp"));
    assert_eq!(
        discovery.server.authorization_endpoint,
        fixture.uri("/oauth/authorize")
    );
    assert_eq!(discovery.server.token_endpoint, fixture.uri("/oauth/token"));
    assert_eq!(
        discovery.server.registration_endpoint,
        Some(fixture.uri("/oauth/register"))
    );
}

#[tokio::test]
async fn discovery_refuses_a_server_without_s256() {
    let fixture = Fixture::new().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-protected-resource/mcp"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"authorization_servers": [fixture.server.uri()]})),
        )
        .mount(&fixture.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": fixture.server.uri(),
            "authorization_endpoint": fixture.uri("/oauth/authorize"),
            "token_endpoint": fixture.uri("/oauth/token"),
            "code_challenge_methods_supported": ["plain"],
        })))
        .mount(&fixture.server)
        .await;

    let err = oauth::discover(&fixture.http, &fixture.config)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("S256"), "{err}");
}

#[tokio::test]
async fn discovery_refuses_metadata_for_another_resource() {
    let fixture = Fixture::new().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-protected-resource/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "resource": "https://elsewhere.example.com/mcp",
            "authorization_servers": [fixture.server.uri()],
        })))
        .mount(&fixture.server)
        .await;

    let err = oauth::discover(&fixture.http, &fixture.config)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("elsewhere.example.com"), "{err}");
}

#[tokio::test]
async fn login_registers_exchanges_the_code_with_pkce_and_stores_the_session() {
    let fixture = Fixture::new().await;
    let browser = Browser::default();
    fixture.mount_discovery().await;
    fixture.mount_registration("client-1", 1).await;
    fixture.mount_code_exchange(&browser, "client-1").await;

    let session = login::run(
        &fixture.config,
        &fixture.http,
        &fixture.sessions,
        browser.finish_with("code=the-code"),
    )
    .await
    .unwrap();

    assert_eq!(session.access_token, "access-1");
    assert_eq!(browser.param("response_type"), "code");
    assert_eq!(browser.param("client_id"), "client-1");
    assert_eq!(browser.param("code_challenge_method"), "S256");
    assert_eq!(browser.param("code_challenge").len(), 43);
    assert_eq!(browser.param("state").len(), 43);
    assert_eq!(browser.param("resource"), fixture.uri("/mcp"));
    let redirect_uri = browser.param("redirect_uri");
    assert!(
        redirect_uri.starts_with("http://127.0.0.1:"),
        "{redirect_uri}"
    );
    assert!(redirect_uri.ends_with("/callback"), "{redirect_uri}");

    let stored = fixture.stored().await.unwrap();
    assert_eq!(stored.access_token, "access-1");
    assert_eq!(stored.refresh_token.as_deref(), Some("refresh-1"));
    assert_eq!(stored.client_id, "client-1");
    assert_eq!(stored.token_endpoint, fixture.uri("/oauth/token"));
    assert!(stored.expires_at > now() + 3500);
    assert_eq!(fixture.sessions.client_id().as_deref(), Some("client-1"));

    let token = fixture.access_token(None).await.unwrap();
    assert_eq!(token.secret, "access-1");
    assert_eq!(token.source, TokenSource::Session);
}

#[tokio::test]
async fn login_reuses_the_registered_client() {
    let fixture = Fixture::new().await;
    let browser = Browser::default();
    fixture.mount_discovery().await;
    fixture.mount_registration("client-1", 1).await;
    fixture.mount_code_exchange(&browser, "client-1").await;
    fixture.mount_client_probe("client-1", 302).await;

    for _ in 0..2 {
        login::run(
            &fixture.config,
            &fixture.http,
            &fixture.sessions,
            browser.finish_with("code=the-code"),
        )
        .await
        .unwrap();
        assert_eq!(browser.param("client_id"), "client-1");
    }
}

#[tokio::test]
async fn login_registers_again_when_the_server_forgot_the_client() {
    let fixture = Fixture::new().await;
    let browser = Browser::default();
    fixture.sessions.remember_client("swept-client").unwrap();
    fixture.mount_discovery().await;
    fixture.mount_client_probe("swept-client", 400).await;
    fixture.mount_registration("client-2", 1).await;
    fixture.mount_code_exchange(&browser, "client-2").await;

    login::run(
        &fixture.config,
        &fixture.http,
        &fixture.sessions,
        browser.finish_with("code=the-code"),
    )
    .await
    .unwrap();

    assert_eq!(browser.param("client_id"), "client-2");
    assert_eq!(fixture.sessions.client_id().as_deref(), Some("client-2"));
}

#[tokio::test]
async fn login_fails_when_the_code_does_not_match_the_verifier() {
    let fixture = Fixture::new().await;
    let browser = Browser::default();
    fixture.mount_discovery().await;
    fixture.mount_registration("client-1", 1).await;
    fixture.mount_code_exchange(&browser, "client-1").await;

    let err = login::run(
        &fixture.config,
        &fixture.http,
        &fixture.sessions,
        browser.finish_with("code=another-code"),
    )
    .await
    .unwrap_err();

    assert!(
        err.to_string().contains("PKCE verification failed"),
        "{err}"
    );
    assert!(fixture.stored().await.is_none());
}

#[tokio::test]
async fn login_fails_when_the_user_declines() {
    let fixture = Fixture::new().await;
    let browser = Browser::default();
    fixture.mount_discovery().await;
    fixture.mount_registration("client-1", 1).await;

    let err = login::run(
        &fixture.config,
        &fixture.http,
        &fixture.sessions,
        browser
            .finish_with("error=access_denied&error_description=The+user+declined+the+connection."),
    )
    .await
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("The user declined the connection."),
        "{err}"
    );
    assert!(fixture.stored().await.is_none());
}

#[tokio::test]
async fn a_fresh_session_is_used_as_it_is() {
    let fixture = Fixture::new().await;
    fixture.store("access-1", 3600).await;

    let token = fixture.access_token(None).await.unwrap();

    assert_eq!(token.secret, "access-1");
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn an_expired_session_is_refreshed_and_the_rotated_token_stored() {
    let fixture = Fixture::new().await;
    fixture.store("access-1", -10).await;
    fixture
        .refresh_request("refresh-1")
        .respond_with(rotated_tokens())
        .expect(1)
        .mount(&fixture.server)
        .await;

    let token = fixture.access_token(None).await.unwrap();
    assert_eq!(token.secret, "access-2");
    assert_eq!(token.source, TokenSource::Session);

    let stored = fixture.stored().await.unwrap();
    assert_eq!(stored.access_token, "access-2");
    assert_eq!(stored.refresh_token.as_deref(), Some("refresh-2"));
    assert_eq!(stored.email.as_deref(), Some("fred@example.com"));
    assert!(stored.expires_at > now() + 3500);

    assert_eq!(fixture.access_token(None).await.unwrap().secret, "access-2");
}

#[tokio::test]
async fn a_session_about_to_expire_is_refreshed() {
    let fixture = Fixture::new().await;
    fixture.store("access-1", 30).await;
    fixture
        .refresh_request("refresh-1")
        .respond_with(rotated_tokens())
        .expect(1)
        .mount(&fixture.server)
        .await;

    assert_eq!(fixture.access_token(None).await.unwrap().secret, "access-2");
}

#[tokio::test]
async fn callers_at_the_same_time_refresh_once() {
    let fixture = Fixture::new().await;
    fixture.store("access-1", -10).await;
    fixture
        .refresh_request("refresh-1")
        .respond_with(rotated_tokens().set_delay(Duration::from_millis(200)))
        .expect(1)
        .mount(&fixture.server)
        .await;

    let (a, b, c) = tokio::join!(
        fixture.access_token(None),
        fixture.access_token(None),
        fixture.access_token(None)
    );

    for token in [a, b, c] {
        assert_eq!(token.unwrap().secret, "access-2");
    }
}

#[tokio::test]
async fn a_rejected_refresh_signs_out() {
    let fixture = Fixture::new().await;
    fixture.store("access-1", -10).await;
    fixture.sessions.remember_client("client-1").unwrap();
    fixture
        .refresh_request("refresh-1")
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": "Unknown or expired refresh token.",
        })))
        .expect(1)
        .mount(&fixture.server)
        .await;

    let err = fixture.access_token(None).await.unwrap_err();

    assert!(matches!(err, Error::NotSignedIn), "{err}");
    assert!(fixture.stored().await.is_none());
    assert_eq!(fixture.sessions.client_id().as_deref(), Some("client-1"));
    assert!(matches!(
        fixture.access_token(None).await,
        Err(Error::NotSignedIn)
    ));
}

#[tokio::test]
async fn a_refresh_for_a_client_the_server_forgot_drops_the_client_too() {
    let fixture = Fixture::new().await;
    fixture.store("access-1", -10).await;
    fixture.sessions.remember_client("client-1").unwrap();
    fixture
        .refresh_request("refresh-1")
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": "invalid_client",
            "error_description": "Unknown client_id.",
        })))
        .mount(&fixture.server)
        .await;

    let err = fixture.access_token(None).await.unwrap_err();

    assert!(matches!(err, Error::NotSignedIn), "{err}");
    assert!(fixture.stored().await.is_none());
    assert_eq!(fixture.sessions.client_id(), None);
}

#[tokio::test]
async fn a_refresh_the_server_could_not_answer_keeps_the_session() {
    let fixture = Fixture::new().await;
    fixture.store("access-1", -10).await;
    fixture
        .refresh_request("refresh-1")
        .respond_with(ResponseTemplate::new(502).set_body_json(json!({
            "error": "server_error",
            "error_description": "The identity provider did not answer.",
        })))
        .mount(&fixture.server)
        .await;

    let err = fixture.access_token(None).await.unwrap_err();

    assert!(
        matches!(
            err,
            Error::Api {
                status: Some(502),
                ..
            }
        ),
        "{err}"
    );
    assert!(err.to_string().contains("did not answer"), "{err}");
    let stored = fixture.stored().await.unwrap();
    assert_eq!(stored.refresh_token.as_deref(), Some("refresh-1"));
}

#[tokio::test]
async fn no_session_is_not_signed_in() {
    let fixture = Fixture::new().await;

    assert!(matches!(
        fixture.access_token(None).await,
        Err(Error::NotSignedIn)
    ));
    assert!(matches!(
        fixture.whoami(None).await,
        Err(Error::NotSignedIn)
    ));
}

#[tokio::test]
async fn client_credentials_are_exchanged_once_for_a_token() {
    let fixture = Fixture::new().await.with_client_credentials();
    fixture.mount_client_credentials(1).await;

    for _ in 0..2 {
        let token = fixture.access_token(None).await.unwrap();
        assert_eq!(token.secret, "ci-token");
        assert_eq!(token.source, TokenSource::ClientCredentials);
    }
    assert!(fixture.stored().await.is_none());
}

#[tokio::test]
async fn rejected_client_credentials_fail_without_naming_the_secret() {
    let fixture = Fixture::new().await.with_client_credentials();
    Mock::given(method("POST"))
        .and(path("/oauth2/authorize"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": "invalid_client",
            "error_description": "Client authentication failed",
        })))
        .mount(&fixture.server)
        .await;

    let err = fixture.access_token(None).await.unwrap_err();

    assert!(
        matches!(
            err,
            Error::Api {
                status: Some(401),
                ..
            }
        ),
        "{err}"
    );
    assert!(
        err.to_string().contains("Client authentication failed"),
        "{err}"
    );
    assert!(!format!("{err} {err:?}").contains("ci-secret"));
}

#[tokio::test]
async fn an_unknown_client_id_fails_with_the_servers_message() {
    let fixture = Fixture::new().await.with_client_credentials();
    Mock::given(method("POST"))
        .and(path("/oauth2/authorize"))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(json!({"message": "Unknown clientId"})),
        )
        .mount(&fixture.server)
        .await;

    let err = fixture.access_token(None).await.unwrap_err();
    assert!(err.to_string().contains("Unknown clientId"), "{err}");
}

#[tokio::test]
async fn rate_limited_client_credentials_say_when_to_retry() {
    let fixture = Fixture::new().await.with_client_credentials();
    Mock::given(method("POST"))
        .and(path("/oauth2/authorize"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "30"))
        .mount(&fixture.server)
        .await;

    let err = fixture.access_token(None).await.unwrap_err();
    assert!(
        matches!(
            err,
            Error::Api {
                status: Some(429),
                ..
            }
        ),
        "{err}"
    );
    assert!(err.to_string().contains("retry after 30 seconds"), "{err}");
}

#[tokio::test]
async fn a_client_id_without_a_secret_is_a_usage_error() {
    let mut fixture = Fixture::new().await;
    fixture.config.client_id = Some("ci-client".into());

    assert!(matches!(
        fixture.access_token(None).await,
        Err(Error::Usage(_))
    ));
}

#[tokio::test]
async fn the_environment_token_comes_before_client_credentials_and_a_session() {
    let fixture = Fixture::new().await.with_client_credentials();
    fixture.store("access-1", 3600).await;

    let token = fixture.access_token(Some("env-token")).await.unwrap();

    assert_eq!(token.secret, "env-token");
    assert_eq!(token.source, TokenSource::Environment);
    assert!(fixture.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn client_credentials_come_before_a_session() {
    let fixture = Fixture::new().await.with_client_credentials();
    fixture.mount_client_credentials(1).await;
    fixture.store("access-1", 3600).await;

    let token = fixture.access_token(None).await.unwrap();

    assert_eq!(token.secret, "ci-token");
    assert_eq!(token.source, TokenSource::ClientCredentials);
}

#[tokio::test]
async fn the_session_is_never_opened_when_another_source_answers() {
    let fixture = Fixture::new().await.with_client_credentials();
    fixture.mount_client_credentials(1).await;
    let unopenable = |_: &Config| -> Result<Sessions> { panic!("opened the session store") };

    let from_env = resolve(
        &fixture.config,
        &fixture.http,
        Some("env-token".into()),
        unopenable,
    )
    .await
    .unwrap();
    let from_credentials = resolve(&fixture.config, &fixture.http, None, unopenable)
        .await
        .unwrap();

    assert_eq!(from_env.source, TokenSource::Environment);
    assert_eq!(from_credentials.source, TokenSource::ClientCredentials);
}

#[tokio::test]
async fn whoami_describes_each_source() {
    let fixture = Fixture::new().await;
    fixture.store("access-1", 3600).await;
    let api_url = fixture.server.uri();

    assert_eq!(
        fixture.whoami(None).await.unwrap(),
        Identity {
            email: Some("fred@example.com".into()),
            api_url: api_url.clone(),
            source: TokenSource::Session,
            client_id: None,
        }
    );

    let env_token = jwt::fake_token(&json!({"email": "wilma@example.com"}));
    assert_eq!(
        fixture.whoami(Some(&env_token)).await.unwrap(),
        Identity {
            email: Some("wilma@example.com".into()),
            api_url: api_url.clone(),
            source: TokenSource::Environment,
            client_id: None,
        }
    );

    let fixture = fixture.with_client_credentials();
    fixture.mount_client_credentials(1).await;
    assert_eq!(
        fixture.whoami(None).await.unwrap(),
        Identity {
            email: None,
            api_url,
            source: TokenSource::ClientCredentials,
            client_id: Some(fixture.ci_client()),
        }
    );
}

#[tokio::test]
async fn whoami_refreshes_an_expired_session() {
    let fixture = Fixture::new().await;
    fixture.store("access-1", -10).await;
    fixture
        .refresh_request("refresh-1")
        .respond_with(rotated_tokens())
        .expect(1)
        .mount(&fixture.server)
        .await;

    assert_eq!(
        fixture.whoami(None).await.unwrap().source,
        TokenSource::Session
    );
    assert_eq!(fixture.stored().await.unwrap().access_token, "access-2");
}

#[tokio::test]
async fn the_email_comes_from_the_token_response_or_the_access_token() {
    let named = oauth::TokenResponse {
        access_token: jwt::fake_token(&json!({"username": "wilma@example.com"})),
        refresh_token: None,
        expires_in: Some(3600),
        email: Some("fred@example.com".into()),
    };
    assert_eq!(
        Session::new(named, "client", "endpoint").email.as_deref(),
        Some("fred@example.com")
    );

    let from_token = oauth::TokenResponse {
        access_token: jwt::fake_token(&json!({"username": "wilma@example.com"})),
        refresh_token: None,
        expires_in: Some(3600),
        email: None,
    };
    assert_eq!(
        Session::new(from_token, "client", "endpoint")
            .email
            .as_deref(),
        Some("wilma@example.com")
    );

    let unnamed = oauth::TokenResponse {
        access_token: jwt::fake_token(&json!({"username": "15e9142e-dd17"})),
        refresh_token: None,
        expires_in: Some(3600),
        email: None,
    };
    assert_eq!(Session::new(unnamed, "client", "endpoint").email, None);
}

#[tokio::test]
async fn logout_during_a_refresh_leaves_no_session() {
    let fixture = Fixture::new().await;
    fixture.store("access-1", -10).await;
    fixture
        .refresh_request("refresh-1")
        .respond_with(rotated_tokens().set_delay(Duration::from_millis(300)))
        .mount(&fixture.server)
        .await;

    let sign_out = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        fixture.sessions.sign_out().await
    };
    let (token, signed_out) = tokio::join!(fixture.access_token(None), sign_out);

    assert_eq!(token.unwrap().secret, "access-2");
    signed_out.unwrap();
    assert!(fixture.stored().await.is_none());
}

#[tokio::test]
async fn client_credentials_requested_at_the_same_time_are_exchanged_once() {
    let fixture = Fixture::new().await.with_client_credentials();
    Mock::given(method("POST"))
        .and(path("/oauth2/authorize"))
        .and(body_string_contains("grant_type=client_credentials"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "token_type": "Bearer",
                    "expires_in": 86400,
                    "access_token": "ci-token",
                }))
                .set_delay(Duration::from_millis(200)),
        )
        .expect(1)
        .mount(&fixture.server)
        .await;

    let (a, b, c) = tokio::join!(
        fixture.access_token(None),
        fixture.access_token(None),
        fixture.access_token(None)
    );

    for token in [a, b, c] {
        assert_eq!(token.unwrap().secret, "ci-token");
    }
}

#[tokio::test]
async fn logout_deletes_the_session() {
    let fixture = Fixture::new().await;
    fixture.store("access-1", 3600).await;

    fixture.sessions.delete().await.unwrap();
    fixture.sessions.delete().await.unwrap();

    assert!(fixture.stored().await.is_none());
    assert!(matches!(
        fixture.access_token(None).await,
        Err(Error::NotSignedIn)
    ));
}

#[test]
fn debug_hides_tokens() {
    let token = AccessToken {
        secret: "hunter2".into(),
        source: TokenSource::Environment,
    };
    assert!(!format!("{token:?}").contains("hunter2"));

    let session = Session {
        access_token: "hunter2".into(),
        refresh_token: Some("hunter3".into()),
        expires_at: 0,
        client_id: "client".into(),
        token_endpoint: "endpoint".into(),
        email: None,
    };
    let debug = format!("{session:?}");
    assert!(
        !debug.contains("hunter2") && !debug.contains("hunter3"),
        "{debug}"
    );
}

#[test]
fn identity_serializes_for_json_output() {
    let identity = Identity {
        email: Some("fred@example.com".into()),
        api_url: "https://api.bugsplat.com".into(),
        source: TokenSource::ClientCredentials,
        client_id: Some("ci-client".into()),
    };
    assert_eq!(
        serde_json::to_value(&identity).unwrap(),
        json!({
            "email": "fred@example.com",
            "apiUrl": "https://api.bugsplat.com",
            "source": "client-credentials",
            "clientId": "ci-client",
        })
    );
}

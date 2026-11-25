//! `login`, `logout` and `whoami` through the built binary.
//!
//! Every run keeps its session in a file under a temporary config directory
//! (`BUGSPLAT_TOKEN_STORE=file`, `BUGSPLAT_CONFIG_DIR`), so the keychain is never touched.
//!
//! The LocalStack check is ignored by default. It needs LocalStack running and an account:
//!
//! ```sh
//! BUGSPLAT_E2E_EMAIL=... BUGSPLAT_E2E_PASSWORD=... \
//!   cargo test -p bugsplat-cli --test auth -- --ignored
//! ```

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, Output, Stdio};

use reqwest::header::LOCATION;
use serde_json::{Value, json};
use tempfile::TempDir;
use url::Url;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// {"alg":"none"} . {"email":"fred@example.com"} . signature
const ACCESS_TOKEN: &str = "eyJhbGciOiJub25lIn0.eyJlbWFpbCI6ImZyZWRAZXhhbXBsZS5jb20ifQ.signature";
const REFRESHED_ACCESS_TOKEN: &str =
    "eyJhbGciOiJub25lIn0.eyJlbWFpbCI6ImZyZWRAZXhhbXBsZS5jb20ifQ.refreshed";

struct Sandbox {
    config_dir: TempDir,
    api_url: String,
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Run {
    fn from(output: Output) -> Self {
        Self {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).expect("stdout is JSON")
    }

    /// `secrets` are never shown, so a failure here can't leak one into the test log.
    fn assert_hides(&self, secrets: &[&str]) {
        for secret in secrets {
            assert!(
                !self.stdout.contains(secret) && !self.stderr.contains(secret),
                "the output contains a secret"
            );
        }
    }
}

impl Sandbox {
    fn new(api_url: &str) -> Self {
        Self {
            config_dir: tempfile::tempdir().unwrap(),
            api_url: api_url.trim_end_matches('/').to_string(),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bugsplat"));
        for name in [
            "BUGSPLAT_ACCESS_TOKEN",
            "BUGSPLAT_API_URL",
            "BUGSPLAT_HOST",
            "BUGSPLAT_DATABASE",
            "BUGSPLAT_CLIENT_ID",
            "BUGSPLAT_CLIENT_SECRET",
            "SYMBOL_UPLOAD_CLIENT_ID",
            "SYMBOL_UPLOAD_CLIENT_SECRET",
        ] {
            command.env_remove(name);
        }
        command
            .env("BUGSPLAT_TOKEN_STORE", "file")
            .env("BUGSPLAT_CONFIG_DIR", self.config_dir.path())
            .args(["--api-url", &self.api_url])
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Run {
        Run::from(self.command().args(args).output().unwrap())
    }

    fn session_file(&self) -> Option<PathBuf> {
        let sessions = std::fs::read_dir(self.config_dir.path().join("sessions")).ok()?;
        sessions
            .filter_map(|entry| Some(entry.ok()?.path()))
            .find(|path| path.extension().is_some_and(|ext| ext == "json"))
    }

    fn session(&self) -> Value {
        let file = self.session_file().expect("a session file");
        serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap()
    }

    fn expire_session(&self) {
        let file = self.session_file().expect("a session file");
        let mut session = self.session();
        session["expires_at"] = json!(0);
        std::fs::write(file, serde_json::to_vec(&session).unwrap()).unwrap();
    }

    fn start_login(&self) -> Login {
        let mut child = self
            .command()
            .args(["login", "--no-browser"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stderr = BufReader::new(child.stderr.take().unwrap());
        let mut printed = String::new();
        let url = loop {
            let mut line = String::new();
            if stderr.read_line(&mut line).unwrap() == 0 {
                let _ = child.kill();
                panic!("login ended without printing a sign-in URL: {printed}");
            }
            if line.trim().starts_with("http") {
                break Url::parse(line.trim()).unwrap();
            }
            printed.push_str(&line);
        };
        Login { child, stderr, url }
    }
}

struct Login {
    child: Child,
    // Kept open until the child exits: a closed stderr would fail its next message.
    stderr: BufReader<ChildStderr>,
    url: Url,
}

impl Login {
    fn param(&self, name: &str) -> String {
        self.url
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .unwrap_or_else(|| panic!("the sign-in URL has no {name}"))
    }

    fn finish(mut self) -> Run {
        let mut stderr = String::new();
        self.stderr.read_to_string(&mut stderr).unwrap();
        let mut run = Run::from(self.child.wait_with_output().unwrap());
        run.stderr = stderr;
        run
    }
}

#[cfg(unix)]
fn assert_private(file: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "{}", file.display());
}

#[cfg(not(unix))]
fn assert_private(_file: &Path) {}

async fn mount_oauth_server(server: &MockServer) {
    let uri = server.uri();
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-protected-resource/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "resource": format!("{uri}/mcp"),
            "authorization_servers": [uri],
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": uri,
            "authorization_endpoint": format!("{uri}/oauth/authorize"),
            "token_endpoint": format!("{uri}/oauth/token"),
            "registration_endpoint": format!("{uri}/oauth/register"),
            "code_challenge_methods_supported": ["S256"],
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/register"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"client_id": "client-1"})))
        .expect(1)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=the-code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token_type": "Bearer",
            "access_token": ACCESS_TOKEN,
            "expires_in": 3600,
            "scope": "",
            "refresh_token": "refresh-1",
        })))
        .expect(1)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=refresh-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token_type": "Bearer",
            "access_token": REFRESHED_ACCESS_TOKEN,
            "expires_in": 3600,
            "scope": "",
            "refresh_token": "refresh-2",
        })))
        .expect(1)
        .mount(server)
        .await;
}

#[test]
fn whoami_without_a_session_exits_3() {
    let sandbox = Sandbox::new("http://127.0.0.1:9");

    let run = sandbox.run(&["whoami"]);

    assert_eq!(run.code, Some(3), "{}", run.stderr);
    assert!(run.stderr.contains("Not signed in"), "{}", run.stderr);
    assert!(run.stdout.is_empty());
}

#[test]
fn logout_without_a_session_succeeds() {
    let sandbox = Sandbox::new("http://127.0.0.1:9");

    let run = sandbox.run(&["logout"]);

    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout, "Signed out of http://127.0.0.1:9\n");
}

#[test]
fn whoami_with_an_environment_token() {
    let sandbox = Sandbox::new("http://127.0.0.1:9");

    let run = Run::from(
        sandbox
            .command()
            .env("BUGSPLAT_ACCESS_TOKEN", ACCESS_TOKEN)
            .args(["whoami", "--json"])
            .output()
            .unwrap(),
    );

    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(
        run.json(),
        json!({
            "email": "fred@example.com",
            "apiUrl": "http://127.0.0.1:9",
            "source": "environment",
            "clientId": null,
        })
    );
    run.assert_hides(&[ACCESS_TOKEN]);
    assert!(sandbox.session_file().is_none());
}

#[test]
fn an_unknown_token_store_is_a_usage_error() {
    let sandbox = Sandbox::new("http://127.0.0.1:9");

    let run = Run::from(
        sandbox
            .command()
            .env("BUGSPLAT_TOKEN_STORE", "vault")
            .arg("whoami")
            .output()
            .unwrap(),
    );

    assert_eq!(run.code, Some(2), "{}", run.stderr);
    assert!(
        run.stderr.contains("BUGSPLAT_TOKEN_STORE"),
        "{}",
        run.stderr
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_with_client_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth2/authorize"))
        .and(body_string_contains("grant_type=client_credentials"))
        .and(body_string_contains("client_id=cli-test-client"))
        .and(body_string_contains("client_secret=cli-test-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token_type": "Bearer",
            "expires_in": 86400,
            "access_token": "cli-test-token",
        })))
        .expect(1)
        .mount(&server)
        .await;
    let sandbox = Sandbox::new(&server.uri());

    let run = Run::from(
        sandbox
            .command()
            .env("SYMBOL_UPLOAD_CLIENT_ID", "cli-test-client")
            .env("SYMBOL_UPLOAD_CLIENT_SECRET", "cli-test-secret")
            .arg("whoami")
            .output()
            .unwrap(),
    );

    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(
        run.stdout.contains("Client ID:  cli-test-client"),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains(&server.uri()), "{}", run.stdout);
    run.assert_hides(&["cli-test-secret", "cli-test-token"]);
    assert!(sandbox.session_file().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn login_whoami_refresh_and_logout_against_a_mock_server() {
    let server = MockServer::start().await;
    mount_oauth_server(&server).await;
    let sandbox = Sandbox::new(&server.uri());

    let login = sandbox.start_login();
    assert_eq!(login.param("client_id"), "client-1");
    assert_eq!(login.param("code_challenge_method"), "S256");
    assert_eq!(login.param("resource"), format!("{}/mcp", server.uri()));
    let callback = format!(
        "{}?code=the-code&state={}",
        login.param("redirect_uri"),
        login.param("state")
    );
    let page = reqwest::get(callback).await.unwrap();
    assert_eq!(page.status(), 200);
    let run = login.finish();
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(
        run.stdout,
        format!("Signed in to {} as fred@example.com\n", server.uri())
    );
    run.assert_hides(&[ACCESS_TOKEN, "refresh-1"]);
    assert_private(&sandbox.session_file().unwrap());

    let run = sandbox.run(&["whoami"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(
        run.stdout.contains("Email:      fred@example.com"),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains(&server.uri()), "{}", run.stdout);
    assert!(run.stdout.contains("browser sign-in"), "{}", run.stdout);
    run.assert_hides(&[ACCESS_TOKEN, "refresh-1"]);

    sandbox.expire_session();
    let run = sandbox.run(&["whoami", "--json"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.json()["source"], "session");
    run.assert_hides(&[REFRESHED_ACCESS_TOKEN, "refresh-2"]);
    let session = sandbox.session();
    assert_eq!(session["access_token"], REFRESHED_ACCESS_TOKEN);
    assert_eq!(session["refresh_token"], "refresh-2");
    assert_private(&sandbox.session_file().unwrap());

    let run = sandbox.run(&["logout"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(sandbox.session_file().is_none());
    assert_eq!(sandbox.run(&["whoami"]).code, Some(3));
}

fn hidden_fields(page: &str) -> Vec<(String, String)> {
    page.split("type=\"hidden\" name=\"")
        .skip(1)
        .filter_map(|rest| {
            let (name, rest) = rest.split_once('"')?;
            let (_, rest) = rest.split_once("value=\"")?;
            let (value, _) = rest.split_once('"')?;
            Some((name.to_string(), value.to_string()))
        })
        .collect()
}

enum Step {
    Page { url: Url, body: String },
    Callback(Url),
}

async fn settle(http: &reqwest::Client, mut response: reqwest::Response, callback: &str) -> Step {
    loop {
        if !response.status().is_redirection() {
            let url = response.url().clone();
            let body = response.text().await.unwrap();
            return Step::Page { url, body };
        }
        let location = response.headers()[LOCATION].to_str().unwrap();
        let next = response.url().join(location).unwrap();
        if next.as_str().starts_with(callback) {
            return Step::Callback(next);
        }
        response = http.get(next).send().await.unwrap();
    }
}

/// What a person does in the browser on LocalStack, whose sign-in page is a plain form:
/// enter the email and password, then allow the connection.
async fn sign_in_over_http(sign_in_url: &Url, callback: &str, email: &str, password: &str) -> Url {
    let http = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let response = http.get(sign_in_url.clone()).send().await.unwrap();
    let Step::Page { url, body } = settle(&http, response, callback).await else {
        panic!("the sign-in redirected back before asking for a password");
    };

    let mut form = hidden_fields(&body);
    form.push(("email".into(), email.into()));
    form.push(("password".into(), password.into()));
    let response = http.post(url).form(&form).send().await.unwrap();
    let (url, body) = match settle(&http, response, callback).await {
        Step::Callback(url) => return url,
        Step::Page { url, body } => (url, body),
    };
    assert!(
        body.contains("name=\"decision\""),
        "the sign-in failed; check the email and password"
    );

    let mut form = hidden_fields(&body);
    form.push(("decision".into(), "allow".into()));
    let response = http.post(url).form(&form).send().await.unwrap();
    match settle(&http, response, callback).await {
        Step::Callback(url) => url,
        Step::Page { .. } => panic!("allowing the connection did not redirect back"),
    }
}

async fn mcp_accepts(api_url: &str, access_token: &str) -> bool {
    let response = reqwest::Client::new()
        .post(format!("{api_url}/mcp"))
        .bearer_auth(access_token)
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "bugsplat-cli-tests", "version": "0"},
            },
        }))
        .send()
        .await
        .unwrap();
    response.status() == 200
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs LocalStack and BUGSPLAT_E2E_EMAIL / BUGSPLAT_E2E_PASSWORD"]
async fn login_whoami_refresh_and_logout_against_localstack() {
    let email = std::env::var("BUGSPLAT_E2E_EMAIL").expect("BUGSPLAT_E2E_EMAIL");
    let password = std::env::var("BUGSPLAT_E2E_PASSWORD").expect("BUGSPLAT_E2E_PASSWORD");
    let api_url = std::env::var("BUGSPLAT_E2E_API_URL")
        .unwrap_or_else(|_| "http://localhost:8085".to_string());
    let host = Url::parse(&api_url).unwrap();
    assert!(
        matches!(host.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
        "refusing to post a password to a host that is not this machine"
    );
    let sandbox = Sandbox::new(&api_url);
    let secrets = |sandbox: &Sandbox| {
        let session = sandbox.session();
        [
            session["access_token"].as_str().unwrap().to_string(),
            session["refresh_token"].as_str().unwrap().to_string(),
        ]
    };

    let login = sandbox.start_login();
    let callback =
        sign_in_over_http(&login.url, &login.param("redirect_uri"), &email, &password).await;
    assert_eq!(reqwest::get(callback).await.unwrap().status(), 200);
    let run = login.finish();
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(
        run.stdout
            .starts_with(&format!("Signed in to {}", sandbox.api_url))
    );
    assert_private(&sandbox.session_file().unwrap());
    let [first_access, first_refresh] = secrets(&sandbox);
    run.assert_hides(&[&first_access, &first_refresh, &password]);
    assert!(mcp_accepts(&sandbox.api_url, &first_access).await);

    let run = sandbox.run(&["whoami", "--json"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let identity = run.json();
    assert_eq!(identity["apiUrl"], sandbox.api_url);
    assert_eq!(identity["source"], "session");
    // LocalStack's access tokens name no email; a server that does must name this one.
    assert!(identity["email"].is_null() || identity["email"] == email);
    run.assert_hides(&[&first_access, &first_refresh]);

    sandbox.expire_session();
    let run = sandbox.run(&["whoami"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let [second_access, second_refresh] = secrets(&sandbox);
    run.assert_hides(&[&second_access, &second_refresh]);
    assert!(
        first_refresh != second_refresh,
        "the refresh token did not rotate"
    );
    assert!(sandbox.session()["expires_at"].as_u64().unwrap() > 0);
    assert!(mcp_accepts(&sandbox.api_url, &second_access).await);

    // Two commands that both find an expired token: one refreshes, the other reuses it.
    sandbox.expire_session();
    let racers: Vec<Child> = (0..2)
        .map(|_| {
            let mut command = sandbox.command();
            command
                .arg("whoami")
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            command.spawn().unwrap()
        })
        .collect();
    for mut racer in racers {
        assert!(racer.wait().unwrap().success());
    }
    let [third_access, _] = secrets(&sandbox);
    assert!(mcp_accepts(&sandbox.api_url, &third_access).await);

    // A second sign-in reuses the client the first one registered.
    let clients = sandbox.config_dir.path().join("clients.json");
    let registered = std::fs::read(&clients).unwrap();
    let login = sandbox.start_login();
    let callback =
        sign_in_over_http(&login.url, &login.param("redirect_uri"), &email, &password).await;
    assert_eq!(reqwest::get(callback).await.unwrap().status(), 200);
    assert_eq!(login.finish().code, Some(0));
    assert_eq!(std::fs::read(&clients).unwrap(), registered);

    let run = sandbox.run(&["logout"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(sandbox.session_file().is_none());
    let run = sandbox.run(&["whoami"]);
    assert_eq!(run.code, Some(3), "{}", run.stderr);
}

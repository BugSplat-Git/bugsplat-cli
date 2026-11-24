//! `bugsplat mcp` from the outside: a JSON-RPC client on the binary's stdin and stdout.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "mcp-test-token";
const PDB_ID: &str = "E546B55B6D214E86871B40AC35CD0D461";
/// Nothing listens here, so a request the tests didn't expect fails instead of leaving the
/// machine.
const NO_API: &str = "http://127.0.0.1:9";
const TIMEOUT: Duration = Duration::from_secs(60);

fn fixture(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bugsplat-debugid/tests/fixtures")
        .join(relative)
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// A running `bugsplat mcp` and everything it has written.
struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Receiver<String>,
    stderr: Option<JoinHandle<String>>,
    lines: Vec<String>,
    next_id: u64,
    _config_dir: TempDir,
}

struct Finished {
    status: ExitStatus,
    stdout: Vec<String>,
    stderr: String,
}

impl Server {
    fn start(api_url: &str, configure: impl FnOnce(&mut Command)) -> Self {
        let config_dir = tempfile::tempdir().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_bugsplat"));
        for name in [
            "BUGSPLAT_ACCESS_TOKEN",
            "BUGSPLAT_HOST",
            "BUGSPLAT_DATABASE",
            "BUGSPLAT_CLIENT_ID",
            "BUGSPLAT_CLIENT_SECRET",
            "SYMBOL_UPLOAD_CLIENT_ID",
            "SYMBOL_UPLOAD_CLIENT_SECRET",
        ] {
            command.env_remove(name);
        }
        // Keep the developer's real session and keychain out of test runs.
        command
            .env("BUGSPLAT_TOKEN_STORE", "file")
            .env("BUGSPLAT_CONFIG_DIR", config_dir.path())
            .env("BUGSPLAT_API_URL", api_url)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure(&mut command);
        let mut child = command.spawn().unwrap();

        let (sender, stdout) = mpsc::channel();
        let out = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut err = child.stderr.take().unwrap();
        let stderr = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = err.read_to_string(&mut text);
            text
        });

        Self {
            stdin: child.stdin.take(),
            child,
            stdout,
            stderr: Some(stderr),
            lines: Vec::new(),
            next_id: 0,
            _config_dir: config_dir,
        }
    }

    /// A server that has been through the handshake.
    fn ready(api_url: &str, configure: impl FnOnce(&mut Command)) -> Self {
        let mut server = Self::start(api_url, configure);
        server.initialize();
        server
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    /// Sends a request and returns the whole response message for it.
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let mut message = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if !params.is_null() {
            message["params"] = params;
        }
        self.send(&message);
        loop {
            let line = self
                .stdout
                .recv_timeout(TIMEOUT)
                .unwrap_or_else(|_| panic!("no response to {method} within {TIMEOUT:?}"));
            let response: Value = serde_json::from_str(&line)
                .unwrap_or_else(|err| panic!("stdout line is not JSON ({err}): {line}"));
            self.lines.push(line);
            if response["id"] == id {
                return response;
            }
        }
    }

    fn initialize(&mut self) -> Value {
        let response = self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "bugsplat-cli-tests", "version": "0" },
            }),
        );
        self.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        response["result"].clone()
    }

    /// The result of a `tools/call`, which is an error result when the tool failed.
    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let response = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    /// Closes stdin, which is how a client stops a stdio server, and collects the rest.
    fn finish(mut self) -> Finished {
        drop(self.stdin.take());
        let deadline = Instant::now() + TIMEOUT;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "bugsplat mcp did not exit after stdin closed"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        let mut stdout = std::mem::take(&mut self.lines);
        stdout.extend(self.stdout.iter());
        let stderr = self.stderr.take().unwrap().join().unwrap();
        Finished {
            status,
            stdout,
            stderr,
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // A failed assertion must not leave the server running.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Finished {
    /// stdout is the protocol channel: every line must be one JSON-RPC message.
    fn assert_stdout_is_only_protocol(&self) {
        assert!(!self.stdout.is_empty());
        for line in &self.stdout {
            let message: Value = serde_json::from_str(line)
                .unwrap_or_else(|err| panic!("stdout line is not JSON ({err}): {line}"));
            assert_eq!(message["jsonrpc"], "2.0", "{line}");
            let is_response = message.get("id").is_some()
                && (message.get("result").is_some() || message.get("error").is_some());
            let is_notification = message["method"].is_string();
            assert!(is_response || is_notification, "{line}");
        }
    }
}

/// What a successful tool result carries, checked to be the same JSON in both places.
fn structured(result: &Value) -> &Value {
    assert_eq!(result["isError"], false, "{result}");
    let from_text: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(from_text, result["structuredContent"], "{result}");
    &result["structuredContent"]
}

fn error_text(result: &Value) -> &str {
    assert_eq!(result["isError"], true, "{result}");
    result["content"][0]["text"].as_str().unwrap()
}

async fn upload_server() -> MockServer {
    let server = MockServer::start().await;
    let url = |key: &str| {
        ResponseTemplate::new(200).set_body_json(json!({
            "Status": "Success",
            "url": format!("{}/bucket/{key}", server.uri()),
        }))
    };
    Mock::given(method("POST"))
        .and(path("/api/v2/versions"))
        .respond_with(url("versions"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/symsrv/uploadUrl"))
        .respond_with(url("symsrv"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/symsrv/uploadComplete"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "Status": "Success" })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex("^/bucket/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    server
}

async fn sent(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect()
}

/// A build folder with a PDB, a source map and a file that is neither.
fn build() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::copy(
        fixture("pdb-guid/bugsplat.pdb"),
        dir.path().join("bugsplat.pdb"),
    )
    .unwrap();
    fs::write(dir.path().join("main.js.map"), br#"{"version":3}"#).unwrap();
    fs::write(dir.path().join("notes.txt"), b"not a symbol file").unwrap();
    dir
}

/// The server blocks on pipes, so a test that also runs a mock server drives it off the
/// async runtime.
async fn off_runtime<T: Send + 'static>(session: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(session).await.unwrap()
}

#[test]
fn the_handshake_names_the_server_and_says_where_crash_data_comes_from() {
    let mut server = Server::start(NO_API, |_| {});

    let result = server.initialize();

    assert_eq!(result["protocolVersion"], "2025-06-18");
    assert!(result["capabilities"]["tools"].is_object(), "{result}");
    assert_eq!(result["serverInfo"]["name"], "bugsplat-cli");
    assert_eq!(result["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    let instructions = result["instructions"].as_str().unwrap();
    for tool in ["symbols-info", "symbols-find", "symbols-upload"] {
        assert!(instructions.contains(tool), "{instructions}");
    }
    assert!(
        instructions.contains("Crash data does not come from this server"),
        "{instructions}"
    );
    assert!(
        instructions.contains(&format!("{NO_API}/mcp")),
        "{instructions}"
    );

    let finished = server.finish();
    assert!(finished.status.success(), "{}", finished.stderr);
    assert_eq!(finished.stderr, "");
    finished.assert_stdout_is_only_protocol();
}

#[test]
fn every_tool_has_a_title_a_described_schema_and_all_three_hints() {
    let mut server = Server::ready(NO_API, |_| {});

    let response = server.request("tools/list", Value::Null);

    let tools = response["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["symbols-info", "symbols-find", "symbols-upload"]);
    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        let title = tool["title"].as_str().unwrap();
        assert!(!title.is_empty(), "{name}");
        assert!(tool["description"].as_str().unwrap().len() > 100, "{name}");

        let hints = &tool["annotations"];
        assert_eq!(hints["title"], title, "{name}");
        assert_eq!(hints["readOnlyHint"], name != "symbols-upload", "{name}");
        assert_eq!(hints["destructiveHint"], false, "{name}");
        assert_eq!(hints["openWorldHint"], false, "{name}");

        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object", "{name}");
        let properties = schema["properties"].as_object().unwrap();
        assert!(!properties.is_empty(), "{name}");
        for (property, definition) in properties {
            assert!(
                definition["description"]
                    .as_str()
                    .is_some_and(|d| !d.is_empty()),
                "{name}.{property} has no description"
            );
        }
    }
    let upload = &tools[2]["inputSchema"];
    assert!(upload["properties"]["dryRun"].is_object());
    assert!(upload["properties"]["database"].is_object());

    server.finish().assert_stdout_is_only_protocol();
}

#[test]
fn symbols_info_reads_debug_ids_from_the_fixtures() {
    let mut server = Server::ready(NO_API, |_| {});
    let pdb = fixture("pdb-guid/bugsplat.pdb");
    let elf = fixture("elfy/hello.elf");
    let dsym = fixture("macho-uuid/bugsplat.app.dSYM");

    let result = server.call(
        "symbols-info",
        json!({ "paths": [text(&pdb), text(&elf), text(&dsym)] }),
    );

    let info = structured(&result);
    assert_eq!(info["failures"], json!([]));
    assert_eq!(info["truncated"], false);
    let objects = info["objects"].as_array().unwrap();
    assert_eq!(objects[0]["path"], text(&pdb));
    assert_eq!(objects[0]["moduleName"], "bugsplat.pdb");
    assert_eq!(objects[0]["debugId"], PDB_ID);
    assert_eq!(objects[0]["kind"], "pdb");
    assert_eq!(objects[1]["moduleName"], "hello.elf");
    assert_eq!(
        objects[1]["debugId"],
        "85fe216fc7dd441f04c237310a56081fbf23c082"
    );
    assert_eq!(objects[1]["kind"], "elf");
    assert!(objects.len() > 2);
    for object in &objects[2..] {
        assert_eq!(object["kind"], "mach-o");
        assert!(object["arch"].is_string(), "{object}");
    }

    server.finish().assert_stdout_is_only_protocol();
}

#[test]
fn symbols_find_searches_the_fixtures_by_debug_id_and_by_name() {
    let mut server = Server::ready(NO_API, |_| {});
    let fixtures = fixture("");

    let by_id = server.call(
        "symbols-find",
        json!({
            "paths": [text(&fixtures)],
            "debugIds": ["e546b55b-6d21-4e86-871b-40ac35cd0d46-1", "ABCDEF0123"],
        }),
    );
    let found = structured(&by_id);
    assert_eq!(found["totalMatches"], 1);
    assert_eq!(found["truncated"], false);
    assert_eq!(found["matches"][0]["moduleName"], "bugsplat.pdb");
    assert_eq!(found["matches"][0]["debugId"], PDB_ID);
    assert_eq!(
        Path::new(found["matches"][0]["path"].as_str().unwrap()).file_name(),
        Some("bugsplat.pdb".as_ref())
    );
    assert_eq!(found["missingIds"], json!(["ABCDEF0123"]));

    let by_name = server.call(
        "symbols-find",
        json!({ "paths": [text(&fixtures)], "names": ["HELLO.elf"] }),
    );
    let found = structured(&by_name);
    assert_eq!(found["totalMatches"], 1);
    assert_eq!(found["matches"][0]["kind"], "elf");
    assert_eq!(found["matches"][0]["matchedNames"], json!(["HELLO.elf"]));

    let everything = server.call(
        "symbols-find",
        json!({ "paths": [text(&fixtures)], "limit": 3 }),
    );
    let found = structured(&everything);
    assert_eq!(found["matches"].as_array().unwrap().len(), 3);
    assert_eq!(found["truncated"], true);
    assert!(found["totalMatches"].as_u64().unwrap() > 3);
    let note = found["note"].as_str().unwrap();
    assert!(note.contains("Showing the first 3 of"), "{note}");
    assert!(note.contains("Narrow the search"), "{note}");

    server.finish().assert_stdout_is_only_protocol();
}

#[test]
fn bad_paths_and_arguments_are_tool_errors_and_the_server_keeps_running() {
    let mut server = Server::ready(NO_API, |_| {});
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope");
    let file = dir.path().join("notes.txt");
    fs::write(&file, "plain text").unwrap();

    let result = server.call("symbols-find", json!({ "paths": [text(&missing)] }));
    assert!(error_text(&result).ends_with("does not exist"), "{result}");

    let result = server.call("symbols-info", json!({ "paths": [text(&file)] }));
    assert!(
        error_text(&result).contains("not a PE, PDB, ELF, Mach-O or Breakpad .sym file"),
        "{result}"
    );

    let result = server.call(
        "symbols-upload",
        json!({
            "application": "MyApp",
            "version": "1.0.0",
            "directory": text(&file),
            "files": "*.pdb",
            "dryRun": true,
        }),
    );
    assert!(
        error_text(&result).ends_with("is a file, not a folder"),
        "{result}"
    );

    let result = server.call("symbols-find", json!({ "path": text(dir.path()) }));
    assert!(
        error_text(&result).starts_with("Invalid arguments:"),
        "{result}"
    );

    let unknown = server.request("tools/call", json!({ "name": "symbols-delete" }));
    assert_eq!(unknown["error"]["code"], -32602, "{unknown}");

    let result = server.call(
        "symbols-info",
        json!({ "paths": [text(&fixture("elfy/hello.elf"))] }),
    );
    assert_eq!(structured(&result)["objects"][0]["moduleName"], "hello.elf");

    let finished = server.finish();
    assert!(finished.status.success(), "{}", finished.stderr);
    finished.assert_stdout_is_only_protocol();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_lists_what_would_be_uploaded_and_sends_nothing() {
    let api = upload_server().await;
    let uri = api.uri();
    let dir = build();
    let directory = text(dir.path()).to_string();

    let finished = off_runtime(move || {
        let mut server = Server::ready(&uri, |_| {});
        let result = server.call(
            "symbols-upload",
            json!({
                "application": "MyApp",
                "version": "1.0.0",
                "directory": directory,
                "files": "*",
                "dryRun": true,
            }),
        );
        let plan = structured(&result);
        assert_eq!(plan["dryRun"], true);
        assert_eq!(plan["database"], Value::Null);
        assert_eq!(plan["uploaded"], 0);
        assert_eq!(plan["wouldUpload"], 2);
        assert_eq!(plan["skipped"], 1);
        assert_eq!(plan["fileCount"], 3);
        assert_eq!(plan["files"][0]["name"], "bugsplat.pdb");
        assert_eq!(plan["files"][0]["debugId"], PDB_ID);
        assert_eq!(plan["files"][0]["route"], "symbol-server");
        assert_eq!(plan["files"][0]["status"], "would-upload");
        assert_eq!(plan["files"][2]["status"], "skipped");
        server.finish()
    })
    .await;

    assert_eq!(sent(&api).await, Vec::<String>::new());
    assert_eq!(finished.stderr, "");
    finished.assert_stdout_is_only_protocol();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upload_sends_the_files_and_keeps_progress_off_stdout() {
    let api = upload_server().await;
    let uri = api.uri();
    let dir = build();
    let directory = text(dir.path()).to_string();

    let finished = off_runtime(move || {
        let mut server = Server::ready(&uri, |command| {
            command
                .env("BUGSPLAT_ACCESS_TOKEN", TOKEN)
                .env("BUGSPLAT_DATABASE", "started-with");
        });
        let result = server.call(
            "symbols-upload",
            json!({
                "database": "fred",
                "application": "MyApp",
                "version": "1.0.0",
                "directory": directory,
                "files": "*",
            }),
        );
        let summary = structured(&result);
        assert_eq!(summary["dryRun"], false);
        assert_eq!(summary["database"], "fred");
        assert_eq!(summary["uploaded"], 2);
        assert_eq!(summary["skipped"], 1);
        assert!(summary["bytes"].as_u64().unwrap() > 0);
        assert_eq!(summary["files"][0]["status"], "uploaded");
        assert_eq!(summary["files"][2]["skipReason"], "it has no debug id");
        server.finish()
    })
    .await;

    let mut requests = sent(&api).await;
    requests.sort();
    assert_eq!(
        requests,
        [
            "POST /api/v2/versions",
            "POST /symsrv/uploadComplete",
            "POST /symsrv/uploadUrl",
            "PUT /bucket/symsrv",
            "PUT /bucket/versions",
        ]
    );
    for request in api.received_requests().await.unwrap() {
        if request.method == "POST" {
            let authorization = request.headers.get("authorization").unwrap();
            assert_eq!(authorization.to_str().unwrap(), format!("Bearer {TOKEN}"));
            let form = String::from_utf8_lossy(&request.body).into_owned();
            assert!(form.contains("name=\"database\"\r\n\r\nfred\r\n"), "{form}");
        }
    }

    assert!(finished.status.success(), "{}", finished.stderr);
    for line in [
        "symbols-upload: uploaded bugsplat.pdb",
        "symbols-upload: uploaded main.js.map.zip",
        "symbols-upload: skipping notes.txt: it has no debug id",
    ] {
        assert!(finished.stderr.contains(line), "{}", finished.stderr);
    }
    finished.assert_stdout_is_only_protocol();
    let everything = format!("{}{}", finished.stdout.join("\n"), finished.stderr);
    assert!(!everything.contains(TOKEN));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_database_defaults_to_the_one_the_server_was_started_with() {
    let api = upload_server().await;
    let uri = api.uri();
    let dir = build();
    let directory = text(dir.path()).to_string();

    let finished = off_runtime(move || {
        let mut server = Server::ready(&uri, |command| {
            command
                .env("BUGSPLAT_ACCESS_TOKEN", TOKEN)
                .args(["--database", "fred", "--quiet"]);
        });
        let result = server.call(
            "symbols-upload",
            json!({
                "application": "MyApp",
                "version": "1.0.0",
                "directory": directory,
                "files": "*.map",
            }),
        );
        let summary = structured(&result);
        assert_eq!(summary["database"], "fred");
        assert_eq!(summary["uploaded"], 1);
        server.finish()
    })
    .await;

    let requests = api.received_requests().await.unwrap();
    let form = String::from_utf8_lossy(&requests[0].body).into_owned();
    assert!(form.contains("name=\"database\"\r\n\r\nfred\r\n"), "{form}");
    assert_eq!(finished.stderr, "", "--quiet keeps progress off stderr");
    finished.assert_stdout_is_only_protocol();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upload_without_a_session_says_to_run_bugsplat_login() {
    let api = upload_server().await;
    let uri = api.uri();
    let dir = build();
    let directory = text(dir.path()).to_string();

    let finished = off_runtime(move || {
        let mut server = Server::ready(&uri, |_| {});
        let result = server.call(
            "symbols-upload",
            json!({
                "database": "fred",
                "application": "MyApp",
                "version": "1.0.0",
                "directory": directory,
                "files": "*",
            }),
        );
        let message = error_text(&result);
        assert!(
            message.contains("`bugsplat login` in a terminal"),
            "{message}"
        );
        assert!(message.contains("nothing was uploaded"), "{message}");
        server.finish()
    })
    .await;

    assert_eq!(sent(&api).await, Vec::<String>::new());
    assert!(finished.status.success(), "{}", finished.stderr);
    finished.assert_stdout_is_only_protocol();
}

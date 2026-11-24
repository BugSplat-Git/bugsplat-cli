//! `symbols upload` and `symbols delete` through the binary, against a mock server.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const TOKEN: &str = "cli-test-token";

fn bugsplat(server: &MockServer, dir: &Path) -> Command {
    let mut command = Command::cargo_bin("bugsplat").unwrap();
    for name in [
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
        .env("BUGSPLAT_API_URL", server.uri())
        .env("BUGSPLAT_ACCESS_TOKEN", TOKEN)
        .env("BUGSPLAT_CONFIG_DIR", dir.join("config"))
        .current_dir(dir);
    command
}

async fn run(mut command: Command) -> assert_cmd::assert::Assert {
    tokio::task::spawn_blocking(move || command.assert())
        .await
        .unwrap()
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

async fn requests(server: &MockServer) -> Vec<Request> {
    server.received_requests().await.unwrap()
}

fn sent(requests: &[Request]) -> Vec<String> {
    requests
        .iter()
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect()
}

fn body_text(request: &Request) -> String {
    String::from_utf8_lossy(&request.body).into_owned()
}

fn stdout(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

fn stderr(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stderr.clone()).unwrap()
}

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("dist")).unwrap();
    fs::write(dir.path().join("dist/main.js.map"), br#"{"version":3}"#).unwrap();
    fs::write(dir.path().join("dist/notes.txt"), b"not a symbol file").unwrap();
    dir
}

#[tokio::test(flavor = "multi_thread")]
async fn uploads_source_maps_with_the_short_flags_symbol_upload_users_know() {
    let server = upload_server().await;
    let dir = project();
    let mut command = bugsplat(&server, dir.path());
    command.args([
        "symbols", "upload", "-b", "fred", "-a", "MyApp", "-v", "1.0.0", "-d", "dist",
    ]);

    let assert = run(command).await.success();

    let out = stdout(&assert);
    assert!(out.contains("Uploaded 1 file"), "{out}");
    assert!(out.contains("fred"), "{out}");
    let err = stderr(&assert);
    assert!(err.contains("uploaded main.js.map.zip"), "{err}");
    assert!(!format!("{out}{err}").contains(TOKEN));

    let requests = requests(&server).await;
    assert_eq!(
        sent(&requests),
        ["POST /api/v2/versions", "PUT /bucket/versions"]
    );
    let authorization = requests[0].headers.get("authorization").unwrap();
    assert_eq!(authorization.to_str().unwrap(), format!("Bearer {TOKEN}"));
    let form = body_text(&requests[0]);
    for field in [
        "name=\"database\"\r\n\r\nfred\r\n",
        "name=\"appName\"\r\n\r\nMyApp\r\n",
        "name=\"appVersion\"\r\n\r\n1.0.0\r\n",
        "name=\"symFileName\"\r\n\r\nmain.js.map.zip\r\n",
    ] {
        assert!(form.contains(field), "missing {field:?} in {form}");
    }
    assert_eq!(&requests[1].body[..2], b"PK");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_without_a_debug_id_is_skipped_with_a_warning() {
    let server = upload_server().await;
    let dir = project();
    let mut command = bugsplat(&server, dir.path());
    command.args([
        "symbols", "upload", "-b", "fred", "-a", "MyApp", "-v", "1.0.0", "-d", "dist", "-f", "*",
    ]);

    let assert = run(command).await.success();

    assert!(
        stderr(&assert).contains("warning: skipping notes.txt: it has no debug id"),
        "{}",
        stderr(&assert)
    );
    assert!(stdout(&assert).contains("Skipped 1 file."));
    assert_eq!(requests(&server).await.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn json_prints_the_summary_and_quiet_prints_nothing_else() {
    let server = upload_server().await;
    let dir = project();
    let mut command = bugsplat(&server, dir.path());
    command.args([
        "--json", "--quiet", "symbols", "upload", "-b", "fred", "-a", "MyApp", "-v", "1.0.0", "-d",
        "dist", "-f", "*", "-w", "2",
    ]);

    let assert = run(command).await.success();

    assert_eq!(stderr(&assert), "");
    let summary: Value = serde_json::from_str(&stdout(&assert)).unwrap();
    assert_eq!(summary["database"], "fred");
    assert_eq!(summary["application"], "MyApp");
    assert_eq!(summary["version"], "1.0.0");
    assert_eq!(summary["dryRun"], false);
    assert_eq!(summary["uploaded"], 1);
    assert_eq!(summary["skipped"], 1);
    assert!(summary["bytes"].as_u64().unwrap() > 0);
    assert!(summary["seconds"].is_number());
    assert!(summary["bytesPerSecond"].is_number());
    let files = summary["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["name"], "main.js.map.zip");
    assert_eq!(files[0]["route"], "versions");
    assert_eq!(files[0]["status"], "uploaded");
    assert_eq!(files[0]["uploadedBytes"], summary["bytes"]);
    assert_eq!(files[1]["status"], "skipped");
    assert_eq!(files[1]["skipReason"], "it has no debug id");
}

#[tokio::test(flavor = "multi_thread")]
async fn dry_run_lists_the_files_and_sends_nothing_even_when_signed_out() {
    let server = upload_server().await;
    let dir = project();
    let mut command = bugsplat(&server, dir.path());
    command.env_remove("BUGSPLAT_ACCESS_TOKEN").args([
        "symbols",
        "upload",
        "-d",
        "dist",
        "-f",
        "*",
        "--dry-run",
    ]);

    let assert = run(command).await.success();

    let out = stdout(&assert);
    assert!(out.contains("DEBUG ID"), "{out}");
    assert!(out.contains("versions"), "{out}");
    assert!(out.contains("main.js.map"), "{out}");
    assert!(out.contains("skip"), "{out}");
    assert!(out.contains("notes.txt (it has no debug id)"), "{out}");
    assert!(
        out.contains("1 file would be uploaded. Skipped 1 file."),
        "{out}"
    );
    assert!(requests(&server).await.is_empty());

    let mut command = bugsplat(&server, dir.path());
    command.args([
        "--json",
        "symbols",
        "upload",
        "-b",
        "fred",
        "-a",
        "MyApp",
        "-v",
        "1.0.0",
        "-d",
        "dist",
        "--dry-run",
    ]);
    let assert = run(command).await.success();
    let plan: Value = serde_json::from_str(&stdout(&assert)).unwrap();
    assert_eq!(plan["dryRun"], true);
    assert_eq!(plan["uploaded"], 0);
    assert_eq!(plan["files"][0]["status"], "would-upload");
    assert_eq!(plan["files"][0]["debugId"], "");
    assert_eq!(plan["files"][0]["route"], "versions");
    assert!(requests(&server).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn application_and_version_default_to_package_json() {
    let server = upload_server().await;
    let dir = project();
    fs::write(
        dir.path().join("package.json"),
        br#"{"name":"my-node-app","version":"2.3.4","database":"fred"}"#,
    )
    .unwrap();
    let mut command = bugsplat(&server, dir.path());
    command.args(["symbols", "upload", "-d", "dist"]);

    run(command).await.success();

    let form = body_text(&requests(&server).await[0]);
    assert!(form.contains("name=\"database\"\r\n\r\nfred\r\n"), "{form}");
    assert!(
        form.contains("name=\"appName\"\r\n\r\nmy-node-app\r\n"),
        "{form}"
    );
    assert!(
        form.contains("name=\"appVersion\"\r\n\r\n2.3.4\r\n"),
        "{form}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_names_are_usage_errors() {
    let server = upload_server().await;
    let dir = project();

    let mut command = bugsplat(&server, dir.path());
    command.args([
        "symbols", "upload", "-b", "fred", "-v", "1.0.0", "-d", "dist",
    ]);
    run(command)
        .await
        .code(2)
        .stderr(predicate::str::contains("--application"));

    let mut command = bugsplat(&server, dir.path());
    command.args([
        "symbols", "upload", "-a", "MyApp", "-v", "1.0.0", "-d", "dist",
    ]);
    run(command)
        .await
        .code(2)
        .stderr(predicate::str::contains("--database"));

    let mut command = bugsplat(&server, dir.path());
    command.args([
        "symbols", "upload", "-b", "fred", "-a", "MyApp", "-v", "1.0.0", "-w", "0",
    ]);
    run(command).await.code(2);

    assert!(requests(&server).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn signed_out_is_exit_code_3_before_anything_is_sent() {
    let server = upload_server().await;
    let dir = project();
    let mut command = bugsplat(&server, dir.path());
    command.env_remove("BUGSPLAT_ACCESS_TOKEN").args([
        "symbols", "upload", "-b", "fred", "-a", "MyApp", "-v", "1.0.0", "-d", "dist",
    ]);

    run(command).await.code(3);
    assert!(requests(&server).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn no_matching_files_is_a_failure() {
    let server = upload_server().await;
    let dir = project();
    let mut command = bugsplat(&server, dir.path());
    command.args([
        "symbols", "upload", "-b", "fred", "-a", "MyApp", "-v", "1.0.0", "-d", "dist", "-f",
        "**/*.pdb",
    ]);

    run(command)
        .await
        .code(1)
        .stderr(predicate::str::contains("Could not find any files"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rejected_upload_fails_with_the_servers_reason_and_no_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/versions"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({ "message": "Authentication failure" })),
        )
        .mount(&server)
        .await;
    let dir = project();
    let mut command = bugsplat(&server, dir.path());
    command.args([
        "symbols", "upload", "-b", "fred", "-a", "MyApp", "-v", "1.0.0", "-d", "dist",
    ]);

    let assert = run(command).await.code(1);

    let err = stderr(&assert);
    assert!(err.contains("main.js.map"), "{err}");
    assert!(err.contains("status 401"), "{err}");
    assert!(!err.contains(TOKEN), "{err}");
    assert_eq!(requests(&server).await.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn local_path_copies_instead_of_uploading() {
    let server = upload_server().await;
    let dir = project();
    let mut command = bugsplat(&server, dir.path());
    command
        .env_remove("BUGSPLAT_ACCESS_TOKEN")
        .args(["symbols", "upload", "-d", "dist", "-l", "store"]);

    let assert = run(command).await.success();

    assert!(
        stdout(&assert).contains("Copied 0 files"),
        "{}",
        stdout(&assert)
    );
    assert!(
        stderr(&assert).contains("warning: skipping main.js.map"),
        "{}",
        stderr(&assert)
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("store/index.txt")).unwrap(),
        "."
    );
    assert!(requests(&server).await.is_empty());
}

async fn delete_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/api/v2/versions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "Status": "Success" })))
        .mount(&server)
        .await;
    server
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_with_yes_deletes_the_version() {
    let server = delete_server().await;
    let dir = tempfile::tempdir().unwrap();
    let mut command = bugsplat(&server, dir.path());
    command.args([
        "symbols", "delete", "-b", "fred", "-a", "MyApp", "-v", "1.0.0", "--yes",
    ]);

    let assert = run(command).await.success();

    assert!(stdout(&assert).contains("Deleted the symbol files for MyApp 1.0.0 in fred."));
    let requests = requests(&server).await;
    assert_eq!(sent(&requests), ["DELETE /api/v2/versions"]);
    assert_eq!(
        requests[0].url.query(),
        Some("database=fred&appName=MyApp&appVersion=1.0.0")
    );
    let authorization = requests[0].headers.get("authorization").unwrap();
    assert_eq!(authorization.to_str().unwrap(), format!("Bearer {TOKEN}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_without_a_terminal_requires_yes() {
    let server = delete_server().await;
    let dir = tempfile::tempdir().unwrap();
    let mut command = bugsplat(&server, dir.path());
    command.args([
        "symbols", "delete", "-b", "fred", "-a", "MyApp", "-v", "1.0.0",
    ]);

    run(command)
        .await
        .code(2)
        .stderr(predicate::str::contains("--yes"));
    assert!(requests(&server).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_prints_json_and_reports_a_failure() {
    let server = delete_server().await;
    let dir = tempfile::tempdir().unwrap();
    let mut command = bugsplat(&server, dir.path());
    command.args([
        "--json", "symbols", "delete", "-b", "fred", "-a", "MyApp", "-v", "1.0.0", "-y",
    ]);
    let assert = run(command).await.success();
    let result: Value = serde_json::from_str(&stdout(&assert)).unwrap();
    assert_eq!(
        result,
        json!({ "database": "fred", "application": "MyApp", "version": "1.0.0", "deleted": true })
    );

    let failing = MockServer::start().await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&failing)
        .await;
    let mut command = bugsplat(&failing, dir.path());
    command.args([
        "symbols", "delete", "-b", "fred", "-a", "MyApp", "-v", "1.0.0", "-y",
    ]);
    run(command).await.code(1).stderr(predicate::str::contains(
        "Error deleting symbols for fred-MyApp-1.0.0",
    ));
}

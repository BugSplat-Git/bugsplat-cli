//! `bugsplat symbols dump` and `bugsplat symbols upload --dump-syms`.
#![cfg(feature = "dump-syms")]

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::{Value, json};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const TOKEN: &str = "cli-test-token";
const HOCKEY: &str = "macho-uuid/bugsplat-ios.app/Frameworks/bugsplat.framework/HockeySDKResources.bundle/Contents/MacOS/HockeySDKResources";

fn bugsplat(dir: &Path) -> Command {
    let mut command = Command::cargo_bin("bugsplat").unwrap();
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
        .env("BUGSPLAT_CONFIG_DIR", dir.join("config"))
        .current_dir(dir);
    command
}

fn fixture(relative: &str) -> PathBuf {
    let mut path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("bugsplat-debugid")
        .join("tests")
        .join("fixtures");
    path.extend(relative.split('/'));
    path
}

fn stdout(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

fn stderr(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stderr.clone()).unwrap()
}

fn json_stdout(assert: &assert_cmd::assert::Assert) -> Value {
    serde_json::from_slice(&assert.get_output().stdout).unwrap()
}

fn first_line(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap();
    text.lines().next().unwrap().to_string()
}

/// What `symbols info` reports for a file: `(debug id, module name, kind)`.
fn info(dir: &Path, file: &Path) -> (String, String, String) {
    let assert = bugsplat(dir)
        .args(["symbols", "info", "--json"])
        .arg(file)
        .assert()
        .success();
    let object = &json_stdout(&assert)[0];
    let field = |name: &str| object[name].as_str().unwrap().to_string();
    (field("debugId"), field("moduleName"), field("kind"))
}

#[test]
fn dump_writes_a_sym_file_for_each_format_that_symbols_info_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("syms");
    // (input, .sym file, MODULE line, the debug id and module name symbol-upload sends)
    let cases = [
        (
            "elfy/hello.elf",
            "hello.sym",
            "MODULE Linux x86_64 6F21FE85DDC71F4404C237310A56081F0 hello.elf",
            ("6F21FE85DDC71F4404C237310A56081F0", "hello"),
        ),
        (
            "macho-uuid/bugsplat-ios.app/bugsplat-ios",
            "bugsplat-ios.sym",
            "MODULE Mac arm64 59686D50E0A43FB9AD27BAADAD47E0BC0 bugsplat-ios",
            ("59686D50E0A43FB9AD27BAADAD47E0BC0", "bugsplat-ios"),
        ),
        (
            "macho-uuid/bugsplat.app.dSYM",
            "bugsplat.sym",
            "MODULE Mac x86_64 AC15902FBA2E31C18B29DA51E889B3210 bugsplat",
            ("AC15902FBA2E31C18B29DA51E889B3210", "bugsplat"),
        ),
        (
            "pdb-guid/bssndrpt.exe",
            "BsSndRpt.sym",
            "MODULE windows x86 1B2C11749B0541AF95DA13C6186D83F41 BsSndRpt.pdb",
            ("1B2C11749B0541AF95DA13C6186D83F41", "BsSndRpt.pdb"),
        ),
    ];
    for (input, file_name, module_line, (debug_id, module_name)) in cases {
        let assert = bugsplat(dir.path())
            .args(["symbols", "dump", "--json", "--output"])
            .arg(&out)
            .arg(fixture(input))
            .assert()
            .success();

        let sym = out.join(file_name);
        assert_eq!(first_line(&sym), module_line, "{input}");
        let dumped = json_stdout(&assert);
        assert_eq!(dumped.as_array().unwrap().len(), 1, "{input}");
        assert_eq!(dumped[0]["path"], json!(sym), "{input}");
        assert_eq!(dumped[0]["debugId"], debug_id, "{input}");
        assert_eq!(dumped[0]["moduleName"], module_name, "{input}");
        assert_eq!(
            info(dir.path(), &sym),
            (
                debug_id.to_string(),
                module_name.to_string(),
                "breakpad-sym".to_string()
            ),
            "{input}"
        );
    }
}

#[test]
fn dump_prints_what_it_wrote_and_defaults_to_the_current_folder() {
    let dir = tempfile::tempdir().unwrap();
    let pdb = fixture("pdb-guid/bugsplat.pdb");
    let assert = bugsplat(dir.path())
        .args(["symbols", "dump"])
        .arg(&pdb)
        .assert()
        .success();

    let sym = Path::new(".").join("bugsplat.sym");
    assert_eq!(
        stdout(&assert),
        format!(
            "{}\n  source    {}\n  module    bugsplat.pdb\n  debug id  E546B55B6D214E86871B40AC35CD0D461\n  os        windows\n  arch      x86\n",
            sym.display(),
            pdb.display()
        )
    );
    assert_eq!(
        first_line(&dir.path().join("bugsplat.sym")),
        "MODULE windows x86 E546B55B6D214E86871B40AC35CD0D461 bugsplat.pdb"
    );
}

#[test]
fn dump_keeps_files_that_share_a_name_apart_by_debug_id() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("syms");
    let assert = bugsplat(dir.path())
        .args(["symbols", "dump", "--json", "-o"])
        .arg(&out)
        .arg(fixture(HOCKEY))
        .arg(fixture("elfy/hello.elf"))
        .arg(fixture("elfy/hello.elf"))
        .assert()
        .success();

    let paths: Vec<PathBuf> = json_stdout(&assert)
        .as_array()
        .unwrap()
        .iter()
        .map(|sym| PathBuf::from(sym["path"].as_str().unwrap()))
        .collect();
    let x86_64 = out
        .join("39D9CB72DE663CA6B85BBFA04B60782E0")
        .join("HockeySDKResources.sym");
    let arm64 = out
        .join("7F2B35C3974533D1B991EC0DA93CC0850")
        .join("HockeySDKResources.sym");
    assert_eq!(
        paths,
        [
            x86_64.clone(),
            arm64.clone(),
            out.join("hello.sym"),
            out.join("6F21FE85DDC71F4404C237310A56081F0")
                .join("hello.sym"),
        ]
    );
    assert_eq!(
        first_line(&x86_64),
        "MODULE Mac x86_64 39D9CB72DE663CA6B85BBFA04B60782E0 HockeySDKResources"
    );
    assert_eq!(
        first_line(&arm64),
        "MODULE Mac arm64 7F2B35C3974533D1B991EC0DA93CC0850 HockeySDKResources"
    );
}

#[test]
fn dump_converts_what_it_can_and_fails_for_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let portable = fixture("symbol-upload/portable.pdb");
    let assert = bugsplat(dir.path())
        .args(["symbols", "dump"])
        .arg(&portable)
        .assert()
        .code(1);
    assert_eq!(stdout(&assert), "");
    let err = stderr(&assert);
    assert!(
        err.contains(&format!(
            "{}: not an ELF, Mach-O, PE or PDB file, or a .dSYM bundle",
            portable.display()
        )),
        "{err}"
    );

    let assert = bugsplat(dir.path())
        .args(["symbols", "dump"])
        .arg(&portable)
        .arg(fixture("elfy/hello.elf"))
        .arg(dir.path().join("missing.dll"))
        .assert()
        .code(1);
    assert!(stdout(&assert).contains("hello.sym"));
    let err = stderr(&assert);
    assert!(err.contains("2 of 3 files could not be converted"), "{err}");
    assert!(dir.path().join("hello.sym").is_file());

    bugsplat(dir.path())
        .args(["symbols", "dump"])
        .assert()
        .code(2);
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

fn gunzip(body: &[u8]) -> String {
    use std::io::Read;
    let mut text = String::new();
    flate2::read::GzDecoder::new(body)
        .read_to_string(&mut text)
        .unwrap();
    text
}

/// A folder with one binary `dump_syms` converts and files it doesn't.
fn build_folder() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let build = dir.path().join("build");
    fs::create_dir(&build).unwrap();
    fs::copy(fixture("pdb-guid/bssndrpt.exe"), build.join("bssndrpt.exe")).unwrap();
    fs::copy(
        fixture("symbol-upload/portable.pdb"),
        build.join("portable.pdb"),
    )
    .unwrap();
    fs::write(build.join("main.js.map"), br#"{"version":3}"#).unwrap();
    dir
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_with_dump_syms_sends_the_sym_file_instead_of_the_binary() {
    let server = upload_server().await;
    let dir = build_folder();
    let mut command = bugsplat(dir.path());
    command
        .env("BUGSPLAT_API_URL", server.uri())
        .env("BUGSPLAT_ACCESS_TOKEN", TOKEN)
        .args([
            "symbols", "upload", "-b", "fred", "-a", "MyApp", "-v", "1.0.0", "-d", "build", "-f",
            "*", "-m",
        ]);

    let assert = run(command).await.success();

    let out = stdout(&assert);
    assert!(out.contains("Uploaded 1 file"), "{out}");
    let err = stderr(&assert);
    assert!(err.contains("generated BsSndRpt.sym from"), "{err}");
    assert!(err.contains("uploaded BsSndRpt.sym"), "{err}");
    // As symbol-upload does, what dump_syms can't convert is left out, not uploaded as it is.
    assert!(
        err.contains("warning: could not generate a .sym file:"),
        "{err}"
    );
    assert!(!err.contains("uploaded main.js.map"), "{err}");

    let requests: Vec<Request> = server.received_requests().await.unwrap();
    let sent: Vec<String> = requests
        .iter()
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect();
    assert_eq!(
        sent,
        [
            "POST /symsrv/uploadUrl",
            "PUT /bucket/symsrv",
            "POST /symsrv/uploadComplete"
        ]
    );
    let form = String::from_utf8_lossy(&requests[0].body).into_owned();
    for field in [
        "name=\"symFileName\"\r\n\r\nBsSndRpt.sym\r\n",
        "name=\"moduleName\"\r\n\r\nBsSndRpt.pdb\r\n",
        "name=\"dbgId\"\r\n\r\n1B2C11749B0541AF95DA13C6186D83F41\r\n",
    ] {
        assert!(form.contains(field), "missing {field:?} in {form}");
    }
    assert!(
        gunzip(&requests[1].body)
            .starts_with("MODULE windows x86 1B2C11749B0541AF95DA13C6186D83F41 BsSndRpt.pdb\n")
    );
}

/// symbol-upload's own spec: `-f bssndrpt.exe -m -l <dir>` leaves `BsSndRpt.pdb/<id>/BsSndRpt.sym`.
#[tokio::test(flavor = "multi_thread")]
async fn dump_syms_with_local_path_stores_the_sym_file_under_the_pdb_name() {
    let dir = build_folder();
    let mut command = bugsplat(dir.path());
    command.args([
        "symbols",
        "upload",
        "-d",
        "build",
        "-f",
        "bssndrpt.exe",
        "-m",
        "-l",
        "store",
    ]);

    let assert = run(command).await.success();

    assert!(
        stdout(&assert).contains("Copied 1 file"),
        "{}",
        stdout(&assert)
    );
    let stored = dir
        .path()
        .join("store")
        .join("BsSndRpt.pdb")
        .join("1B2C11749B0541AF95DA13C6186D83F41")
        .join("BsSndRpt.sym");
    assert_eq!(
        first_line(&stored),
        "MODULE windows x86 1B2C11749B0541AF95DA13C6186D83F41 BsSndRpt.pdb"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_with_dump_syms_shows_what_each_sym_file_comes_from() {
    let dir = build_folder();
    let exe = Path::new("build").join("bssndrpt.exe");
    let args = ["symbols", "upload", "-d", "build", "-f", "*.exe", "-m"];

    let mut command = bugsplat(dir.path());
    command.args(args).arg("--dry-run");
    let assert = run(command).await.success();
    let out = stdout(&assert);
    assert!(
        out.contains(&format!(
            "symbol-server  1B2C11749B0541AF95DA13C6186D83F41  BsSndRpt.sym from {} (x86)",
            exe.display()
        )),
        "{out}"
    );
    assert!(out.contains("1 file would be uploaded."), "{out}");

    let mut command = bugsplat(dir.path());
    command.args(args).args(["--dry-run", "--json", "--quiet"]);
    let assert = run(command).await.success();
    assert_eq!(stderr(&assert), "");
    let report = json_stdout(&assert);
    assert_eq!(report["files"][0]["name"], "BsSndRpt.sym");
    assert_eq!(report["files"][0]["moduleName"], "BsSndRpt.pdb");
    assert_eq!(report["files"][0]["status"], "would-upload");
    assert_eq!(report["generated"][0]["source"], json!(exe));
    assert_eq!(
        report["generated"][0]["debugId"],
        "1B2C11749B0541AF95DA13C6186D83F41"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn dump_syms_fails_when_nothing_can_be_converted() {
    let dir = build_folder();
    let mut command = bugsplat(dir.path());
    command.args([
        "symbols",
        "upload",
        "-d",
        "build",
        "-f",
        "*.{pdb,map}",
        "--dump-syms",
        "--dry-run",
    ]);

    let assert = run(command).await.code(1);

    let err = stderr(&assert);
    assert!(
        err.contains("No .sym files could be generated from the files found."),
        "{err}"
    );
    assert_eq!(stdout(&assert), "");
}

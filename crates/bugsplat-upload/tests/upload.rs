//! The upload pipeline against a mock server: both upload paths, the request shapes, what is
//! compressed how, skip rules, retries, the shared rate-limit pause, and delete.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bugsplat_core::{Config, Error};
use bugsplat_debugid::{DebugInfo, ObjectKind, Slice};
use bugsplat_upload::{
    Client, Event, EventSink, Options, RetryPolicy, Route, SymbolFile, Target, TokenProvider, plan,
    upload,
};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const TOKEN: &str = "test-token";

fn target() -> Target {
    Target {
        database: "fred".to_string(),
        application: "my app".to_string(),
        version: "1.0.0".to_string(),
    }
}

fn client(server: &MockServer) -> Client {
    let config = Config::new(Some(&server.uri())).unwrap();
    Client::new(reqwest::Client::new(), &config, TOKEN)
}

fn options() -> Options {
    Options {
        workers: 2,
        retry: RetryPolicy {
            max_retries: 3,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            rate_limit_pause: Duration::from_millis(600),
            max_retry_after: Duration::from_millis(600),
        },
        pause_between_files: Duration::ZERO,
    }
}

fn recorder() -> (EventSink, Arc<Mutex<Vec<Event>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    (
        Arc::new(move |event| sink.lock().unwrap().push(event)),
        events,
    )
}

fn quiet() -> EventSink {
    Arc::new(|_| {})
}

fn write(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, contents).unwrap();
    path
}

fn unsupported(path: &Path) -> bugsplat_debugid::Result<Vec<DebugInfo>> {
    Err(bugsplat_debugid::Error::Unsupported(path.to_path_buf()))
}

fn with_id(debug_id: &'static str) -> impl Fn(&Path) -> bugsplat_debugid::Result<Vec<DebugInfo>> {
    move |path| {
        Ok(vec![DebugInfo {
            path: path.to_path_buf(),
            module_name: path.file_name().unwrap().to_string_lossy().into_owned(),
            debug_id: debug_id.to_string(),
            kind: ObjectKind::Pdb,
            arch: None,
            slice: None,
        }])
    }
}

fn success_with_url(server: &MockServer, key: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "Status": "Success",
        "url": format!("{}/bucket/{key}?X-Amz-Signature=secret", server.uri()),
    }))
}

fn success() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({ "Status": "Success" }))
}

async fn happy_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/symsrv/uploadUrl"))
        .respond_with(success_with_url(&server, "symsrv"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/symsrv/uploadComplete"))
        .respond_with(success())
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v2/versions"))
        .respond_with(success_with_url(&server, "versions"))
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

fn paths_of(requests: &[Request]) -> Vec<String> {
    requests
        .iter()
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect()
}

fn header<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request.headers.get(name).map(|v| v.to_str().unwrap())
}

/// The text fields of a multipart form, in the order they were sent.
fn form_fields(request: &Request) -> Vec<(String, String)> {
    let content_type = header(request, "content-type").unwrap();
    assert!(content_type.starts_with("multipart/form-data"));
    let boundary = content_type.split("boundary=").nth(1).unwrap();
    let body = String::from_utf8(request.body.clone()).unwrap();
    body.split(&format!("--{boundary}"))
        .filter_map(|part| {
            let (headers, value) = part.split_once("\r\n\r\n")?;
            let name = headers.split("name=\"").nth(1)?.split('"').next()?;
            Some((name.to_string(), value.trim_end_matches("\r\n").to_string()))
        })
        .collect()
}

fn gunzip(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut out)
        .unwrap();
    out
}

fn assert_iso8601(value: &str) {
    let shape: String = value
        .chars()
        .map(|c| if c.is_ascii_digit() { '0' } else { c })
        .collect();
    assert_eq!(
        shape, "0000-00-00T00:00:00.000Z",
        "lastModified was {value}"
    );
}

#[tokio::test]
async fn a_file_with_a_debug_id_is_gzipped_and_sent_through_the_symbol_server() {
    let server = happy_server().await;
    let dir = tempfile::tempdir().unwrap();
    let contents = b"Microsoft C/C++ MSF 7.00 ".repeat(200);
    let pdb = write(dir.path(), "MyApp.pdb", &contents);
    let files = plan(&[pdb], with_id("E546B55B6D214E86871B40AC35CD0D461"));

    let (sink, events) = recorder();
    let summary = upload(&client(&server), &target(), &files, &options(), sink)
        .await
        .unwrap();

    let requests = requests(&server).await;
    assert_eq!(
        paths_of(&requests),
        [
            "POST /symsrv/uploadUrl",
            "PUT /bucket/symsrv",
            "POST /symsrv/uploadComplete"
        ]
    );

    let fields = form_fields(&requests[0]);
    let names: Vec<_> = fields.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "database",
            "appName",
            "appVersion",
            "size",
            "symFileName",
            "moduleName",
            "dbgId",
            "lastModified",
            "SendPdbsVersion"
        ]
    );
    let by_name: HashMap<_, _> = fields.iter().cloned().collect();
    assert_eq!(by_name["database"], "fred");
    assert_eq!(by_name["appName"], "my app");
    assert_eq!(by_name["appVersion"], "1.0.0");
    assert_eq!(
        by_name["size"],
        contents.len().to_string(),
        "the size before gzip"
    );
    assert_eq!(by_name["symFileName"], "MyApp.pdb");
    assert_eq!(by_name["moduleName"], "MyApp.pdb");
    assert_eq!(by_name["dbgId"], "E546B55B6D214E86871B40AC35CD0D461");
    assert_eq!(by_name["SendPdbsVersion"], "spdbsv2");
    assert_iso8601(&by_name["lastModified"]);
    assert_eq!(
        form_fields(&requests[2]),
        fields,
        "uploadComplete repeats the fields"
    );

    for request in [&requests[0], &requests[2]] {
        assert_eq!(
            header(request, "authorization"),
            Some("Bearer test-token"),
            "{}",
            request.url.path()
        );
    }

    let put = &requests[1];
    assert_eq!(
        header(put, "content-type"),
        Some("application/octet-stream")
    );
    assert_eq!(header(put, "content-encoding"), Some("gzip"));
    assert_eq!(
        header(put, "content-length"),
        Some(put.body.len().to_string().as_str())
    );
    assert_eq!(
        header(put, "transfer-encoding"),
        None,
        "S3 rejects chunked uploads"
    );
    assert_eq!(
        header(put, "authorization"),
        None,
        "the presigned URL is the credential"
    );
    assert_eq!(put.url.query(), Some("X-Amz-Signature=secret"));
    assert_eq!(&put.body[..2], [0x1f, 0x8b]);
    assert_eq!(gunzip(&put.body), contents);

    assert_eq!((summary.uploaded, summary.skipped), (1, 0));
    assert_eq!(summary.bytes, put.body.len() as u64);
    assert_eq!(summary.files[0].uploaded_bytes, put.body.len() as u64);

    let events = events.lock().unwrap();
    assert!(
        matches!(&events[0], Event::Started { index: 0, name, bytes }
        if name == "MyApp.pdb" && *bytes == put.body.len() as u64)
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Progress { index: 0, sent }
                if *sent == put.body.len() as u64))
    );
    assert!(matches!(
        events.last(),
        Some(Event::Uploaded { index: 0, .. })
    ));
}

#[tokio::test]
async fn a_source_map_is_zipped_and_sent_through_the_versions_endpoint() {
    let server = happy_server().await;
    let dir = tempfile::tempdir().unwrap();
    let contents = br#"{"version":3,"sources":["main.ts"],"mappings":"AAAA"}"#;
    let map = write(dir.path(), "main.js.map", contents);
    let files = plan(&[map], unsupported);

    let summary = upload(&client(&server), &target(), &files, &options(), quiet())
        .await
        .unwrap();

    let requests = requests(&server).await;
    assert_eq!(
        paths_of(&requests),
        ["POST /api/v2/versions", "PUT /bucket/versions"]
    );

    let put = &requests[1];
    let fields = form_fields(&requests[0]);
    let names: Vec<_> = fields.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "database",
            "appName",
            "appVersion",
            "size",
            "symFileName",
            "lastModified",
            "moduleName"
        ],
        "no dbgId and no SendPdbsVersion"
    );
    let by_name: HashMap<_, _> = fields.into_iter().collect();
    assert_eq!(by_name["database"], "fred");
    assert_eq!(by_name["appName"], "my app");
    assert_eq!(by_name["appVersion"], "1.0.0");
    assert_eq!(
        by_name["size"],
        put.body.len().to_string(),
        "the size of the zip"
    );
    assert_eq!(by_name["symFileName"], "main.js.map.zip");
    assert_eq!(by_name["moduleName"], "main.js.map");
    assert_iso8601(&by_name["lastModified"]);
    assert_eq!(
        header(&requests[0], "authorization"),
        Some("Bearer test-token")
    );

    assert_eq!(
        header(put, "content-type"),
        Some("application/octet-stream")
    );
    assert_eq!(header(put, "content-encoding"), None);
    assert_eq!(
        header(put, "content-length"),
        Some(put.body.len().to_string().as_str())
    );
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(put.body.clone())).unwrap();
    assert_eq!(archive.len(), 1);
    let mut entry = archive.by_index(0).unwrap();
    assert_eq!(entry.name().unwrap(), "main.js.map");
    let mut unzipped = Vec::new();
    entry.read_to_end(&mut unzipped).unwrap();
    assert_eq!(unzipped, contents);

    assert_eq!(summary.uploaded, 1);
}

#[tokio::test]
async fn the_other_legacy_kinds_use_the_versions_endpoint_and_a_zip_goes_as_it_is() {
    let server = happy_server().await;
    let dir = tempfile::tempdir().unwrap();
    let zip_bytes = b"PK\x05\x06 not really a zip, and never opened";
    let paths = [
        write(dir.path(), "LineNumberMappings.json", b"{}"),
        write(dir.path(), "UnrealGame.sym", b"MODULE Linux x86_64"),
        write(dir.path(), "symbols.zip", zip_bytes),
    ];
    let files = plan(&paths, unsupported);

    let mut options = options();
    options.workers = 1;
    upload(&client(&server), &target(), &files, &options, quiet())
        .await
        .unwrap();

    let requests = requests(&server).await;
    let posts: Vec<_> = requests
        .iter()
        .filter(|request| request.method.as_str() == "POST")
        .collect();
    assert!(
        posts
            .iter()
            .all(|request| request.url.path() == "/api/v2/versions")
    );
    let names: Vec<_> = posts
        .iter()
        .map(|request| {
            let fields: HashMap<_, _> = form_fields(request).into_iter().collect();
            fields["symFileName"].clone()
        })
        .collect();
    assert_eq!(
        names,
        [
            "LineNumberMappings.json.zip",
            "UnrealGame.sym.zip",
            "symbols.zip"
        ]
    );

    let last_put = requests.last().unwrap();
    assert_eq!(last_put.method.as_str(), "PUT");
    assert_eq!(last_put.body, zip_bytes, "a zip is not zipped again");
    let fields: HashMap<_, _> = form_fields(posts[2]).into_iter().collect();
    assert_eq!(fields["size"], zip_bytes.len().to_string());
}

#[tokio::test]
async fn a_file_without_a_debug_id_is_skipped_with_a_warning() {
    let server = happy_server().await;
    let dir = tempfile::tempdir().unwrap();
    let paths = [
        write(dir.path(), "notes.txt", b"hello"),
        write(dir.path(), "main.js.map", b"{}"),
    ];
    let files = plan(&paths, unsupported);
    assert_eq!(files[0].route, Route::Skip);

    let (sink, events) = recorder();
    let summary = upload(&client(&server), &target(), &files, &options(), sink)
        .await
        .unwrap();

    assert_eq!((summary.uploaded, summary.skipped), (1, 1));
    assert_eq!(summary.files[0].uploaded_bytes, 0);
    let first = events.lock().unwrap()[0].clone();
    assert!(matches!(&first, Event::Skipped { index: 0, name, reason }
        if name == "notes.txt" && reason == "it has no debug id"));
    let requests = requests(&server).await;
    assert_eq!(
        paths_of(&requests),
        ["POST /api/v2/versions", "PUT /bucket/versions"],
        "nothing was sent for notes.txt"
    );
}

#[tokio::test]
async fn each_slice_of_a_fat_file_uploads_under_its_own_id() {
    let server = happy_server().await;
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("MyApp.app.dSYM");
    let dwarf_dir = bundle.join("Contents/Resources/DWARF");
    fs::create_dir_all(&dwarf_dir).unwrap();
    let dwarf = write(
        &dwarf_dir,
        "MyApp",
        b"FATHEADRx86_64-slice....arm64-slice-bytes",
    );
    let slices = [
        ("AAAA0000AAAA0000AAAA0000AAAA0000", "x86_64", 8u64, 16u64),
        ("BBBB1111BBBB1111BBBB1111BBBB1111", "arm64", 24, 17),
    ];
    let reader = |_: &Path| {
        Ok(slices
            .iter()
            .map(|(debug_id, arch, offset, size)| DebugInfo {
                path: dwarf.clone(),
                module_name: "MyApp".to_string(),
                debug_id: debug_id.to_string(),
                kind: ObjectKind::MachO,
                arch: Some(arch.to_string()),
                slice: Some(Slice {
                    offset: *offset,
                    size: *size,
                }),
            })
            .collect())
    };
    let files = plan(&[bundle], reader);
    assert_eq!(files.len(), 2);

    let mut options = options();
    options.workers = 1;
    upload(&client(&server), &target(), &files, &options, quiet())
        .await
        .unwrap();

    let requests = requests(&server).await;
    assert_eq!(requests.len(), 6);
    let uploads: Vec<_> = requests
        .chunks(3)
        .map(|exchange| {
            let fields: HashMap<_, _> = form_fields(&exchange[0]).into_iter().collect();
            (
                fields["dbgId"].clone(),
                fields["symFileName"].clone(),
                fields["moduleName"].clone(),
                fields["size"].clone(),
                String::from_utf8(gunzip(&exchange[1].body)).unwrap(),
            )
        })
        .collect();
    let expected = |id: &str, size: &str, bytes: &str| {
        (
            id.to_string(),
            "MyApp".to_string(),
            "MyApp".to_string(),
            size.to_string(),
            bytes.to_string(),
        )
    };
    assert_eq!(
        uploads,
        [
            expected("AAAA0000AAAA0000AAAA0000AAAA0000", "16", "x86_64-slice...."),
            expected(
                "BBBB1111BBBB1111BBBB1111BBBB1111",
                "17",
                "arm64-slice-bytes"
            ),
        ]
    );
}

#[tokio::test]
async fn a_transient_failure_is_retried_from_the_upload_url() {
    let server = happy_server().await;
    Mock::given(method("POST"))
        .and(path("/symsrv/uploadUrl"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex("^/bucket/"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let files = plan(&[write(dir.path(), "app.pdb", b"pdb")], with_id("ABC1"));

    let (sink, events) = recorder();
    let summary = upload(&client(&server), &target(), &files, &options(), sink)
        .await
        .unwrap();

    assert_eq!(summary.uploaded, 1);
    assert_eq!(
        paths_of(&requests(&server).await),
        [
            "POST /symsrv/uploadUrl",
            "POST /symsrv/uploadUrl",
            "PUT /bucket/symsrv",
            "POST /symsrv/uploadUrl",
            "PUT /bucket/symsrv",
            "POST /symsrv/uploadComplete"
        ]
    );
    let retries = events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| matches!(event, Event::Retrying { .. }))
        .count();
    assert_eq!(retries, 2);
}

#[tokio::test]
async fn a_request_timeout_status_is_retried() {
    let server = happy_server().await;
    Mock::given(method("POST"))
        .and(path("/api/v2/versions"))
        .respond_with(ResponseTemplate::new(408))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let files = plan(&[write(dir.path(), "main.js.map", b"{}")], unsupported);

    upload(&client(&server), &target(), &files, &options(), quiet())
        .await
        .unwrap();
    assert_eq!(requests(&server).await.len(), 3);
}

async fn upload_error(status: u16, body: serde_json::Value) -> (Error, usize) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/symsrv/uploadUrl"))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let files = plan(&[write(dir.path(), "app.pdb", b"pdb")], with_id("ABC1"));
    let error = upload(&client(&server), &target(), &files, &options(), quiet())
        .await
        .unwrap_err();
    (error, requests(&server).await.len())
}

#[tokio::test]
async fn a_4xx_is_not_retried_and_says_what_the_server_said() {
    let (error, sent) = upload_error(
        400,
        serde_json::json!({ "message": "Empty dbgId not supported" }),
    )
    .await;
    assert_eq!(sent, 1);
    assert!(matches!(
        error,
        Error::Api {
            status: Some(400),
            ..
        }
    ));
    let message = error.to_string();
    assert!(message.contains("app.pdb"), "{message}");
    assert!(message.contains("Empty dbgId not supported"), "{message}");
}

#[tokio::test]
async fn forbidden_points_at_the_database_and_the_credentials() {
    let (error, sent) = upload_error(403, serde_json::json!({ "message": "nope" })).await;
    assert_eq!(sent, 1);
    let message = error.to_string();
    assert!(
        message.contains("Not authorized to upload symbols to fred"),
        "{message}"
    );
}

#[tokio::test]
async fn unauthorized_is_not_retried_and_never_shows_the_token() {
    let (error, sent) = upload_error(
        401,
        serde_json::json!({ "message": "Authentication failure" }),
    )
    .await;
    assert_eq!(sent, 1);
    assert!(matches!(
        error,
        Error::Api {
            status: Some(401),
            ..
        }
    ));
    let message = format!("{error} {error:?}");
    assert!(message.contains("bugsplat login"), "{message}");
    assert!(!message.contains(TOKEN), "{message}");
}

#[tokio::test]
async fn a_failed_status_body_is_an_error_and_is_retried_until_the_limit() {
    let (error, sent) = upload_error(
        200,
        serde_json::json!({ "Status": "Failed", "Error": "SQLSTATE[HY000]: server has gone away" }),
    )
    .await;
    assert_eq!(sent, 4, "one attempt and three retries");
    assert!(error.to_string().contains("server has gone away"));
}

#[tokio::test]
async fn a_size_limit_failure_is_final() {
    let (error, sent) = upload_error(
        200,
        serde_json::json!({
            "Status": "Failed",
            "Error": "Symbol file max size 4096 > 2048 mb exceeded fred, my app-1.0.0"
        }),
    )
    .await;
    assert_eq!(sent, 1);
    assert!(error.to_string().contains("Symbol file max size"));
}

#[tokio::test]
async fn a_server_error_gives_up_after_the_retry_limit() {
    let (error, sent) = upload_error(502, serde_json::json!({})).await;
    assert_eq!(sent, 4);
    assert!(matches!(
        error,
        Error::Api {
            status: Some(502),
            ..
        }
    ));
}

#[tokio::test]
async fn a_failed_upload_complete_is_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/symsrv/uploadUrl"))
        .respond_with(success_with_url(&server, "symsrv"))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/symsrv/uploadComplete"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(serde_json::json!({ "message": "A valid dbgId is required" })),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let files = plan(&[write(dir.path(), "app.pdb", b"pdb")], with_id("ABC1"));

    let error = upload(&client(&server), &target(), &files, &options(), quiet())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("A valid dbgId is required"));
    assert_eq!(requests(&server).await.len(), 3);
}

#[tokio::test]
async fn a_storage_error_keeps_the_presigned_url_out_of_the_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/symsrv/uploadUrl"))
        .respond_with(success_with_url(&server, "symsrv"))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(403).set_body_string("<Error>SignatureDoesNotMatch"))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let files = plan(&[write(dir.path(), "app.pdb", b"pdb")], with_id("ABC1"));

    let error = upload(&client(&server), &target(), &files, &options(), quiet())
        .await
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("status 403"), "{message}");
    assert!(!message.contains("X-Amz-Signature"), "{message}");
    assert_eq!(requests(&server).await.len(), 2, "a 403 is not retried");
}

struct Timed {
    arrivals: Arc<Mutex<Vec<(String, Instant)>>>,
    response: ResponseTemplate,
}

impl Respond for Timed {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        self.arrivals
            .lock()
            .unwrap()
            .push((request.url.path().to_string(), Instant::now()));
        self.response.clone()
    }
}

#[tokio::test]
async fn one_429_pauses_every_worker() {
    let server = MockServer::start().await;
    let arrivals = Arc::new(Mutex::new(Vec::new()));
    let timed = |response: ResponseTemplate| Timed {
        arrivals: Arc::clone(&arrivals),
        response,
    };
    Mock::given(method("POST"))
        .and(path("/symsrv/uploadUrl"))
        .respond_with(timed(ResponseTemplate::new(429)))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    // The other worker's answer arrives after the 429 has been seen, so its next request
    // shows whether it honored the pause.
    Mock::given(method("POST"))
        .and(path("/symsrv/uploadUrl"))
        .respond_with(timed(
            success_with_url(&server, "symsrv").set_delay(Duration::from_millis(200)),
        ))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(timed(ResponseTemplate::new(200)))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/symsrv/uploadComplete"))
        .respond_with(timed(success()))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let paths = [
        write(dir.path(), "a.pdb", b"a"),
        write(dir.path(), "b.pdb", b"b"),
    ];
    let files = plan(&paths, with_id("ABC1"));
    let options = options();
    let pause = options.retry.rate_limit_pause;

    let (sink, events) = recorder();
    let summary = upload(&client(&server), &target(), &files, &options, sink)
        .await
        .unwrap();
    assert_eq!(summary.uploaded, 2);

    let arrivals = arrivals.lock().unwrap();
    assert_eq!(arrivals.len(), 7, "one 429 and two complete exchanges");
    let rate_limited_at = arrivals[0].1;
    let during_pause: Vec<_> = arrivals
        .iter()
        .map(|(path, at)| (path, at.duration_since(rate_limited_at)))
        .filter(|(_, after)| {
            *after > Duration::from_millis(150) && *after < pause - Duration::from_millis(20)
        })
        .collect();
    assert!(
        during_pause.is_empty(),
        "requests were sent while rate limited: {during_pause:?}"
    );
    let after_pause = arrivals
        .iter()
        .filter(|(_, at)| at.duration_since(rate_limited_at) >= pause - Duration::from_millis(20))
        .count();
    assert_eq!(
        after_pause, 5,
        "everything else waited for the pause to end"
    );

    let events = events.lock().unwrap();
    let announced = events
        .iter()
        .filter(|event| matches!(event, Event::RateLimited { .. }))
        .count();
    assert_eq!(announced, 1);
}

#[tokio::test]
async fn workers_upload_every_file_once() {
    let server = happy_server().await;
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..9)
        .map(|i| write(dir.path(), &format!("chunk-{i}.js.map"), b"{}"))
        .collect();
    let files = plan(&paths, unsupported);
    let mut options = options();
    options.workers = 4;

    let summary = upload(&client(&server), &target(), &files, &options, quiet())
        .await
        .unwrap();

    assert_eq!(summary.uploaded, 9);
    let mut names: Vec<_> = requests(&server)
        .await
        .iter()
        .filter(|request| request.method.as_str() == "POST")
        .map(|request| {
            let fields: HashMap<_, _> = form_fields(request).into_iter().collect();
            fields["symFileName"].clone()
        })
        .collect();
    names.sort();
    let expected: Vec<_> = (0..9).map(|i| format!("chunk-{i}.js.map.zip")).collect();
    assert_eq!(names, expected);
}

#[tokio::test]
async fn each_worker_pauses_between_its_files() {
    let server = happy_server().await;
    let dir = tempfile::tempdir().unwrap();
    let paths = [
        write(dir.path(), "a.js.map", b"{}"),
        write(dir.path(), "b.js.map", b"{}"),
    ];
    let files = plan(&paths, unsupported);
    let mut options = options();
    options.workers = 1;
    options.pause_between_files = Duration::from_millis(300);

    let started = Instant::now();
    upload(&client(&server), &target(), &files, &options, quiet())
        .await
        .unwrap();
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(300), "{elapsed:?}");
    assert!(
        elapsed < Duration::from_millis(600),
        "no pause after the last file: {elapsed:?}"
    );
}

#[tokio::test]
async fn delete_names_the_version_in_the_query() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/api/v2/versions"))
        .respond_with(success())
        .mount(&server)
        .await;

    client(&server).delete_symbols(&target()).await.unwrap();

    let requests = requests(&server).await;
    assert_eq!(requests.len(), 1);
    let query: HashMap<_, _> = requests[0].url.query_pairs().into_owned().collect();
    assert_eq!(query.len(), 3);
    assert_eq!(query["database"], "fred");
    assert_eq!(query["appName"], "my app");
    assert_eq!(query["appVersion"], "1.0.0");
    assert_eq!(
        header(&requests[0], "authorization"),
        Some("Bearer test-token")
    );
    assert!(requests[0].body.is_empty());
}

#[tokio::test]
async fn a_token_provider_is_asked_for_a_token_before_every_request() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/api/v2/versions"))
        .respond_with(success())
        .mount(&server)
        .await;
    let config = Config::new(Some(&server.uri())).unwrap();
    let calls = Arc::new(Mutex::new(0));
    let counter = calls.clone();
    let provider: TokenProvider = Arc::new(move || {
        let counter = counter.clone();
        Box::pin(async move {
            let mut calls = counter.lock().unwrap();
            *calls += 1;
            Ok(format!("token-{calls}"))
        })
    });
    let client = Client::with_token_provider(reqwest::Client::new(), &config, provider);

    client.delete_symbols(&target()).await.unwrap();
    client.delete_symbols(&target()).await.unwrap();

    let requests = requests(&server).await;
    assert_eq!(
        header(&requests[0], "authorization"),
        Some("Bearer token-1")
    );
    assert_eq!(
        header(&requests[1], "authorization"),
        Some("Bearer token-2")
    );
}

#[tokio::test]
async fn delete_reports_a_bad_status_and_a_failed_body() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "message": "Authentication failure"
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let error = client(&server).delete_symbols(&target()).await.unwrap_err();
    assert!(matches!(
        error,
        Error::Api {
            status: Some(401),
            ..
        }
    ));
    assert!(
        error
            .to_string()
            .contains("Error deleting symbols for fred-my app-1.0.0")
    );

    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Status": "Failed",
            "Error": "Invalid deleteSymbols parameters"
        })))
        .mount(&server)
        .await;
    let error = client(&server).delete_symbols(&target()).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Invalid deleteSymbols parameters")
    );
}

#[test]
fn a_dry_run_plan_lists_every_file_with_its_debug_id_and_route() {
    let dir = tempfile::tempdir().unwrap();
    let paths = [
        write(dir.path(), "app.pdb", b"pdb"),
        write(dir.path(), "main.js.map", b"{}"),
        write(dir.path(), "notes.txt", b"hi"),
    ];
    let files: Vec<SymbolFile> = plan(&paths, |path| {
        if path.extension().unwrap() == "pdb" {
            with_id("ABC1")(path)
        } else {
            unsupported(path)
        }
    });
    let json = serde_json::to_value(&files).unwrap();
    assert_eq!(json[0]["debugId"], "ABC1");
    assert_eq!(json[0]["route"], "symbol-server");
    assert_eq!(json[0]["moduleName"], "app.pdb");
    assert_eq!(json[1]["debugId"], "");
    assert_eq!(json[1]["route"], "versions");
    assert_eq!(json[1]["name"], "main.js.map.zip");
    assert_eq!(json[2]["route"], "skip");
    assert_eq!(json[2]["skipReason"], "it has no debug id");
}

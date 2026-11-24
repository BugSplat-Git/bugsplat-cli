//! End-to-end against LocalStack, never production. Uploads one file through each path to
//! application `bugsplat-cli-test` version `e2e`, checks the version holds them, and deletes
//! them again. BugSplat keeps the emptied version in its list, so the version name is fixed
//! to keep reruns from adding rows.
//!
//!     export BUGSPLAT_API_URL=http://localhost:8085
//!     export BUGSPLAT_ACCESS_TOKEN=$(scripts/localstack-token.py "$EMAIL" "$PASSWORD")
//!     cargo test -p bugsplat-upload --test localstack -- --ignored --nocapture
//!
//! `BUGSPLAT_DATABASE` picks the database (default `Fred`). Set `BUGSPLAT_E2E_KEEP=1` to leave
//! the files in place for a look; the test prints the debug id it made up.

use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bugsplat_core::Config;
use bugsplat_debugid::{DebugInfo, ObjectKind};
use bugsplat_upload::{Client, Options, Route, Target, plan, upload};

const APPLICATION: &str = "bugsplat-cli-test";

/// The megabytes of symbols the versions list shows for the target, if it lists the version.
async fn stored_megabytes(
    http: &reqwest::Client,
    config: &Config,
    token: &str,
    target: &Target,
) -> Option<f64> {
    let response = http
        .get(config.url("/api/v2/versions"))
        .query(&[("database", target.database.as_str())])
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "GET /api/v2/versions");
    let json: serde_json::Value = response.json().await.unwrap();
    json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| {
            row["appName"] == target.application.as_str()
                && row["version"] == target.version.as_str()
        })
        .map(|row| row["size"].as_str().unwrap().parse().unwrap())
}

#[tokio::test]
#[ignore = "needs LocalStack, BUGSPLAT_API_URL and BUGSPLAT_ACCESS_TOKEN"]
async fn uploads_through_both_paths_then_deletes_the_version() {
    let api_url = std::env::var("BUGSPLAT_API_URL").expect("BUGSPLAT_API_URL");
    let token = std::env::var("BUGSPLAT_ACCESS_TOKEN").expect("BUGSPLAT_ACCESS_TOKEN");
    let config = Config::new(Some(&api_url)).unwrap();
    assert!(
        matches!(
            config.api_url.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]")
        ),
        "this test only runs against a BugSplat on this machine, not {api_url}"
    );

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let target = Target {
        database: std::env::var("BUGSPLAT_DATABASE").unwrap_or_else(|_| "Fred".to_string()),
        application: APPLICATION.to_string(),
        version: "e2e".to_string(),
    };
    let debug_id = format!("{nanos:032X}1");

    let dir = tempfile::tempdir().unwrap();
    let pdb = dir.path().join("bugsplat-cli-test.pdb");
    std::fs::write(&pdb, b"not a real pdb; the debug id is made up ".repeat(64)).unwrap();
    let map = dir.path().join("main.js.map");
    std::fs::write(
        &map,
        br#"{"version":3,"sources":["main.ts"],"mappings":"AAAA"}"#,
    )
    .unwrap();

    let reader = |path: &Path| {
        if path.extension().is_some_and(|ext| ext == "pdb") {
            Ok(vec![DebugInfo {
                path: path.to_path_buf(),
                module_name: "bugsplat-cli-test.pdb".to_string(),
                debug_id: debug_id.clone(),
                kind: ObjectKind::Pdb,
                arch: None,
                slice: None,
            }])
        } else {
            Err(bugsplat_debugid::Error::Unsupported(path.to_path_buf()))
        }
    };
    let files = plan(&[pdb, map], reader);
    assert_eq!(
        files.iter().map(|file| file.route).collect::<Vec<_>>(),
        [Route::SymbolServer, Route::Versions]
    );

    let http = bugsplat_core::http_client().unwrap();
    let client = Client::new(http.clone(), &config, token.clone());
    let summary = upload(
        &client,
        &target,
        &files,
        &Options::default(),
        Arc::new(|_| {}),
    )
    .await
    .expect("upload");
    assert_eq!((summary.uploaded, summary.skipped), (2, 0));
    let stored = stored_megabytes(&http, &config, &token, &target).await;
    assert!(
        stored.is_some_and(|megabytes| megabytes > 0.0),
        "the versions list shows {stored:?} MB for {} {} after the upload",
        target.application,
        target.version
    );
    println!(
        "uploaded {} {} to {} with debug id {debug_id}",
        target.application, target.version, target.database
    );

    if std::env::var_os("BUGSPLAT_E2E_KEEP").is_some() {
        return;
    }
    client.delete_symbols(&target).await.expect("delete");
    let stored = stored_megabytes(&http, &config, &token, &target).await;
    assert!(
        stored.is_none_or(|megabytes| megabytes == 0.0),
        "the versions list shows {stored:?} MB for {} {} after the delete",
        target.application,
        target.version
    );
}

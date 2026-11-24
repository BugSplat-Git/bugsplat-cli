//! `symbols-upload`: what `bugsplat symbols upload --json` does, for a model.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use bugsplat_core::{Config, Error};
use bugsplat_upload::{
    Client, Event, FileStatus, Options, RetryPolicy, Summary, SymbolFile, Target, TokenProvider,
};
use rmcp::model::Tool;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Effect, ToolError, ToolResult, blocking, count, definition, readable_folder, resolve};

pub(crate) const NAME: &str = "symbols-upload";

/// A game build has thousands of symbol files; the counts in the result stay exact.
const MAX_FILES: usize = 200;
/// `symbol-upload` retries 15 times because a build shouldn't fail on a bad minute. Here a
/// person is waiting on the answer, and the model can call the tool again.
const MAX_RETRIES: u32 = 3;

pub(crate) fn tool() -> Tool {
    definition(
        NAME,
        "Upload Symbol Files",
        "Upload symbol files from this computer to BugSplat for one application version, so that \
         version's crashes get function names and line numbers. This writes to the user's \
         BugSplat database: call it with dryRun true first, show the user which files would be \
         uploaded, and upload only once they agree. Files with a debug id (PDB, PE, ELF, Mach-O, \
         dSYM, Breakpad .sym) are stored by that id, one upload per architecture; source maps, \
         .sym files without an id, Unity LineNumberMappings JSON and .zip files are stored with \
         the version; every other file is skipped and listed with the reason. application and \
         version must be exactly what the app reports in its crashes; list-versions on the hosted \
         BugSplat server shows them. Uploading needs a signed-in user: when the tool says nobody \
         is signed in, ask the user to run `bugsplat login` in a terminal, then call it again. \
         The result lists at most 200 files; its counts cover all of them.",
        Effect::Reversible,
        json!({
            "type": "object",
            "properties": {
                "application": {
                    "type": "string",
                    "description": "Application name, exactly as the app reports it in its crashes.",
                },
                "version": {
                    "type": "string",
                    "description": "Application version, exactly as the app reports it in its \
                                    crashes.",
                },
                "directory": {
                    "type": "string",
                    "description": "Absolute path of the folder that holds the symbol files. A \
                                    leading ~ is the user's home folder.",
                },
                "files": {
                    "type": "string",
                    "description": "Glob, relative to directory, selecting the files to upload, \
                                    for example **/*.pdb, **/*.{pdb,exe,dll}, **/*.dSYM or \
                                    *.js.map. * and ? stay inside one folder; ** crosses folders.",
                },
                "database": {
                    "type": "string",
                    "description": "BugSplat database to upload to (see list-databases on the \
                                    hosted BugSplat server). Defaults to the database this server \
                                    was started with, if any.",
                },
                "dryRun": {
                    "type": "boolean",
                    "default": false,
                    "description": "true lists what would be uploaded, with each file's debug \
                                    id, and uploads nothing.",
                },
            },
            "required": ["application", "version", "directory", "files"],
        }),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Args {
    application: String,
    version: String,
    directory: String,
    files: String,
    database: Option<String>,
    dry_run: Option<bool>,
}

/// What an upload needs from the server process.
pub(crate) struct Access<'a> {
    pub config: &'a Config,
    pub token: &'a TokenProvider,
    /// Keeps the per-file lines off stderr.
    pub quiet: bool,
}

#[derive(Debug)]
struct Request {
    database: Option<String>,
    application: String,
    version: String,
    directory: PathBuf,
    files: String,
    dry_run: bool,
}

impl Request {
    fn new(args: Args, config: &Config) -> Result<Self, ToolError> {
        let required = |name: &str, value: &str| {
            if value.trim().is_empty() {
                Err(ToolError(format!("{name} can't be empty.")))
            } else {
                Ok(())
            }
        };
        required("application", &args.application)?;
        required("version", &args.version)?;
        required("files", &args.files)?;
        let directory = resolve(&args.directory)?;
        readable_folder(&directory)?;
        Ok(Self {
            database: args
                .database
                .filter(|database| !database.trim().is_empty())
                .or_else(|| config.database.clone()),
            application: args.application,
            version: args.version,
            directory,
            files: args.files,
            dry_run: args.dry_run.unwrap_or(false),
        })
    }

    fn target(&self) -> Result<Target, ToolError> {
        let database = self.database.clone().ok_or_else(|| {
            ToolError::new(
                "No database, so nothing was uploaded. Pass database; list-databases on the \
                 hosted BugSplat server names the ones the user can open. The server can also be \
                 started with --database or BUGSPLAT_DATABASE to set a default.",
            )
        })?;
        Ok(Target {
            database,
            application: self.application.clone(),
            version: self.version.clone(),
        })
    }
}

pub(crate) async fn run(
    args: Args,
    access: Access<'_>,
    cancelled: impl Future<Output = ()>,
) -> ToolResult {
    let request = Request::new(args, access.config)?;

    // Checked before any file is read, so a missing database or session fails at once.
    let target = if request.dry_run {
        None
    } else {
        let target = request.target()?;
        (access.token)().await.map_err(sign_in_error)?;
        Some(target)
    };

    let files = plan(&request).await?;
    let Some(target) = target else {
        return Ok(report(&request, &Summary::dry_run(&files)));
    };

    let http = bugsplat_core::http_client()
        .map_err(|err| ToolError(format!("Could not start the upload: {err}")))?;
    let client = Client::with_token_provider(http, access.config, Arc::clone(access.token));
    let options = Options {
        retry: RetryPolicy {
            max_retries: MAX_RETRIES,
            ..RetryPolicy::default()
        },
        ..Options::default()
    };
    let quiet = access.quiet;
    let events = Arc::new(move |event| {
        if !quiet {
            log(event);
        }
    });
    let summary = tokio::select! {
        result = bugsplat_upload::upload(&client, &target, &files, &options, events) => {
            result.map_err(|err| ToolError(format!(
                "{err}\nThe upload stopped at that file; files uploaded before it stay uploaded."
            )))?
        }
        () = cancelled => {
            return Err(ToolError::new(
                "The upload was cancelled. Files already uploaded stay uploaded.",
            ));
        }
    };
    Ok(report(&request, &summary))
}

async fn plan(request: &Request) -> Result<Vec<SymbolFile>, ToolError> {
    let directory = request.directory.clone();
    let glob = request.files.clone();
    let files = blocking(move || {
        // The message names the CLI's flag; the argument here has no dashes.
        let paths = bugsplat_upload::find_files(&directory, &glob)
            .map_err(|err| ToolError(err.to_string().replace("--files", "files")))?;
        Ok(bugsplat_upload::plan(&paths, bugsplat_debugid::read))
    })
    .await??;
    if files.is_empty() {
        return Err(ToolError(format!(
            "No files under {} match `{}`. Check the folder with symbols-find, or use a glob \
             that crosses folders, such as **/*.pdb.",
            request.directory.display(),
            request.files
        )));
    }
    Ok(files)
}

/// Never starts a sign-in: that opens a browser and waits, which a server whose stdout is
/// the protocol can't do for the user.
fn sign_in_error(err: Error) -> ToolError {
    match err {
        Error::NotSignedIn => ToolError::new(
            "Nobody is signed in to BugSplat, so nothing was uploaded. Ask the user to run \
             `bugsplat login` in a terminal, then call symbols-upload again.",
        ),
        err => ToolError(format!(
            "Could not get a BugSplat access token, so nothing was uploaded: {err}\nIf the \
             session has expired, ask the user to run `bugsplat login` in a terminal."
        )),
    }
}

fn report(request: &Request, summary: &Summary) -> Value {
    let total = summary.files.len();
    let mut value = json!({
        "database": request.database,
        "application": request.application,
        "version": request.version,
        "dryRun": request.dry_run,
        "uploaded": summary.uploaded,
        "skipped": summary.skipped,
        "bytes": summary.bytes,
        "seconds": summary.seconds,
        "bytesPerSecond": summary.bytes_per_second,
        "fileCount": total,
        "files": &summary.files[..total.min(MAX_FILES)],
        "truncated": total > MAX_FILES,
    });
    if request.dry_run {
        value["wouldUpload"] = json!(
            summary
                .files
                .iter()
                .filter(|file| file.status == FileStatus::WouldUpload)
                .count()
        );
    }
    if total > MAX_FILES {
        value["note"] = Value::String(format!(
            "Listing the first {MAX_FILES} of {}; the counts cover all of them. Use a narrower \
             files glob to see the rest.",
            count(total, "file")
        ));
    }
    value
}

fn log(event: Event) {
    match event {
        Event::Skipped { name, reason, .. } => {
            eprintln!("symbols-upload: skipping {name}: {reason}");
        }
        Event::Uploaded { name, bytes, .. } => {
            eprintln!("symbols-upload: uploaded {name} ({bytes} bytes)");
        }
        Event::Retrying {
            name, error, delay, ..
        } => {
            eprintln!(
                "symbols-upload: {name}: {error}; trying again in {:.1}s",
                delay.as_secs_f64()
            );
        }
        Event::RateLimited { pause } => {
            eprintln!(
                "symbols-upload: BugSplat is rate limiting uploads; pausing for {:.0}s",
                pause.as_secs_f64()
            );
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use bugsplat_upload::{FileResult, Route};
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::arguments;
    use super::super::testing::{fixture, object, text};
    use super::*;

    const TOKEN: &str = "mcp-server-test-token";

    fn signed_in() -> TokenProvider {
        Arc::new(|| Box::pin(async { Ok(TOKEN.to_string()) }))
    }

    fn signed_out() -> TokenProvider {
        Arc::new(|| Box::pin(async { Err(Error::NotSignedIn) }))
    }

    /// A config whose API is `uri`, so a test can never reach production.
    fn config(uri: &str, database: Option<&str>) -> Config {
        let mut config = Config::new(Some(uri)).unwrap();
        config.database = database.map(str::to_string);
        config
    }

    async fn call(args: Value, config: &Config, token: &TokenProvider) -> ToolResult {
        let access = Access {
            config,
            token,
            quiet: true,
        };
        run(arguments(object(args)).unwrap(), access, pending()).await
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
    fn build() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        std::fs::copy(fixture("pdb-guid/bugsplat.pdb"), out.join("bugsplat.pdb")).unwrap();
        std::fs::write(out.join("main.js.map"), br#"{"version":3}"#).unwrap();
        std::fs::write(out.join("notes.txt"), b"not a symbol file").unwrap();
        dir
    }

    fn args(dir: &Path, extra: Value) -> Value {
        let mut args = json!({
            "application": "MyApp",
            "version": "1.0.0",
            "directory": text(dir),
            "files": "**/*",
        });
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        args
    }

    #[tokio::test]
    async fn a_dry_run_lists_the_files_and_needs_no_session_or_database() {
        let server = upload_server().await;
        let dir = build();

        let result = call(
            args(dir.path(), json!({ "dryRun": true })),
            &config(&server.uri(), None),
            &signed_out(),
        )
        .await
        .unwrap();

        assert_eq!(result["dryRun"], true);
        assert_eq!(result["database"], Value::Null);
        assert_eq!(result["application"], "MyApp");
        assert_eq!(result["version"], "1.0.0");
        assert_eq!(result["uploaded"], 0);
        assert_eq!(result["wouldUpload"], 2);
        assert_eq!(result["skipped"], 1);
        assert_eq!(result["fileCount"], 3);
        assert_eq!(result["truncated"], false);
        let files = result["files"].as_array().unwrap();
        assert_eq!(files[0]["name"], "bugsplat.pdb");
        assert_eq!(files[0]["debugId"], "E546B55B6D214E86871B40AC35CD0D461");
        assert_eq!(files[0]["route"], "symbol-server");
        assert_eq!(files[0]["status"], "would-upload");
        assert_eq!(files[1]["name"], "main.js.map.zip");
        assert_eq!(files[1]["route"], "versions");
        assert_eq!(files[2]["status"], "skipped");
        assert_eq!(files[2]["skipReason"], "it has no debug id");
        assert_eq!(sent(&server).await, Vec::<String>::new());
    }

    #[tokio::test]
    async fn uploads_to_the_database_in_the_arguments() {
        let server = upload_server().await;
        let dir = build();

        let result = call(
            args(
                dir.path(),
                json!({ "database": "fred", "files": "out/*.pdb" }),
            ),
            &config(&server.uri(), Some("other")),
            &signed_in(),
        )
        .await
        .unwrap();

        assert_eq!(result["dryRun"], false);
        assert_eq!(result["database"], "fred");
        assert_eq!(result["uploaded"], 1);
        assert_eq!(result["skipped"], 0);
        assert!(result["bytes"].as_u64().unwrap() > 0);
        assert!(result.get("wouldUpload").is_none());
        assert_eq!(result["files"][0]["status"], "uploaded");

        assert_eq!(
            sent(&server).await,
            [
                "POST /symsrv/uploadUrl",
                "PUT /bucket/symsrv",
                "POST /symsrv/uploadComplete"
            ]
        );
        let requests = server.received_requests().await.unwrap();
        let authorization = requests[0].headers.get("authorization").unwrap();
        assert_eq!(authorization.to_str().unwrap(), format!("Bearer {TOKEN}"));
        let form = String::from_utf8_lossy(&requests[0].body).into_owned();
        for field in [
            "name=\"database\"\r\n\r\nfred\r\n",
            "name=\"appName\"\r\n\r\nMyApp\r\n",
            "name=\"appVersion\"\r\n\r\n1.0.0\r\n",
        ] {
            assert!(form.contains(field), "missing {field:?} in {form}");
        }
        assert!(!result.to_string().contains(TOKEN));
    }

    #[tokio::test]
    async fn the_database_falls_back_to_the_one_the_server_was_started_with() {
        let server = upload_server().await;
        let dir = build();

        let result = call(
            args(dir.path(), json!({ "files": "out/*.map", "database": "" })),
            &config(&server.uri(), Some("fred")),
            &signed_in(),
        )
        .await
        .unwrap();

        assert_eq!(result["database"], "fred");
        assert_eq!(result["uploaded"], 1);
        let requests = server.received_requests().await.unwrap();
        let form = String::from_utf8_lossy(&requests[0].body).into_owned();
        assert!(form.contains("name=\"database\"\r\n\r\nfred\r\n"), "{form}");
    }

    #[tokio::test]
    async fn no_database_is_a_tool_error_before_anything_is_sent() {
        let server = upload_server().await;
        let dir = build();

        let err = call(
            args(dir.path(), json!({})),
            &config(&server.uri(), None),
            &signed_in(),
        )
        .await
        .unwrap_err();

        assert!(err.0.starts_with("No database"), "{err:?}");
        assert!(err.0.contains("list-databases"), "{err:?}");
        assert_eq!(sent(&server).await, Vec::<String>::new());
    }

    #[tokio::test]
    async fn signed_out_says_to_run_bugsplat_login_and_sends_nothing() {
        let server = upload_server().await;
        let dir = build();

        let err = call(
            args(dir.path(), json!({ "database": "fred" })),
            &config(&server.uri(), None),
            &signed_out(),
        )
        .await
        .unwrap_err();

        assert!(err.0.contains("`bugsplat login` in a terminal"), "{err:?}");
        assert!(err.0.contains("nothing was uploaded"), "{err:?}");
        assert_eq!(sent(&server).await, Vec::<String>::new());
    }

    #[tokio::test]
    async fn a_session_that_cannot_be_refreshed_says_why() {
        let server = upload_server().await;
        let dir = build();
        let expired: TokenProvider =
            Arc::new(|| Box::pin(async { Err(Error::api(400, "invalid_grant")) }));

        let err = call(
            args(dir.path(), json!({ "database": "fred" })),
            &config(&server.uri(), None),
            &expired,
        )
        .await
        .unwrap_err();

        assert!(err.0.contains("invalid_grant"), "{err:?}");
        assert!(err.0.contains("`bugsplat login`"), "{err:?}");
    }

    #[tokio::test]
    async fn a_refused_upload_is_a_tool_error_with_the_servers_reason() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/symsrv/uploadUrl"))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({
                "Status": "Failed",
                "Error": "You do not have access to database fred",
            })))
            .mount(&server)
            .await;
        let dir = build();

        let err = call(
            args(
                dir.path(),
                json!({ "database": "fred", "files": "out/*.pdb" }),
            ),
            &config(&server.uri(), None),
            &signed_in(),
        )
        .await
        .unwrap_err();

        assert!(err.0.contains("bugsplat.pdb"), "{err:?}");
        assert!(err.0.contains("stay uploaded"), "{err:?}");
        assert_eq!(sent(&server).await, ["POST /symsrv/uploadUrl"]);
    }

    #[tokio::test]
    async fn a_cancelled_upload_stops() {
        let server = upload_server().await;
        let dir = build();
        let config = config(&server.uri(), Some("fred"));
        let token = signed_in();
        let access = Access {
            config: &config,
            token: &token,
            quiet: true,
        };

        let err = run(
            arguments(object(args(dir.path(), json!({})))).unwrap(),
            access,
            async {},
        )
        .await
        .unwrap_err();

        assert!(err.0.starts_with("The upload was cancelled"), "{err:?}");
    }

    #[tokio::test]
    async fn a_folder_or_glob_that_cannot_be_used_is_a_tool_error() {
        let server = upload_server().await;
        let config = config(&server.uri(), Some("fred"));
        let dir = build();
        let err = |args: Value| {
            let config = &config;
            async move { call(args, config, &signed_in()).await.unwrap_err().0 }
        };

        let missing = dir.path().join("nope");
        let message = err(args(&missing, json!({}))).await;
        assert_eq!(message, format!("{}: does not exist", missing.display()));

        let file = dir.path().join("out").join("notes.txt");
        let message = err(args(&file, json!({}))).await;
        assert!(message.ends_with("is a file, not a folder"), "{message}");

        let message = err(args(dir.path(), json!({ "files": "*.dll" }))).await;
        assert!(message.starts_with("No files under"), "{message}");
        assert!(message.contains("`*.dll`"), "{message}");
        assert!(message.contains("symbols-find"), "{message}");

        let message = err(args(dir.path(), json!({ "files": "{a" }))).await;
        assert!(message.starts_with("Invalid files glob"), "{message}");

        for (name, extra) in [
            ("application", json!({ "application": " " })),
            ("version", json!({ "version": "" })),
            ("files", json!({ "files": "" })),
        ] {
            let message = err(args(dir.path(), extra)).await;
            assert_eq!(message, format!("{name} can't be empty."));
        }

        let message = err(args(dir.path(), json!({ "directory": "" }))).await;
        assert_eq!(message, "A path can't be empty.");
        assert_eq!(sent(&server).await, Vec::<String>::new());
    }

    #[tokio::test]
    async fn the_token_is_asked_for_before_any_file_is_read() {
        let asked = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&asked);
        let token: TokenProvider = Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
            Box::pin(async { Err(Error::NotSignedIn) })
        });
        let dir = build();

        // A glob that matches nothing: the sign-in error wins because it is checked first.
        let err = call(
            args(dir.path(), json!({ "database": "fred", "files": "*.dll" })),
            &config("http://127.0.0.1:9", None),
            &token,
        )
        .await
        .unwrap_err();

        assert!(err.0.contains("`bugsplat login`"), "{err:?}");
        assert_eq!(asked.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_long_file_list_is_cut_off_and_the_counts_stay_exact() {
        let file = |index: usize| FileResult {
            file: SymbolFile {
                path: PathBuf::from(format!("/build/{index}.pdb")),
                name: format!("{index}.pdb"),
                module_name: format!("{index}.pdb"),
                debug_id: format!("{index:032X}1"),
                arch: None,
                slice: None,
                size: 10,
                route: Route::SymbolServer,
                skip_reason: None,
            },
            status: FileStatus::Uploaded,
            uploaded_bytes: 5,
        };
        let summary = Summary {
            uploaded: 250,
            skipped: 0,
            bytes: 1250,
            seconds: 2.0,
            bytes_per_second: 625.0,
            files: (0..250).map(file).collect(),
        };
        let request = Request {
            database: Some("fred".to_string()),
            application: "MyApp".to_string(),
            version: "1.0.0".to_string(),
            directory: PathBuf::from("/build"),
            files: "*.pdb".to_string(),
            dry_run: false,
        };

        let value = report(&request, &summary);

        assert_eq!(value["files"].as_array().unwrap().len(), 200);
        assert_eq!(value["fileCount"], 250);
        assert_eq!(value["uploaded"], 250);
        assert_eq!(value["truncated"], true);
        assert_eq!(
            value["note"],
            "Listing the first 200 of 250 files; the counts cover all of them. Use a narrower \
             files glob to see the rest."
        );
    }
}

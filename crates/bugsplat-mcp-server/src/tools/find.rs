//! `symbols-find`: what `bugsplat symbols find --json` prints, bounded for a model.

use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bugsplat_debugid::DebugInfo;
use bugsplat_symbols::{Search, SearchResult};
use rmcp::model::Tool;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Effect, ToolError, ToolResult, blocking, definition, not_usable, resolve};

pub(crate) const NAME: &str = "symbols-find";

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 500;
const MAX_WARNINGS: usize = 20;
/// Under the 60 seconds most MCP clients wait for a tool, so the model gets this tool's
/// advice on narrowing the search and not the client's timeout.
const TIME_LIMIT: Duration = Duration::from_secs(45);

pub(crate) fn tool() -> Tool {
    definition(
        NAME,
        "Find Symbol Files",
        "Search folders on this computer for symbol files and binaries (PDB, PE, ELF, Mach-O, \
         dSYM, Breakpad .sym) by debug id, by file or module name, or both. Use it to locate the \
         files a crash needs: get-report on the hosted BugSplat server lists the modules whose \
         symbols are missing, with their debug ids. A file that has any requested id or name is \
         returned with what it matched, and the ids and names nothing matched are listed in \
         missingIds and missingNames. With neither debugIds nor names, every symbol file under \
         the paths is listed. Build folders are large, so search the narrowest folder that could \
         hold the file and set maxDepth when you can. At most limit matches are returned; \
         truncated says when there were more.",
        Effect::Read,
        json!({
            "type": "object",
            "properties": {
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "description": "Absolute paths of the folders to search. A leading ~ is the \
                                    user's home folder. A single file or bundle works too.",
                },
                "debugIds": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Debug ids to look for, in any letter case, with or without \
                                    dashes, as get-report and list-symbols show them.",
                },
                "names": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "File or module names to look for, not case-sensitive, with * \
                                    and ? wildcards, for example myapp.pdb or *.dSYM.",
                },
                "maxDepth": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "How many folder levels below each path to search; 1 is only \
                                    the files directly in it. Defaults to every level.",
                },
                "includeIgnored": {
                    "type": "boolean",
                    "default": false,
                    "description": "Also search .git, node_modules and the other folders that \
                                    are skipped by default.",
                },
                "followLinks": {
                    "type": "boolean",
                    "default": false,
                    "description": "Follow symbolic links.",
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_LIMIT,
                    "default": DEFAULT_LIMIT,
                    "description": "Most matches to return (1-500, defaults to 100).",
                },
            },
            "required": ["paths"],
        }),
    )
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Args {
    paths: Vec<String>,
    debug_ids: Option<Vec<String>>,
    names: Option<Vec<String>>,
    max_depth: Option<usize>,
    include_ignored: Option<bool>,
    follow_links: Option<bool>,
    limit: Option<usize>,
}

pub(crate) async fn run(args: Args, cancelled: impl Future<Output = ()>) -> ToolResult {
    let (search, limit) = plan(args)?;
    let result = search_within(search, bugsplat_debugid::read, TIME_LIMIT, cancelled).await?;
    Ok(report(result, limit))
}

fn plan(args: Args) -> Result<(Search, usize), ToolError> {
    if args.paths.is_empty() {
        return Err(ToolError::new(
            "paths is empty: name at least one folder to search.",
        ));
    }
    let mut paths = Vec::new();
    for raw in &args.paths {
        let path = resolve(raw)?;
        if path.parent().is_none() {
            return Err(ToolError(format!(
                "{}: is the whole disk. Name the folder the build or the symbols are in.",
                path.display()
            )));
        }
        let metadata = std::fs::metadata(&path).map_err(|err| not_usable(&path, &err))?;
        if metadata.is_dir() {
            std::fs::read_dir(&path).map_err(|err| not_usable(&path, &err))?;
        }
        paths.push(path);
    }

    let debug_ids = args.debug_ids.unwrap_or_default();
    for id in &debug_ids {
        let normalized = bugsplat_debugid::normalize_id(id);
        if normalized.is_empty() || !normalized.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ToolError(format!(
                "`{id}` is not a debug id: expected hexadecimal digits, with or without dashes. \
                 To search by file name, use names."
            )));
        }
    }
    let names = args.names.unwrap_or_default();
    if names.iter().any(|name| name.trim().is_empty()) {
        return Err(ToolError::new("A name in names can't be empty."));
    }
    if args.max_depth == Some(0) {
        return Err(ToolError::new(
            "maxDepth must be 1 or more; 1 searches only the files directly in each path.",
        ));
    }
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(ToolError(format!(
            "limit must be between 1 and {MAX_LIMIT}."
        )));
    }

    Ok((
        Search {
            paths,
            debug_ids,
            names,
            max_depth: args.max_depth,
            no_ignore: args.include_ignored.unwrap_or(false),
            follow_links: args.follow_links.unwrap_or(false),
        },
        limit,
    ))
}

/// Runs the search until it finishes, `cancelled` resolves or `time_limit` passes.
///
/// `bugsplat_symbols::find` can't be interrupted, so a search that is given up on stops
/// reading files and only finishes walking the folders, on its own thread.
async fn search_within<R>(
    search: Search,
    read: R,
    time_limit: Duration,
    cancelled: impl Future<Output = ()>,
) -> Result<SearchResult, ToolError>
where
    R: Fn(&Path) -> bugsplat_debugid::Result<Vec<DebugInfo>> + Send + Sync + 'static,
{
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    let work = blocking(move || {
        bugsplat_symbols::find(&search, |path| {
            if stopped.load(Ordering::Relaxed) {
                return Err(bugsplat_debugid::Error::Unsupported(path.to_path_buf()));
            }
            read(path)
        })
    });

    tokio::select! {
        result = work => result?.map_err(|err| ToolError(err.to_string())),
        () = cancelled => {
            stop.store(true, Ordering::Relaxed);
            Err(ToolError::new("The search was cancelled."))
        }
        () = tokio::time::sleep(time_limit) => {
            stop.store(true, Ordering::Relaxed);
            Err(ToolError(format!(
                "The search did not finish in {} seconds and was stopped. Search a narrower \
                 folder, such as the build's output folder instead of the whole project or home \
                 folder, or set maxDepth.",
                time_limit.as_secs()
            )))
        }
    }
}

fn report(result: SearchResult, limit: usize) -> Value {
    let total = result.matches.len();
    let shown = total.min(limit);
    let warnings = result.warnings.len();
    let mut value = json!({
        "matches": &result.matches[..shown],
        "totalMatches": total,
        "truncated": total > limit,
        "missingIds": result.missing_ids,
        "missingNames": result.missing_names,
        "warnings": &result.warnings[..warnings.min(MAX_WARNINGS)],
        "warningsOmitted": warnings.saturating_sub(MAX_WARNINGS),
    });
    if total > limit {
        value["note"] = Value::String(format!(
            "Showing the first {shown} of {total} matches, in path order. Narrow the search with \
             debugIds or names, a folder closer to the files, or maxDepth; or raise limit (up to \
             {MAX_LIMIT})."
        ));
    }
    value
}

#[cfg(test)]
mod tests {
    use std::future::pending;

    use super::super::arguments;
    use super::super::testing::{fixture, object, text};
    use super::*;

    const PDB_ID: &str = "E546B55B6D214E86871B40AC35CD0D461";

    async fn call(args: Value) -> ToolResult {
        run(arguments(object(args)).unwrap(), pending()).await
    }

    fn names(result: &Value) -> Vec<&str> {
        result["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|found| found["moduleName"].as_str().unwrap())
            .collect()
    }

    #[tokio::test]
    async fn finds_a_file_by_its_debug_id_in_any_spelling() {
        let fixtures = fixture("pdb-guid");

        let result = call(json!({
            "paths": [text(&fixtures)],
            "debugIds": ["e546b55b-6d21-4e86-871b-40ac35cd0d46-1"],
        }))
        .await
        .unwrap();

        assert_eq!(names(&result), ["bugsplat.pdb"]);
        let found = &result["matches"][0];
        assert_eq!(found["debugId"], PDB_ID);
        assert_eq!(found["kind"], "pdb");
        assert_eq!(found["path"], text(&fixtures.join("bugsplat.pdb")));
        assert_eq!(
            found["matchedIds"],
            json!(["e546b55b-6d21-4e86-871b-40ac35cd0d46-1"])
        );
        assert_eq!(result["totalMatches"], 1);
        assert_eq!(result["truncated"], false);
        assert_eq!(result["missingIds"], json!([]));
        assert!(result.get("note").is_none());
    }

    #[tokio::test]
    async fn finds_files_by_name_and_reports_what_is_missing() {
        let result = call(json!({
            "paths": [text(&fixture("pdb-guid"))],
            "names": ["*.PDB", "nothere.dll"],
            "debugIds": ["ABCDEF"],
        }))
        .await
        .unwrap();

        assert_eq!(names(&result), ["bugsplat.pdb", "testapp.pdb"]);
        assert_eq!(result["missingNames"], json!(["nothere.dll"]));
        assert_eq!(result["missingIds"], json!(["ABCDEF"]));
    }

    #[tokio::test]
    async fn with_nothing_requested_every_symbol_file_is_listed_up_to_the_limit() {
        let fixtures = text(&fixture("pdb-guid"));

        let all = call(json!({ "paths": [fixtures] })).await.unwrap();
        let total = all["totalMatches"].as_u64().unwrap();
        assert!(total >= 6, "{all}");
        assert_eq!(all["truncated"], false);

        let some = call(json!({ "paths": [fixtures], "limit": 2 }))
            .await
            .unwrap();
        assert_eq!(some["matches"].as_array().unwrap().len(), 2);
        assert_eq!(some["totalMatches"], total);
        assert_eq!(some["truncated"], true);
        let note = some["note"].as_str().unwrap();
        assert!(
            note.starts_with(&format!("Showing the first 2 of {total} matches")),
            "{note}"
        );
        assert!(note.contains("debugIds or names"), "{note}");
        assert!(note.contains("maxDepth"), "{note}");
    }

    #[tokio::test]
    async fn max_depth_keeps_the_search_near_the_top() {
        let fixtures = text(&fixture(""));

        let shallow =
            call(json!({ "paths": [fixtures], "names": ["bugsplat.pdb"], "maxDepth": 1 }))
                .await
                .unwrap();
        assert_eq!(shallow["totalMatches"], 0);
        assert_eq!(shallow["missingNames"], json!(["bugsplat.pdb"]));

        let deep = call(json!({ "paths": [fixtures], "names": ["bugsplat.pdb"], "maxDepth": 2 }))
            .await
            .unwrap();
        assert_eq!(deep["totalMatches"], 1);
    }

    #[tokio::test]
    async fn a_path_that_cannot_be_searched_is_a_tool_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");

        let err = call(json!({ "paths": [text(dir.path()), text(&missing)] }))
            .await
            .unwrap_err();
        assert_eq!(err.0, format!("{}: does not exist", missing.display()));

        let none = call(json!({ "paths": [] })).await.unwrap_err();
        assert!(none.0.contains("at least one folder"), "{none:?}");

        let root = if cfg!(windows) { "C:\\" } else { "/" };
        let whole_disk = call(json!({ "paths": [root] })).await.unwrap_err();
        assert!(whole_disk.0.contains("is the whole disk"), "{whole_disk:?}");
    }

    #[tokio::test]
    async fn arguments_that_make_no_sense_are_tool_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = text(dir.path());
        let err = |args: Value| async move { call(args).await.unwrap_err().0 };

        let not_an_id = err(json!({ "paths": [path], "debugIds": ["myapp.pdb"] })).await;
        assert!(
            not_an_id.contains("`myapp.pdb` is not a debug id"),
            "{not_an_id}"
        );
        assert!(not_an_id.contains("use names"), "{not_an_id}");

        let empty_name = err(json!({ "paths": [path], "names": [""] })).await;
        assert_eq!(empty_name, "A name in names can't be empty.");

        let bad_glob = err(json!({ "paths": [path], "names": ["[abc"] })).await;
        assert!(bad_glob.contains("not a valid name pattern"), "{bad_glob}");

        let depth = err(json!({ "paths": [path], "maxDepth": 0 })).await;
        assert!(depth.starts_with("maxDepth must be 1 or more"), "{depth}");

        for limit in [0, 501] {
            let limit = err(json!({ "paths": [path], "limit": limit })).await;
            assert_eq!(limit, "limit must be between 1 and 500.");
        }
    }

    #[tokio::test]
    async fn optional_arguments_may_be_null() {
        let result = call(json!({
            "paths": [text(&fixture("elfy"))],
            "debugIds": null,
            "names": null,
            "maxDepth": null,
            "includeIgnored": null,
            "followLinks": null,
            "limit": null,
        }))
        .await
        .unwrap();

        assert_eq!(names(&result), ["hello.elf"]);
    }

    fn search(path: &Path) -> Search {
        Search {
            paths: vec![path.to_path_buf()],
            ..Search::default()
        }
    }

    #[tokio::test]
    async fn a_search_that_takes_too_long_is_stopped_with_advice() {
        let slow = |path: &Path| {
            std::thread::sleep(Duration::from_millis(300));
            bugsplat_debugid::read(path)
        };

        let err = search_within(
            search(&fixture("pdb-guid")),
            slow,
            Duration::from_millis(20),
            pending(),
        )
        .await
        .unwrap_err();

        assert!(
            err.0.starts_with("The search did not finish in 0 seconds"),
            "{err:?}"
        );
        assert!(err.0.contains("narrower folder"), "{err:?}");
        assert!(err.0.contains("maxDepth"), "{err:?}");
    }

    #[tokio::test]
    async fn a_cancelled_search_stops() {
        let slow = |path: &Path| {
            std::thread::sleep(Duration::from_millis(300));
            bugsplat_debugid::read(path)
        };

        let err = search_within(
            search(&fixture("pdb-guid")),
            slow,
            Duration::from_secs(60),
            async {},
        )
        .await
        .unwrap_err();

        assert_eq!(err.0, "The search was cancelled.");
    }

    #[test]
    fn warnings_are_capped_and_counted() {
        let result = SearchResult {
            warnings: (0..25).map(|n| format!("/build/{n}: denied")).collect(),
            ..SearchResult::default()
        };

        let value = report(result, 10);

        assert_eq!(value["warnings"].as_array().unwrap().len(), 20);
        assert_eq!(value["warningsOmitted"], 5);
        assert_eq!(value["truncated"], false);
    }
}

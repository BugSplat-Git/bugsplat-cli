//! `symbols-info`: what `bugsplat symbols info --json` prints, for a model.

use std::path::Path;

use bugsplat_debugid::DebugInfo;
use rmcp::model::Tool;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Effect, ToolError, ToolResult, blocking, count, definition, resolve};

pub(crate) const NAME: &str = "symbols-info";

const MAX_PATHS: usize = 100;
/// An `.xcarchive` holds an object for every framework and architecture in it.
const MAX_OBJECTS: usize = 500;

pub(crate) fn tool() -> Tool {
    definition(
        NAME,
        "Inspect Symbol Files",
        "Read the debug id, module name, kind (pe, pdb, portable-pdb, elf, mach-o or breakpad-sym) \
         and architecture of symbol files and binaries on this computer: .pdb, .exe and .dll files, \
         ELF and Mach-O binaries, .dSYM, .app, .framework and .xcarchive bundles, and Breakpad .sym \
         files. The debug id is what BugSplat matches a crash's modules against, so compare it with \
         the ids of the modules get-report on the hosted BugSplat server says are missing symbols, \
         or with list-symbols. A fat Mach-O file or a bundle returns one object per architecture. \
         Files that can't be read are listed in failures. To search a folder, use symbols-find.",
        Effect::Read,
        json!({
            "type": "object",
            "properties": {
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "maxItems": MAX_PATHS,
                    "description": "Absolute paths of the files or bundles to inspect, up to 100. \
                                    A leading ~ is the user's home folder.",
                },
            },
            "required": ["paths"],
        }),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Args {
    paths: Vec<String>,
}

pub(crate) async fn run(args: Args) -> ToolResult {
    if args.paths.is_empty() {
        return Err(ToolError::new("paths is empty: name at least one file."));
    }
    if args.paths.len() > MAX_PATHS {
        return Err(ToolError(format!(
            "{} paths is more than the {MAX_PATHS} one call takes. Split them over several calls, \
             or search their folder with symbols-find.",
            args.paths.len()
        )));
    }
    blocking(move || inspect(&args.paths, MAX_OBJECTS)).await?
}

fn inspect(paths: &[String], max_objects: usize) -> ToolResult {
    let mut objects: Vec<DebugInfo> = Vec::new();
    let mut failures = Vec::new();
    for raw in paths {
        let read = resolve(raw).and_then(|path| {
            bugsplat_debugid::read(&path).map_err(|err| ToolError(describe(&path, err)))
        });
        match read {
            Ok(found) => objects.extend(found),
            Err(ToolError(error)) => failures.push(json!({ "path": raw, "error": error })),
        }
    }

    if objects.is_empty() && !failures.is_empty() {
        let reasons: Vec<&str> = failures
            .iter()
            .filter_map(|failure| failure["error"].as_str())
            .collect();
        return Err(ToolError(reasons.join("\n")));
    }

    let total = objects.len();
    objects.truncate(max_objects);
    let mut result = json!({
        "objects": objects,
        "failures": failures,
        "truncated": total > max_objects,
    });
    if total > max_objects {
        result["note"] = Value::String(format!(
            "Showing the first {max_objects} of {}. Inspect fewer paths, or a file inside the \
             bundle, to see the rest.",
            count(total, "object")
        ));
    }
    Ok(result)
}

fn describe(path: &Path, err: bugsplat_debugid::Error) -> String {
    match err {
        bugsplat_debugid::Error::Unsupported(_) if path.is_dir() => {
            if bugsplat_debugid::is_bundle(path) {
                format!("{}: no Mach-O files in this bundle", path.display())
            } else {
                format!(
                    "{}: is a folder; pass files or a .dSYM, .app, .framework, .bundle or \
                     .xcarchive bundle, or search the folder with symbols-find",
                    path.display()
                )
            }
        }
        bugsplat_debugid::Error::Io { source, .. } => super::not_usable(path, &source).0,
        err => err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::arguments;
    use super::super::testing::{fixture, object, text};
    use super::*;

    async fn call(paths: Value) -> ToolResult {
        run(arguments(object(json!({ "paths": paths }))).unwrap()).await
    }

    #[tokio::test]
    async fn reads_the_debug_id_kind_and_architecture() {
        let pdb = fixture("pdb-guid/bugsplat.pdb");
        let elf = fixture("elfy/hello.elf");

        let result = call(json!([text(&pdb), text(&elf)])).await.unwrap();

        assert_eq!(result["truncated"], false);
        assert_eq!(result["failures"], json!([]));
        let objects = result["objects"].as_array().unwrap();
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0]["path"], text(&pdb));
        assert_eq!(objects[0]["moduleName"], "bugsplat.pdb");
        assert_eq!(objects[0]["debugId"], "E546B55B6D214E86871B40AC35CD0D461");
        assert_eq!(objects[0]["kind"], "pdb");
        assert_eq!(objects[1]["moduleName"], "hello.elf");
        assert_eq!(
            objects[1]["debugId"],
            "85fe216fc7dd441f04c237310a56081fbf23c082"
        );
        assert_eq!(objects[1]["kind"], "elf");
        assert!(objects[1]["arch"].is_string());
    }

    #[tokio::test]
    async fn a_bundle_gives_the_objects_inside_it() {
        let dsym = fixture("macho-uuid/bugsplat.app.dSYM");

        let result = call(json!([text(&dsym)])).await.unwrap();

        let objects = result["objects"].as_array().unwrap();
        assert!(!objects.is_empty());
        assert!(objects.iter().all(|object| object["kind"] == "mach-o"));
    }

    #[tokio::test]
    async fn a_file_that_cannot_be_read_is_listed_next_to_the_ones_that_can() {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join("notes.txt");
        std::fs::write(&notes, "plain text").unwrap();
        let missing = dir.path().join("missing.pdb");
        let pdb = fixture("pdb-guid/bugsplat.pdb");

        let result = call(json!([text(&notes), text(&pdb), text(&missing)]))
            .await
            .unwrap();

        assert_eq!(result["objects"].as_array().unwrap().len(), 1);
        let failures = result["failures"].as_array().unwrap();
        assert_eq!(failures.len(), 2);
        assert_eq!(failures[0]["path"], text(&notes));
        assert!(
            failures[0]["error"]
                .as_str()
                .unwrap()
                .contains("not a PE, PDB, ELF, Mach-O or Breakpad .sym file")
        );
        assert_eq!(failures[1]["path"], text(&missing));
        assert!(
            failures[1]["error"]
                .as_str()
                .unwrap()
                .ends_with("does not exist")
        );
    }

    #[tokio::test]
    async fn nothing_readable_is_a_tool_error_with_every_reason() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.pdb");

        let err = call(json!([text(&missing), text(dir.path()), ""]))
            .await
            .unwrap_err();

        let lines: Vec<&str> = err.0.lines().collect();
        assert_eq!(lines.len(), 3, "{err:?}");
        assert!(lines[0].ends_with("does not exist"), "{err:?}");
        assert!(lines[1].contains("is a folder"), "{err:?}");
        assert!(lines[1].contains("symbols-find"), "{err:?}");
        assert_eq!(lines[2], "A path can't be empty.");
    }

    #[tokio::test]
    async fn the_number_of_paths_is_bounded() {
        let none = call(json!([])).await.unwrap_err();
        assert!(none.0.contains("at least one file"), "{none:?}");

        let many: Vec<String> = (0..=MAX_PATHS).map(|n| format!("/tmp/{n}.pdb")).collect();
        let err = call(json!(many)).await.unwrap_err();
        assert!(
            err.0.starts_with("101 paths is more than the 100"),
            "{err:?}"
        );
        assert!(err.0.contains("symbols-find"), "{err:?}");
    }

    #[test]
    fn a_long_result_is_cut_off_and_says_so() {
        let pdb = text(&fixture("pdb-guid/bugsplat.pdb"));
        let exe = text(&fixture("pdb-guid/bssndrpt.exe"));
        let elf = text(&fixture("elfy/hello.elf"));

        let result = inspect(&[pdb, exe, elf], 2).unwrap();

        assert_eq!(result["objects"].as_array().unwrap().len(), 2);
        assert_eq!(result["truncated"], true);
        assert_eq!(
            result["note"],
            "Showing the first 2 of 3 objects. Inspect fewer paths, or a file inside the bundle, \
             to see the rest."
        );
    }
}

//! What the three tools share: how a tool is described, how its arguments are read, and how
//! a path the model sent becomes one to open.

pub(crate) mod find;
pub(crate) mod info;
pub(crate) mod upload;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// A failure the model should read and act on. It becomes an `isError` tool result, never a
/// protocol error, which clients show as an opaque failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolError(pub String);

impl ToolError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

pub(crate) type ToolResult = Result<Value, ToolError>;

/// What a tool can do, in the hosted server's terms (`McpEffect` in webroot's
/// `mcp/tools/McpTool.php`), so both servers advertise the same hints for the same kind of
/// tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Changes nothing.
    Read,
    /// Adds to the user's BugSplat data in a way that can be put back.
    Reversible,
}

pub(crate) fn definition(
    name: &'static str,
    title: &'static str,
    description: &'static str,
    effect: Effect,
    input_schema: Value,
) -> Tool {
    let Value::Object(input_schema) = input_schema else {
        unreachable!("a tool's input schema is a JSON object");
    };
    // Sending the same files again leaves BugSplat as it was, so every tool here is
    // idempotent. Not open world: the hosted server keeps that hint for tools that reach a
    // system outside BugSplat, such as an issue tracker.
    let annotations = ToolAnnotations::with_title(title)
        .read_only(effect == Effect::Read)
        .destructive(false)
        .idempotent(true)
        .open_world(false);
    Tool::new(name, description, Arc::new(input_schema))
        .with_title(title)
        .with_annotations(annotations)
}

pub(crate) fn arguments<T: DeserializeOwned>(
    arguments: Option<JsonObject>,
) -> Result<T, ToolError> {
    serde_json::from_value(Value::Object(arguments.unwrap_or_default()))
        .map_err(|err| ToolError(format!("Invalid arguments: {err}")))
}

/// A path argument as the tools open it: `~` is the user's home folder, as a shell would
/// have made it, and a relative path is under the folder the server was started in.
pub(crate) fn resolve(raw: &str) -> Result<PathBuf, ToolError> {
    if raw.trim().is_empty() {
        return Err(ToolError::new("A path can't be empty."));
    }
    let path = match home_relative(raw) {
        Some(rest) => std::env::home_dir()
            .ok_or_else(|| {
                ToolError(format!(
                    "{raw}: could not find the home folder; pass an absolute path"
                ))
            })?
            .join(rest),
        None => PathBuf::from(raw),
    };
    if path.is_absolute() {
        return Ok(path);
    }
    let base = std::env::current_dir().map_err(|err| {
        ToolError(format!(
            "{raw}: is a relative path and the server's folder can't be read ({err}); pass an absolute path"
        ))
    })?;
    Ok(base.join(path))
}

fn home_relative(raw: &str) -> Option<&str> {
    let rest = raw.strip_prefix('~')?;
    if rest.is_empty() {
        return Some(rest);
    }
    rest.strip_prefix('/')
        .or_else(|| rest.strip_prefix(std::path::MAIN_SEPARATOR))
}

/// The folder at `path`, checked the way the model needs to hear about it: missing, not a
/// folder, or not readable (on macOS that includes folders the client app has no permission
/// for).
pub(crate) fn readable_folder(path: &Path) -> Result<(), ToolError> {
    let metadata = std::fs::metadata(path).map_err(|err| not_usable(path, &err))?;
    if !metadata.is_dir() {
        return Err(ToolError(format!(
            "{}: is a file, not a folder",
            path.display()
        )));
    }
    std::fs::read_dir(path)
        .map(|_| ())
        .map_err(|err| not_usable(path, &err))
}

pub(crate) fn not_usable(path: &Path, err: &std::io::Error) -> ToolError {
    let reason = match err.kind() {
        std::io::ErrorKind::NotFound => "does not exist".to_string(),
        std::io::ErrorKind::PermissionDenied => format!("can't be read ({err})"),
        _ => err.to_string(),
    };
    ToolError(format!("{}: {reason}", path.display()))
}

/// Runs blocking file work off the async runtime, on a thread of its own rather than
/// `spawn_blocking`: the runtime waits for its blocking pool when the process exits, and a
/// walk of a large folder would keep a server alive after its client has gone.
pub(crate) async fn blocking<T, F>(work: F) -> Result<T, ToolError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (done, result) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("bugsplat-mcp-files".to_string())
        .spawn(move || {
            let _ = done.send(work());
        })
        .map_err(|err| ToolError(format!("Could not start reading files: {err}")))?;
    // The sender is dropped without a value when the work panicked.
    result
        .await
        .map_err(|_| ToolError::new("Reading the files failed unexpectedly."))
}

pub(crate) fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    pub(crate) fn object(value: Value) -> Option<JsonObject> {
        match value {
            Value::Object(object) => Some(object),
            other => panic!("not an object: {other}"),
        }
    }

    /// The fixtures the debug id reader is tested with.
    pub(crate) fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../bugsplat-debugid/tests/fixtures")
            .join(name)
    }

    pub(crate) fn text(path: &Path) -> String {
        path.to_str().unwrap().to_string()
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::json;

    use super::testing::object;
    use super::*;

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Sample {
        paths: Vec<String>,
        max_depth: Option<usize>,
    }

    #[test]
    fn arguments_are_read_by_their_camel_case_names() {
        let sample: Sample = arguments(object(json!({ "paths": ["a"], "maxDepth": 2 }))).unwrap();
        assert_eq!(sample.paths, ["a"]);
        assert_eq!(sample.max_depth, Some(2));
    }

    #[test]
    fn a_bad_argument_is_a_tool_error_that_names_it() {
        let missing = arguments::<Sample>(None).unwrap_err();
        assert!(missing.0.contains("paths"), "{missing:?}");

        let unknown = arguments::<Sample>(object(json!({ "paths": [], "path": "a" }))).unwrap_err();
        assert!(unknown.0.starts_with("Invalid arguments:"), "{unknown:?}");
        assert!(unknown.0.contains("`path`"), "{unknown:?}");

        let wrong_type = arguments::<Sample>(object(json!({ "paths": "a" }))).unwrap_err();
        assert!(wrong_type.0.contains("sequence"), "{wrong_type:?}");
    }

    #[test]
    fn an_absolute_path_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().to_str().unwrap();
        assert_eq!(resolve(raw).unwrap(), dir.path());
    }

    #[test]
    fn a_relative_path_is_under_the_servers_folder() {
        let resolved = resolve("build").unwrap();
        assert!(resolved.is_absolute());
        assert_eq!(resolved, std::env::current_dir().unwrap().join("build"));
    }

    #[test]
    fn a_tilde_is_the_home_folder() {
        let Some(home) = std::env::home_dir() else {
            return;
        };
        assert_eq!(resolve("~").unwrap(), home);
        assert_eq!(resolve("~/symbols").unwrap(), home.join("symbols"));
        // Not a home folder: a file whose name starts with a tilde.
        assert!(resolve("~backup").unwrap().ends_with("~backup"));
    }

    #[test]
    fn an_empty_path_is_refused() {
        for raw in ["", "  "] {
            assert_eq!(resolve(raw).unwrap_err().0, "A path can't be empty.");
        }
    }

    #[test]
    fn a_folder_that_cannot_be_used_says_why() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(readable_folder(dir.path()), Ok(()));

        let missing = readable_folder(&dir.path().join("nope")).unwrap_err();
        assert!(missing.0.ends_with("does not exist"), "{missing:?}");

        let file = dir.path().join("file.txt");
        std::fs::write(&file, "x").unwrap();
        let not_a_folder = readable_folder(&file).unwrap_err();
        assert!(
            not_a_folder.0.ends_with("is a file, not a folder"),
            "{not_a_folder:?}"
        );
    }

    #[tokio::test]
    async fn blocking_work_that_panics_is_a_tool_error() {
        assert_eq!(blocking(|| 2 + 2).await, Ok(4));

        let failed = blocking(|| -> usize { panic!("a malformed file") }).await;
        assert_eq!(
            failed.unwrap_err().0,
            "Reading the files failed unexpectedly."
        );
    }

    #[test]
    fn a_definition_carries_its_title_and_every_hint() {
        let read = definition(
            "a",
            "A",
            "Reads.",
            Effect::Read,
            json!({ "type": "object" }),
        );
        assert_eq!(read.title.as_deref(), Some("A"));
        let hints = read.annotations.unwrap();
        assert_eq!(hints.title.as_deref(), Some("A"));
        assert_eq!(hints.read_only_hint, Some(true));
        assert_eq!(hints.destructive_hint, Some(false));
        assert_eq!(hints.open_world_hint, Some(false));

        let write = definition("b", "B", "Writes.", Effect::Reversible, json!({}));
        let hints = write.annotations.unwrap();
        assert_eq!(hints.read_only_hint, Some(false));
        assert_eq!(hints.destructive_hint, Some(false));
        assert_eq!(hints.open_world_hint, Some(false));
    }
}

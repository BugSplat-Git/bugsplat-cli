//! Find symbol files on disk by debug id or name.
//! See https://github.com/BugSplat-Git/bugsplat-cli/issues/6

mod candidate;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use globset::{GlobBuilder, GlobMatcher};
use ignore::{DirEntry, WalkBuilder, WalkState};
use serde::Serialize;

pub use bugsplat_debugid::{DebugInfo, ObjectKind, Slice};
pub use candidate::{could_be_symbol_file, is_bundle};

/// Folders that are not searched unless `Search::no_ignore` is set. A folder named in
/// `Search::paths` is always searched.
pub const DEFAULT_IGNORED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".jj",
    "node_modules",
    "bower_components",
    "__pycache__",
    ".venv",
    ".tox",
    ".idea",
    ".vscode",
    ".vs",
];

/// What to look for and where.
#[derive(Debug, Clone, Default)]
pub struct Search {
    /// Folders, files or bundles to search. Empty means the current folder.
    pub paths: Vec<PathBuf>,
    /// Debug ids to match, in any case, with or without dashes.
    pub debug_ids: Vec<String>,
    /// File or module names to match, case-insensitively, with globs.
    pub names: Vec<String>,
    /// How far below each path to look; 1 is the path's direct children. A bundle counts
    /// as one entry however deep its contents are.
    pub max_depth: Option<usize>,
    /// Search the folders in `DEFAULT_IGNORED_DIRS` too.
    pub no_ignore: bool,
    pub follow_links: bool,
}

/// One object that was found. With no ids or names requested every object is a match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Match {
    #[serde(flatten)]
    pub info: DebugInfo,
    /// The `.dSYM`, `.app` or `.xcarchive` the object was found in.
    pub bundle: Option<PathBuf>,
    /// The requested ids this object has, as they were requested.
    pub matched_ids: Vec<String>,
    /// The requested names this object has, as they were requested.
    pub matched_names: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    /// Sorted by path, then by position in the file.
    pub matches: Vec<Match>,
    pub missing_ids: Vec<String>,
    pub missing_names: Vec<String>,
    /// Files and folders that could not be read, one `path: reason` line each.
    #[serde(skip)]
    pub warnings: Vec<String>,
}

impl SearchResult {
    /// Whether every requested id and name was found.
    pub fn is_complete(&self) -> bool {
        self.missing_ids.is_empty() && self.missing_names.is_empty()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Path {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("`{name}` is not a valid name pattern: {reason}")]
    InvalidName { name: String, reason: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Searches `search.paths` and reports the objects that have a requested id or name, or
/// every object when none is requested.
///
/// `read` is `bugsplat_debugid::read`, or a stand-in with the same contract: it is given a
/// file or a bundle and returns the objects inside. It is called from several threads, and
/// only for bundles and for files that pass [`could_be_symbol_file`].
pub fn find<R>(search: &Search, read: R) -> Result<SearchResult>
where
    R: Fn(&Path) -> bugsplat_debugid::Result<Vec<DebugInfo>> + Sync,
{
    let wanted = Wanted::new(search)?;
    let roots = roots(search)?;

    let mut builder = WalkBuilder::new(&roots[0]);
    for root in &roots[1..] {
        builder.add(root);
    }
    builder
        .standard_filters(false)
        .follow_links(search.follow_links)
        .max_depth(search.max_depth);

    let visitor = Visitor {
        wanted: &wanted,
        read: &read,
        no_ignore: search.no_ignore,
        matches: Mutex::new(Vec::new()),
        warnings: Mutex::new(Vec::new()),
    };
    builder
        .build_parallel()
        .run(|| Box::new(|entry| visitor.visit(entry)));

    let mut matches = visitor.matches.into_inner().unwrap();
    matches.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
    // Overlapping paths and followed links reach the same file more than once.
    let mut seen = HashSet::new();
    matches.retain(|found| {
        let path = std::fs::canonicalize(&found.info.path).unwrap_or(found.info.path.clone());
        seen.insert((
            path,
            found.info.slice.map(|slice| slice.offset),
            found.info.debug_id.clone(),
        ))
    });

    let mut warnings = visitor.warnings.into_inner().unwrap();
    warnings.sort();
    warnings.dedup();

    Ok(SearchResult {
        missing_ids: wanted.missing_ids(&matches),
        missing_names: wanted.missing_names(&matches),
        matches,
        warnings,
    })
}

fn sort_key(found: &Match) -> (&Path, u64, &str) {
    (
        &found.info.path,
        found.info.slice.map_or(0, |slice| slice.offset),
        &found.info.debug_id,
    )
}

fn roots(search: &Search) -> Result<Vec<PathBuf>> {
    if search.paths.is_empty() {
        return Ok(vec![PathBuf::from(".")]);
    }
    search
        .paths
        .iter()
        .map(|path| {
            let path_error = |source| Error::Path {
                path: path.clone(),
                source,
            };
            let metadata = std::fs::symlink_metadata(path).map_err(path_error)?;
            // The walker reports a link it was pointed at without entering it, and a path
            // the caller named is one they want searched.
            if metadata.file_type().is_symlink() {
                std::fs::canonicalize(path).map_err(path_error)
            } else {
                Ok(path.clone())
            }
        })
        .collect()
}

struct Visitor<'a, R> {
    wanted: &'a Wanted,
    read: &'a R,
    no_ignore: bool,
    matches: Mutex<Vec<Match>>,
    warnings: Mutex<Vec<String>>,
}

impl<R> Visitor<'_, R>
where
    R: Fn(&Path) -> bugsplat_debugid::Result<Vec<DebugInfo>> + Sync,
{
    fn visit(&self, entry: std::result::Result<DirEntry, ignore::Error>) -> WalkState {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                self.warn(err.to_string());
                return WalkState::Continue;
            }
        };
        let Some(file_type) = entry.file_type() else {
            return WalkState::Continue;
        };
        let path = entry.path();

        if file_type.is_dir() {
            if is_bundle(path) {
                self.read(path, Some(path));
                return WalkState::Skip;
            }
            if entry.depth() > 0 && !self.no_ignore && is_ignored_dir(path) {
                return WalkState::Skip;
            }
            return WalkState::Continue;
        }

        if file_type.is_file() {
            match could_be_symbol_file(path) {
                Ok(true) => self.read(path, None),
                Ok(false) => {}
                Err(err) => self.warn(format!("{}: {err}", path.display())),
            }
        }
        WalkState::Continue
    }

    fn read(&self, path: &Path, bundle: Option<&Path>) {
        let infos = match (self.read)(path) {
            Ok(infos) => infos,
            Err(bugsplat_debugid::Error::Unsupported(_)) => return,
            Err(err) => return self.warn(err.to_string()),
        };
        let found: Vec<Match> = infos
            .into_iter()
            .filter_map(|info| self.wanted.check(info, bundle))
            .collect();
        if !found.is_empty() {
            self.matches.lock().unwrap().extend(found);
        }
    }

    fn warn(&self, warning: String) {
        self.warnings.lock().unwrap().push(warning);
    }
}

fn is_ignored_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| DEFAULT_IGNORED_DIRS.contains(&name))
}

struct Wanted {
    ids: Vec<WantedId>,
    names: Vec<WantedName>,
}

struct WantedId {
    requested: String,
    normalized: String,
}

struct WantedName {
    requested: String,
    matcher: GlobMatcher,
}

impl Wanted {
    fn new(search: &Search) -> Result<Self> {
        let mut ids: Vec<WantedId> = Vec::new();
        for id in &search.debug_ids {
            let normalized = bugsplat_debugid::normalize_id(id);
            if !ids.iter().any(|known| known.normalized == normalized) {
                ids.push(WantedId {
                    requested: id.trim().to_string(),
                    normalized,
                });
            }
        }

        let mut names: Vec<WantedName> = Vec::new();
        for name in &search.names {
            if names.iter().any(|known| &known.requested == name) {
                continue;
            }
            let glob = GlobBuilder::new(name)
                .case_insensitive(true)
                .backslash_escape(false)
                .build()
                .map_err(|err| Error::InvalidName {
                    name: name.clone(),
                    reason: err.kind().to_string(),
                })?;
            names.push(WantedName {
                requested: name.clone(),
                matcher: glob.compile_matcher(),
            });
        }

        Ok(Self { ids, names })
    }

    fn lists_everything(&self) -> bool {
        self.ids.is_empty() && self.names.is_empty()
    }

    fn check(&self, info: DebugInfo, bundle: Option<&Path>) -> Option<Match> {
        let id = bugsplat_debugid::normalize_id(&info.debug_id);
        let matched_ids: Vec<String> = self
            .ids
            .iter()
            .filter(|wanted| wanted.normalized == id)
            .map(|wanted| wanted.requested.clone())
            .collect();

        let file_name = info.path.file_name().and_then(|name| name.to_str());
        let bundle_name = bundle
            .and_then(Path::file_name)
            .and_then(|name| name.to_str());
        let candidates = [Some(info.module_name.as_str()), file_name, bundle_name];
        let matched_names: Vec<String> = self
            .names
            .iter()
            .filter(|wanted| {
                candidates
                    .iter()
                    .flatten()
                    .any(|name| wanted.matcher.is_match(name))
            })
            .map(|wanted| wanted.requested.clone())
            .collect();

        if !self.lists_everything() && matched_ids.is_empty() && matched_names.is_empty() {
            return None;
        }
        Some(Match {
            info,
            bundle: bundle.map(Path::to_path_buf),
            matched_ids,
            matched_names,
        })
    }

    fn missing_ids(&self, matches: &[Match]) -> Vec<String> {
        self.ids
            .iter()
            .map(|wanted| &wanted.requested)
            .filter(|id| !matches.iter().any(|found| found.matched_ids.contains(id)))
            .cloned()
            .collect()
    }

    fn missing_names(&self, matches: &[Match]) -> Vec<String> {
        self.names
            .iter()
            .map(|wanted| &wanted.requested)
            .filter(|name| {
                !matches
                    .iter()
                    .any(|found| found.matched_names.contains(name))
            })
            .cloned()
            .collect()
    }
}

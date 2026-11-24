use std::collections::HashSet;
use std::path::{Path, PathBuf};

use bugsplat_debugid::{DebugInfo, Slice};
use serde::Serialize;

use crate::find::has_extension;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Route {
    /// Gzipped and stored by debug id through `/symsrv`.
    SymbolServer,
    /// Zipped and stored with the application version through `/api/v2/versions`.
    Versions,
    Skip,
}

/// One upload: a file, or one architecture of a fat Mach-O file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolFile {
    pub path: PathBuf,
    /// The file name BugSplat stores (`symFileName`).
    pub name: String,
    pub module_name: String,
    /// Empty when the file has none.
    pub debug_id: String,
    pub arch: Option<String>,
    pub slice: Option<Slice>,
    /// Bytes before compression.
    pub size: u64,
    pub route: Route,
    pub skip_reason: Option<String>,
}

/// Decides how each path is uploaded. `reader` is `bugsplat_debugid::read`, or a fake in tests.
///
/// A file with a debug id goes to the symbol server, one upload per architecture. Without one,
/// source maps, `.sym` files, Unity `LineNumberMappings*.json` and `.zip` files go through the
/// versions endpoint, and anything else is skipped.
pub fn plan<R>(paths: &[PathBuf], reader: R) -> Vec<SymbolFile>
where
    R: Fn(&Path) -> bugsplat_debugid::Result<Vec<DebugInfo>>,
{
    let mut files = Vec::new();
    let mut seen = HashSet::new();

    for path in paths {
        let is_zip = crate::compress::is_zip(path);
        let (infos, read_error) = if is_zip {
            (Vec::new(), None)
        } else {
            match reader(path) {
                Ok(infos) => (infos, None),
                Err(bugsplat_debugid::Error::Unsupported(_)) => (Vec::new(), None),
                Err(error) => (Vec::new(), Some(error.to_string())),
            }
        };

        if infos.iter().any(|info| !info.debug_id.is_empty()) {
            for info in infos {
                // A glob like `**/*` matches a dSYM bundle and the DWARF file inside it.
                let key = (
                    info.path.clone(),
                    info.slice.map(|slice| slice.offset),
                    info.debug_id.clone(),
                );
                if seen.insert(key) {
                    files.push(with_debug_info(info));
                }
            }
            continue;
        }

        if !seen.insert((path.clone(), None, String::new())) {
            continue;
        }
        let file_name = file_name(path);
        let legacy = path.is_file()
            && (is_zip
                || has_extension(path, "map")
                || has_extension(path, "sym")
                || is_unity_line_number_mappings(&file_name));
        let module_name = infos
            .first()
            .map(|info| info.module_name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| file_name.clone());
        files.push(SymbolFile {
            path: path.clone(),
            name: if legacy && !is_zip {
                format!("{file_name}.zip")
            } else {
                file_name
            },
            module_name,
            debug_id: String::new(),
            arch: None,
            slice: None,
            size: file_size(path),
            route: if legacy { Route::Versions } else { Route::Skip },
            skip_reason: (!legacy)
                .then(|| read_error.unwrap_or_else(|| "it has no debug id".to_string())),
        });
    }

    files
}

fn with_debug_info(info: DebugInfo) -> SymbolFile {
    let has_id = !info.debug_id.is_empty();
    SymbolFile {
        name: file_name(&info.path),
        size: info
            .slice
            .map_or_else(|| file_size(&info.path), |slice| slice.size),
        route: if has_id {
            Route::SymbolServer
        } else {
            Route::Skip
        },
        skip_reason: (!has_id).then(|| "it has no debug id".to_string()),
        path: info.path,
        module_name: info.module_name,
        debug_id: info.debug_id,
        arch: info.arch,
        slice: info.slice,
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file())
        .map_or(0, |metadata| metadata.len())
}

/// `LineNumberMappings.json` or `LineNumberMappings-<word>.json`, as Unity IL2CPP writes them.
fn is_unity_line_number_mappings(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    let Some(middle) = lower
        .strip_prefix("linenumbermappings")
        .and_then(|rest| rest.strip_suffix(".json"))
    else {
        return false;
    };
    match middle.strip_prefix('-') {
        Some(word) => {
            !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => middle.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use bugsplat_debugid::{Error, ObjectKind};

    use super::*;

    fn unsupported(path: &Path) -> bugsplat_debugid::Result<Vec<DebugInfo>> {
        Err(Error::Unsupported(path.to_path_buf()))
    }

    fn info(path: &Path, module_name: &str, debug_id: &str) -> DebugInfo {
        DebugInfo {
            path: path.to_path_buf(),
            module_name: module_name.to_string(),
            debug_id: debug_id.to_string(),
            kind: ObjectKind::Pdb,
            arch: None,
            slice: None,
        }
    }

    fn write(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn a_file_with_a_debug_id_goes_to_the_symbol_server() {
        let dir = tempfile::tempdir().unwrap();
        let pdb = write(dir.path(), "app.pdb", b"12345");
        let files = plan(std::slice::from_ref(&pdb), |path| {
            Ok(vec![info(path, "app.pdb", "ABC1")])
        });
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].route, Route::SymbolServer);
        assert_eq!(files[0].name, "app.pdb");
        assert_eq!(files[0].module_name, "app.pdb");
        assert_eq!(files[0].debug_id, "ABC1");
        assert_eq!(files[0].size, 5);
    }

    #[test]
    fn files_the_reader_rejects_use_the_versions_endpoint_when_they_are_a_legacy_kind() {
        let dir = tempfile::tempdir().unwrap();
        let paths = [
            write(dir.path(), "main.js.map", b"{}"),
            write(dir.path(), "UnrealGame.sym", b"MODULE"),
            write(dir.path(), "LineNumberMappings.json", b"{}"),
            write(dir.path(), "LineNumberMappings-arm64.json", b"{}"),
            write(dir.path(), "symbols.zip", b"PK"),
        ];
        let files = plan(&paths, unsupported);
        let summary: Vec<_> = files
            .iter()
            .map(|file| (file.name.as_str(), file.route))
            .collect();
        assert_eq!(
            summary,
            [
                ("main.js.map.zip", Route::Versions),
                ("UnrealGame.sym.zip", Route::Versions),
                ("LineNumberMappings.json.zip", Route::Versions),
                ("LineNumberMappings-arm64.json.zip", Route::Versions),
                ("symbols.zip", Route::Versions),
            ]
        );
        assert_eq!(files[0].module_name, "main.js.map");
        assert!(files.iter().all(|file| file.debug_id.is_empty()));
    }

    #[test]
    fn anything_else_without_a_debug_id_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let paths = [
            write(dir.path(), "readme.txt", b"hi"),
            write(dir.path(), "LineNumberMappings-.json", b"{}"),
            write(dir.path(), "LineNumberMappingsX.json", b"{}"),
            write(dir.path(), "stripped.dll", b"MZ"),
        ];
        let files = plan(&paths, |path| {
            if path.ends_with("stripped.dll") {
                Ok(vec![info(path, "stripped.dll", "")])
            } else {
                unsupported(path)
            }
        });
        assert_eq!(files.len(), 4);
        for file in &files {
            assert_eq!(file.route, Route::Skip, "{}", file.name);
            assert_eq!(file.skip_reason.as_deref(), Some("it has no debug id"));
        }
    }

    #[test]
    fn a_reader_failure_is_the_skip_reason() {
        let dir = tempfile::tempdir().unwrap();
        let pdb = write(dir.path(), "broken.pdb", b"x");
        let files = plan(std::slice::from_ref(&pdb), |path| {
            Err(Error::Malformed {
                path: path.to_path_buf(),
                reason: "truncated header".to_string(),
            })
        });
        assert_eq!(files[0].route, Route::Skip);
        assert!(
            files[0]
                .skip_reason
                .as_deref()
                .unwrap()
                .contains("truncated header")
        );
    }

    #[test]
    fn a_sym_file_with_a_debug_id_goes_to_the_symbol_server_under_its_module_name() {
        let dir = tempfile::tempdir().unwrap();
        let sym = write(dir.path(), "app.sym", b"MODULE windows x86_64 ABC app.pdb");
        let files = plan(std::slice::from_ref(&sym), |path| {
            Ok(vec![info(path, "app.pdb", "ABC1")])
        });
        assert_eq!(files[0].route, Route::SymbolServer);
        assert_eq!(files[0].name, "app.sym");
        assert_eq!(files[0].module_name, "app.pdb");
    }

    #[test]
    fn a_zip_never_asks_the_reader() {
        let dir = tempfile::tempdir().unwrap();
        let zip = write(dir.path(), "symbols.ZIP", b"PK");
        let files = plan(std::slice::from_ref(&zip), |_| {
            panic!("the reader was called for a zip")
        });
        assert_eq!(files[0].route, Route::Versions);
        assert_eq!(files[0].name, "symbols.ZIP");
    }

    #[test]
    fn each_slice_of_a_fat_file_is_its_own_upload_and_duplicates_collapse() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("App.app.dSYM");
        let dwarf_dir = bundle.join("Contents/Resources/DWARF");
        fs::create_dir_all(&dwarf_dir).unwrap();
        let dwarf = write(&dwarf_dir, "App", &[0u8; 64]);

        let slices = |_: &Path| {
            Ok(vec![
                DebugInfo {
                    arch: Some("x86_64".to_string()),
                    slice: Some(Slice {
                        offset: 8,
                        size: 16,
                    }),
                    kind: ObjectKind::MachO,
                    ..info(&dwarf, "App", "AAAA")
                },
                DebugInfo {
                    arch: Some("arm64".to_string()),
                    slice: Some(Slice {
                        offset: 32,
                        size: 24,
                    }),
                    kind: ObjectKind::MachO,
                    ..info(&dwarf, "App", "BBBB")
                },
            ])
        };
        let files = plan(&[bundle, dwarf.clone()], slices);
        let summary: Vec<_> = files
            .iter()
            .map(|file| (file.debug_id.as_str(), file.size, file.name.as_str()))
            .collect();
        assert_eq!(summary, [("AAAA", 16, "App"), ("BBBB", 24, "App")]);
        assert!(files.iter().all(|file| file.path == dwarf));
    }

    #[test]
    fn a_bundle_the_reader_rejects_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Empty.dSYM");
        fs::create_dir_all(&bundle).unwrap();
        let files = plan(&[bundle], unsupported);
        assert_eq!(files[0].route, Route::Skip);
    }
}

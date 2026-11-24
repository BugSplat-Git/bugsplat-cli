use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use bugsplat_core::Result;
use serde::Serialize;

use crate::compress;
use crate::plan::{Route, SymbolFile};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalCopy {
    pub local_path: PathBuf,
    pub copied: usize,
    pub skipped: usize,
    pub bytes: u64,
    pub files: Vec<LocalFile>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalFile {
    #[serde(flatten)]
    pub file: SymbolFile,
    pub destination: Option<PathBuf>,
}

/// Copies files into a symbol-server layout, `<local_path>/<module>/<debug id>/<file>`,
/// instead of uploading them. Files without a debug id have no place in that layout and are
/// skipped.
pub fn copy_to_local(files: &[SymbolFile], local_path: &Path) -> Result<LocalCopy> {
    let mut copy = LocalCopy {
        local_path: local_path.to_path_buf(),
        copied: 0,
        skipped: 0,
        bytes: 0,
        files: Vec::new(),
    };

    for file in files {
        let skip_reason = if file.route != Route::SymbolServer {
            Some(
                file.skip_reason
                    .clone()
                    .unwrap_or_else(|| "it has no debug id".to_string()),
            )
        } else if [&file.module_name, &file.debug_id, &file.name]
            .iter()
            .any(|part| !is_plain_name(part))
        {
            // Module names come from inside the file; one with a separator or `..` would
            // write outside the symbol folder.
            Some("its module name or debug id is not a plain name".to_string())
        } else {
            None
        };

        if let Some(reason) = skip_reason {
            copy.skipped += 1;
            copy.files.push(LocalFile {
                file: SymbolFile {
                    route: Route::Skip,
                    skip_reason: Some(reason),
                    ..file.clone()
                },
                destination: None,
            });
            continue;
        }

        let folder = local_path.join(&file.module_name).join(&file.debug_id);
        fs::create_dir_all(&folder)?;
        let destination = folder.join(&file.name);
        // Through a temporary file: when the search includes the store itself, a file is
        // its own destination, and creating it in place would empty it before it is read.
        let mut reader = compress::open(&file.path, file.slice)?;
        let mut staged = tempfile::NamedTempFile::new_in(&folder)?;
        copy.bytes += io::copy(&mut reader, &mut staged)?;
        drop(reader);
        staged.persist(&destination).map_err(|error| error.error)?;
        copy.copied += 1;
        copy.files.push(LocalFile {
            file: file.clone(),
            destination: Some(destination),
        });
    }

    // Marks the folder as a symbol store for symsrv clients.
    fs::create_dir_all(local_path)?;
    fs::write(local_path.join("index.txt"), ".")?;
    Ok(copy)
}

// One ordinary path component. On Windows that also rules out a drive-relative name
// such as `C:outside.so`, which would leave the store when joined to it.
fn is_plain_name(name: &str) -> bool {
    !name.contains(['/', '\\'])
        && matches!(
            Path::new(name).components().collect::<Vec<_>>().as_slice(),
            [Component::Normal(_)]
        )
}

#[cfg(test)]
mod tests {
    use bugsplat_debugid::Slice;

    use super::*;

    fn file(path: &Path, module_name: &str, debug_id: &str, route: Route) -> SymbolFile {
        SymbolFile {
            path: path.to_path_buf(),
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            module_name: module_name.to_string(),
            debug_id: debug_id.to_string(),
            arch: None,
            slice: None,
            size: 0,
            route,
            skip_reason: None,
        }
    }

    #[test]
    fn copies_into_module_and_debug_id_folders_and_marks_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let pdb = dir.path().join("app.pdb");
        fs::write(&pdb, b"pdb bytes").unwrap();
        let map = dir.path().join("main.js.map");
        fs::write(&map, b"{}").unwrap();
        let store = dir.path().join("store");

        let copy = copy_to_local(
            &[
                file(&pdb, "app.pdb", "ABC1", Route::SymbolServer),
                file(&map, "main.js.map", "", Route::Versions),
            ],
            &store,
        )
        .unwrap();

        assert_eq!((copy.copied, copy.skipped, copy.bytes), (1, 1, 9));
        assert_eq!(
            fs::read(store.join("app.pdb/ABC1/app.pdb")).unwrap(),
            b"pdb bytes"
        );
        assert_eq!(fs::read_to_string(store.join("index.txt")).unwrap(), ".");
        assert_eq!(copy.files[1].destination, None);
        assert_eq!(copy.files[1].file.route, Route::Skip);
    }

    #[test]
    fn a_file_already_in_the_store_keeps_its_contents() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("store");
        let folder = store.join("app.pdb").join("ABC1");
        fs::create_dir_all(&folder).unwrap();
        let pdb = folder.join("app.pdb");
        fs::write(&pdb, b"pdb bytes").unwrap();

        let copy = copy_to_local(
            &[file(&pdb, "app.pdb", "ABC1", Route::SymbolServer)],
            &store,
        )
        .unwrap();

        assert_eq!((copy.copied, copy.bytes), (1, 9));
        assert_eq!(fs::read(&pdb).unwrap(), b"pdb bytes");
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
    }

    #[test]
    fn only_a_single_ordinary_component_is_a_plain_name() {
        for name in ["app.pdb", "Electron Helper (GPU)", "libc++_shared.so"] {
            assert!(is_plain_name(name), "{name}");
        }
        for name in ["", ".", "..", "a/b", "a\\b", "/abs", "../up"] {
            assert!(!is_plain_name(name), "{name}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_relative_name_is_not_a_plain_name() {
        assert!(!is_plain_name("C:outside.so"));
    }

    #[test]
    fn a_slice_is_copied_alone() {
        let dir = tempfile::tempdir().unwrap();
        let fat = dir.path().join("App");
        fs::write(&fat, b"0123456789abcdef").unwrap();
        let store = dir.path().join("store");
        let slice = SymbolFile {
            slice: Some(Slice {
                offset: 10,
                size: 6,
            }),
            ..file(&fat, "App", "BBBB", Route::SymbolServer)
        };
        copy_to_local(&[slice], &store).unwrap();
        assert_eq!(fs::read(store.join("App/BBBB/App")).unwrap(), b"abcdef");
    }

    #[test]
    fn a_module_name_that_would_leave_the_store_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let sym = dir.path().join("evil.sym");
        fs::write(&sym, b"MODULE").unwrap();
        let store = dir.path().join("store");
        let copy = copy_to_local(
            &[
                file(&sym, "../../escape", "ABC1", Route::SymbolServer),
                file(&sym, "..", "ABC1", Route::SymbolServer),
            ],
            &store,
        )
        .unwrap();
        assert_eq!((copy.copied, copy.skipped), (0, 2));
        assert!(!dir.path().join("escape").exists());
    }
}

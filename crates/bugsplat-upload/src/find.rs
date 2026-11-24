use std::path::{Path, PathBuf};

use bugsplat_core::{Error, Result};
use globset::GlobBuilder;
use walkdir::WalkDir;

/// The files under `directory` that `pattern` selects, sorted. Folders are left out, except
/// `.dSYM` bundles, which upload as one unit.
///
/// The glob follows `symbol-upload`'s rules: `*` and `?` stay inside one path component, `**`
/// crosses them, `{a,b}` alternates, and names starting with a dot only match a pattern
/// component that starts with a dot.
pub fn find_files(directory: &Path, pattern: &str) -> Result<Vec<PathBuf>> {
    let mut components: Vec<&str> = pattern
        .split('/')
        .filter(|component| !component.is_empty() && *component != ".")
        .collect();
    if components.is_empty() {
        return Err(Error::Usage("The --files glob is empty.".to_string()));
    }

    // Leading components without wildcards are a path, not a pattern; starting the walk there
    // keeps `build/Release/*.pdb` from walking the whole tree and lets `../out/*.pdb` work.
    let mut root = directory.to_path_buf();
    while components.len() > 1 && !has_wildcard(components[0]) {
        root.push(components.remove(0));
    }

    let glob = GlobBuilder::new(&components.join("/"))
        .literal_separator(true)
        .case_insensitive(cfg!(any(target_os = "macos", windows)))
        .build()
        .map_err(|e| Error::Usage(format!("Invalid --files glob `{pattern}`: {e}")))?
        .compile_matcher();

    let recursive = components.iter().any(|component| component.contains("**"));
    let max_depth = if recursive {
        usize::MAX
    } else {
        components.len()
    };
    let match_hidden = components
        .iter()
        .any(|component| component.starts_with('.'));

    let mut found = Vec::new();
    let walk = WalkDir::new(&root)
        .min_depth(1)
        .max_depth(max_depth)
        .into_iter()
        .filter_entry(|entry| match_hidden || !is_hidden(entry.file_name()));
    for entry in walk.filter_map(|entry| entry.ok()) {
        let Ok(relative) = entry.path().strip_prefix(&root) else {
            continue;
        };
        let relative = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if !glob.is_match(&relative) {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(entry.path()) else {
            continue;
        };
        if metadata.is_file() || is_dsym_bundle(entry.path()) {
            found.push(entry.into_path());
        }
    }
    found.sort();
    Ok(found)
}

pub(crate) fn is_dsym_bundle(path: &Path) -> bool {
    path.is_dir() && has_extension(path, "dsym")
}

pub(crate) fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
}

fn has_wildcard(component: &str) -> bool {
    component.contains(['*', '?', '[', '{'])
}

fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for file in [
            "main.js.map",
            "notes.txt",
            ".hidden.js.map",
            "build/app.pdb",
            "build/app.exe",
            "build/readme.md",
            "build/deep/lib.dll",
            "build/.cache/tool.pdb",
            "mac/App.app.dSYM/Contents/Info.plist",
            "mac/App.app.dSYM/Contents/Resources/DWARF/App",
            "mac/folder.pdb/inner.txt",
        ] {
            let path = root.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"x").unwrap();
        }
        dir
    }

    fn names(root: &Path, pattern: &str) -> Vec<String> {
        find_files(root, pattern)
            .unwrap()
            .iter()
            .map(|path| {
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    #[test]
    fn the_default_glob_stays_in_the_directory() {
        let dir = tree();
        assert_eq!(names(dir.path(), "*.js.map"), ["main.js.map"]);
    }

    #[test]
    fn braces_and_double_star_search_recursively() {
        let dir = tree();
        assert_eq!(
            names(dir.path(), "**/*.{pdb,exe,dll}"),
            ["build/app.exe", "build/app.pdb", "build/deep/lib.dll"]
        );
    }

    #[test]
    fn a_literal_prefix_narrows_the_search() {
        let dir = tree();
        assert_eq!(
            names(dir.path(), "build/*.pdb"),
            ["build/app.pdb"],
            "one star does not cross into build/deep"
        );
        assert_eq!(names(dir.path(), "./build/app.exe"), ["build/app.exe"]);
    }

    #[test]
    fn folders_are_left_out_except_dsym_bundles() {
        let dir = tree();
        assert_eq!(names(dir.path(), "mac/*"), ["mac/App.app.dSYM"]);
        assert_eq!(names(dir.path(), "**/*.dSYM"), ["mac/App.app.dSYM"]);
    }

    #[test]
    fn hidden_names_need_a_pattern_that_asks_for_them() {
        let dir = tree();
        assert_eq!(names(dir.path(), "**/*.pdb"), ["build/app.pdb"]);
        assert_eq!(names(dir.path(), ".*.js.map"), [".hidden.js.map"]);
    }

    #[test]
    fn nothing_found_is_an_empty_list_and_a_bad_glob_is_a_usage_error() {
        let dir = tree();
        assert!(names(dir.path(), "*.nope").is_empty());
        assert!(
            find_files(&dir.path().join("missing"), "*.pdb")
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            find_files(dir.path(), "{a,b"),
            Err(Error::Usage(_))
        ));
    }
}

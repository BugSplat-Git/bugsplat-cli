use std::path::{Path, PathBuf};

use bugsplat_core::{Error, Result};
use globset::{GlobBuilder, GlobMatcher};
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

    let alternatives = expand_braces(&components.join("/"))
        .iter()
        .map(|alternative| compile(alternative, pattern))
        .collect::<Result<Vec<_>>>()?;

    let recursive = alternatives
        .iter()
        .flatten()
        .any(|part| matches!(part, Part::AnyFolders));
    let max_depth = if recursive {
        usize::MAX
    } else {
        alternatives.iter().map(Vec::len).max().unwrap_or(0)
    };
    // Only to skip hidden folders early; `matches` decides each name.
    let any_hidden = alternatives
        .iter()
        .flatten()
        .any(|part| matches!(part, Part::Name { hidden: true, .. }));

    let mut found = Vec::new();
    let walk = WalkDir::new(&root)
        .min_depth(1)
        .max_depth(max_depth)
        .into_iter()
        .filter_entry(|entry| any_hidden || !is_hidden(entry.file_name()));
    for entry in walk.filter_map(|entry| entry.ok()) {
        let Ok(relative) = entry.path().strip_prefix(&root) else {
            continue;
        };
        let names: Vec<String> = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect();
        if !alternatives.iter().any(|parts| matches(parts, &names)) {
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

/// One component of a pattern, matched against one component of a path.
enum Part {
    /// `**`: any number of folders, none of them hidden.
    AnyFolders,
    Name {
        matcher: GlobMatcher,
        hidden: bool,
    },
}

fn compile(alternative: &str, pattern: &str) -> Result<Vec<Part>> {
    alternative
        .split('/')
        .filter(|component| !component.is_empty() && *component != ".")
        .map(|component| {
            if component == "**" {
                return Ok(Part::AnyFolders);
            }
            let matcher = GlobBuilder::new(component)
                .literal_separator(true)
                .case_insensitive(cfg!(any(target_os = "macos", windows)))
                .build()
                .map_err(|e| Error::Usage(format!("Invalid --files glob `{pattern}`: {e}")))?
                .compile_matcher();
            Ok(Part::Name {
                matcher,
                hidden: component.starts_with('.'),
            })
        })
        .collect()
}

/// A name that starts with a dot matches only a pattern component that starts with one,
/// component by component: `**/.symbols/*.pdb` takes `.symbols/app.pdb` and leaves
/// `.symbols/.private.pdb` and `.git/.symbols/app.pdb`.
fn matches(parts: &[Part], names: &[String]) -> bool {
    match parts.split_first() {
        None => names.is_empty(),
        Some((Part::AnyFolders, rest)) => (0..=names.len())
            .take_while(|&taken| taken == 0 || !names[taken - 1].starts_with('.'))
            .any(|taken| matches(rest, &names[taken..])),
        Some((Part::Name { matcher, hidden }, rest)) => match names.split_first() {
            Some((name, tail)) => {
                (*hidden || !name.starts_with('.')) && matcher.is_match(name) && matches(rest, tail)
            }
            None => false,
        },
    }
}

/// `a/{b,c/d}` as `a/b` and `a/c/d`, so each alternative splits into components. A brace
/// without its partner is left for the glob compiler to reject.
fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    let mut depth = 0;
    let mut options = Vec::new();
    let mut option_start = open + 1;
    for (index, character) in pattern[open..].char_indices().map(|(i, c)| (open + i, c)) {
        match character {
            '{' => depth += 1,
            ',' if depth == 1 => {
                options.push(&pattern[option_start..index]);
                option_start = index + 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    options.push(&pattern[option_start..index]);
                    let (prefix, suffix) = (&pattern[..open], &pattern[index + 1..]);
                    return options
                        .iter()
                        .flat_map(|option| expand_braces(&format!("{prefix}{option}{suffix}")))
                        .collect();
                }
            }
            _ => {}
        }
    }
    vec![pattern.to_string()]
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
    fn a_hidden_name_is_matched_component_by_component() {
        let dir = tempfile::tempdir().unwrap();
        for file in [
            ".symbols/app.pdb",
            ".symbols/.private.pdb",
            ".git/.symbols/app.pdb",
            "out/.symbols/app.pdb",
        ] {
            let path = dir.path().join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"x").unwrap();
        }
        assert_eq!(
            names(dir.path(), "**/.symbols/*.pdb"),
            [".symbols/app.pdb", "out/.symbols/app.pdb"]
        );
        assert_eq!(
            names(dir.path(), "**/.symbols/.*.pdb"),
            [".symbols/.private.pdb"]
        );
    }

    #[test]
    fn braces_can_hold_folders() {
        let dir = tree();
        assert_eq!(
            names(dir.path(), "{build/deep,mac}/*.{dll,dSYM}"),
            ["build/deep/lib.dll", "mac/App.app.dSYM"]
        );
        assert_eq!(
            expand_braces("a/{b,c/{d,e}}/f"),
            ["a/b/f", "a/c/d/f", "a/c/e/f"]
        );
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

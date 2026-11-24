//! `find` over temporary directory trees, with a stand-in for `bugsplat_debugid::read`.
//!
//! A fake symbol file is a real signature, a newline, then one `id [module [arch]]` line per
//! object in the file. The fake reader parses that, and reads a bundle by parsing every file
//! inside it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use bugsplat_debugid::Error as ReadError;
use bugsplat_symbols::{DebugInfo, Error, ObjectKind, Search, SearchResult, Slice, find};
use serde_json::json;
use tempfile::TempDir;

const PE: &[u8] = b"MZ";
const PDB: &[u8] = b"Microsoft C/C++ MSF 7.00";
const ELF: &[u8] = b"\x7fELF";
const MACHO: &[u8] = &[0xcf, 0xfa, 0xed, 0xfe];
const FAT: &[u8] = &[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2];
const SYM: &[u8] = b"MODULE ";

const PDB_ID: &str = "E546B55B6D214E86871B40AC35CD0D461";
const ELF_ID: &str = "005F5F676D6F6E5F73746172745F5F006C6962632E736F2E36";
const ARM_ID: &str = "59686D50E0A93E3EB5C3C5D7B6F2A4A1";
const X64_ID: &str = "2DE6A5B1F0C84F0F9A49C29A2D5B0B77";

struct Tree {
    dir: TempDir,
}

impl Tree {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root().join(relative)
    }

    fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    fn symbol(&self, relative: &str, magic: &[u8], objects: &str) -> PathBuf {
        let mut bytes = magic.to_vec();
        bytes.push(b'\n');
        bytes.extend_from_slice(objects.as_bytes());
        self.write(relative, &bytes)
    }

    fn search(&self) -> Search {
        Search {
            paths: vec![self.root().to_path_buf()],
            ..Search::default()
        }
    }

    fn relative(&self, path: &Path) -> String {
        path.strip_prefix(self.root())
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn found(&self, result: &SearchResult) -> Vec<String> {
        result
            .matches
            .iter()
            .map(|found| self.relative(&found.info.path))
            .collect()
    }
}

#[derive(Default)]
struct FakeReader {
    calls: Mutex<Vec<PathBuf>>,
}

impl FakeReader {
    fn read(&self, path: &Path) -> bugsplat_debugid::Result<Vec<DebugInfo>> {
        self.calls.lock().unwrap().push(path.to_path_buf());
        if path.is_dir() {
            let mut files = Vec::new();
            files_under(path, &mut files);
            files.sort();
            let mut infos = Vec::new();
            for file in files {
                infos.extend(parse(&file)?);
            }
            return Ok(infos);
        }
        let infos = parse(path)?;
        if infos.is_empty() {
            return Err(ReadError::Unsupported(path.to_path_buf()));
        }
        Ok(infos)
    }

    fn calls(&self, tree: &Tree) -> Vec<String> {
        let mut calls: Vec<String> = self
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|path| tree.relative(path))
            .collect();
        calls.sort();
        calls
    }
}

fn files_under(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_under(&path, files);
        } else {
            files.push(path);
        }
    }
}

fn parse(path: &Path) -> bugsplat_debugid::Result<Vec<DebugInfo>> {
    let bytes = fs::read(path).map_err(|source| ReadError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let kinds: &[(&[u8], ObjectKind)] = &[
        (PE, ObjectKind::Pe),
        (PDB, ObjectKind::Pdb),
        (ELF, ObjectKind::Elf),
        (MACHO, ObjectKind::MachO),
        (FAT, ObjectKind::MachO),
        (SYM, ObjectKind::BreakpadSym),
    ];
    let Some((magic, kind)) = kinds.iter().find(|(magic, _)| bytes.starts_with(magic)) else {
        return Ok(Vec::new());
    };
    let text = String::from_utf8_lossy(&bytes[magic.len()..]).into_owned();
    let lines: Vec<&str> = text.lines().filter(|line| !line.is_empty()).collect();
    if lines == ["MALFORMED"] {
        return Err(ReadError::Malformed {
            path: path.to_path_buf(),
            reason: "truncated header".to_string(),
        });
    }
    let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
    Ok(lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let mut fields = line.split(' ');
            let debug_id = fields.next().unwrap().replace('-', "");
            let debug_id = if debug_id == "NOID" {
                String::new()
            } else {
                debug_id
            };
            let module_name = fields.next().unwrap_or(&file_name).to_string();
            let arch = fields.next().map(str::to_string);
            DebugInfo {
                path: path.to_path_buf(),
                module_name,
                debug_id,
                kind: *kind,
                slice: arch.as_ref().map(|_| Slice {
                    offset: 4096 * (index as u64 + 1),
                    size: 4096,
                }),
                arch,
            }
        })
        .collect())
}

fn run(search: &Search) -> (SearchResult, FakeReader) {
    let reader = FakeReader::default();
    let result = find(search, |path| reader.read(path)).unwrap();
    (result, reader)
}

fn build_tree() -> Tree {
    let tree = Tree::new();
    tree.symbol("build/myapp.pdb", PDB, PDB_ID);
    tree.symbol("build/myapp.exe", PE, PDB_ID);
    tree.symbol("build/linux/myapp", ELF, ELF_ID);
    tree.symbol("build/linux/libfoo.so.1.2", ELF, "AAAA");
    tree.symbol("build/mac/myapp", MACHO, &format!("{ARM_ID} myapp arm64"));
    tree.symbol(
        "build/sym/myapp.sym",
        SYM,
        "E546B55B6D214E86871B40AC35CD0D461 myapp.pdb",
    );
    tree.write("src/main.c", b"int main() { return 0; }\n");
    tree.write("README.md", b"# My app\n");
    tree.write("LICENSE", b"MIT\n");
    tree
}

#[test]
fn lists_every_symbol_file_when_nothing_is_requested() {
    let tree = build_tree();
    let (result, reader) = run(&tree.search());

    let expected = [
        "build/linux/libfoo.so.1.2",
        "build/linux/myapp",
        "build/mac/myapp",
        "build/myapp.exe",
        "build/myapp.pdb",
        "build/sym/myapp.sym",
    ];
    assert_eq!(tree.found(&result), expected);
    assert_eq!(reader.calls(&tree), expected);
    assert!(result.matches.iter().all(|found| {
        found.matched_ids.is_empty() && found.matched_names.is_empty() && found.bundle.is_none()
    }));
    assert!(result.is_complete());
    assert!(result.warnings.is_empty());

    let pdb = &result.matches[4];
    assert_eq!(pdb.info.debug_id, PDB_ID);
    assert_eq!(pdb.info.module_name, "myapp.pdb");
    assert_eq!(pdb.info.kind, ObjectKind::Pdb);
}

#[test]
fn matches_debug_ids_in_any_case_and_with_dashes() {
    let tree = build_tree();
    let requested = "e546b55b-6d21-4e86-871b-40ac35cd0d46-1";
    let search = Search {
        debug_ids: vec![requested.to_string()],
        ..tree.search()
    };
    let (result, _) = run(&search);

    assert_eq!(
        tree.found(&result),
        ["build/myapp.exe", "build/myapp.pdb", "build/sym/myapp.sym"]
    );
    for found in &result.matches {
        assert_eq!(found.matched_ids, [requested]);
        assert!(found.matched_names.is_empty());
    }
    assert!(result.missing_ids.is_empty());
    assert!(result.is_complete());
}

#[test]
fn reports_the_ids_that_were_not_found() {
    let tree = build_tree();
    let search = Search {
        debug_ids: vec![
            ELF_ID.to_lowercase(),
            "00000000000000000000000000000000".to_string(),
            format!(" {ELF_ID} "),
            "FFFF".to_string(),
        ],
        ..tree.search()
    };
    let (result, _) = run(&search);

    assert_eq!(tree.found(&result), ["build/linux/myapp"]);
    assert_eq!(result.matches[0].matched_ids, [ELF_ID.to_lowercase()]);
    assert_eq!(
        result.missing_ids,
        ["00000000000000000000000000000000", "FFFF"]
    );
    assert!(!result.is_complete());
}

#[test]
fn a_file_without_an_id_is_listed_but_matches_no_id() {
    let tree = Tree::new();
    tree.symbol("stripped", ELF, "NOID");

    let (listed, _) = run(&tree.search());
    assert_eq!(tree.found(&listed), ["stripped"]);
    assert_eq!(listed.matches[0].info.debug_id, "");

    let search = Search {
        debug_ids: vec!["AAAA".to_string()],
        ..tree.search()
    };
    let (result, _) = run(&search);
    assert!(result.matches.is_empty());
    assert_eq!(result.missing_ids, ["AAAA"]);
}

#[test]
fn matches_file_names_case_insensitively_and_with_globs() {
    let tree = build_tree();
    let names = |names: &[&str]| Search {
        names: names.iter().map(|name| name.to_string()).collect(),
        ..tree.search()
    };

    let (exact, _) = run(&names(&["MyApp.PDB"]));
    assert_eq!(
        tree.found(&exact),
        ["build/myapp.pdb", "build/sym/myapp.sym"]
    );
    assert_eq!(exact.matches[0].matched_names, ["MyApp.PDB"]);

    let (star, _) = run(&names(&["*.EXE"]));
    assert_eq!(tree.found(&star), ["build/myapp.exe"]);

    let (question, _) = run(&names(&["myapp.???"]));
    assert_eq!(
        tree.found(&question),
        ["build/myapp.exe", "build/myapp.pdb", "build/sym/myapp.sym"]
    );

    let (class, _) = run(&names(&["libfoo.so.[0-9]*"]));
    assert_eq!(tree.found(&class), ["build/linux/libfoo.so.1.2"]);

    let (whole_name_only, _) = run(&names(&["myapp.p"]));
    assert!(whole_name_only.matches.is_empty());
}

#[test]
fn matches_the_module_name_when_it_differs_from_the_file_name() {
    let tree = build_tree();
    let search = Search {
        names: vec!["myapp.pdb".to_string()],
        ..tree.search()
    };
    let (result, _) = run(&search);

    let sym = result
        .matches
        .iter()
        .find(|found| found.info.kind == ObjectKind::BreakpadSym)
        .expect("the .sym whose module is myapp.pdb");
    assert_eq!(tree.relative(&sym.info.path), "build/sym/myapp.sym");
    assert_eq!(sym.info.module_name, "myapp.pdb");
}

#[test]
fn a_file_matching_an_id_or_a_name_is_reported_with_what_it_matched() {
    let tree = build_tree();
    let search = Search {
        debug_ids: vec![PDB_ID.to_string()],
        names: vec![
            "*.exe".to_string(),
            "libfoo*".to_string(),
            "nothing.dll".to_string(),
        ],
        ..tree.search()
    };
    let (result, _) = run(&search);

    let matched: Vec<(String, &[String], &[String])> = result
        .matches
        .iter()
        .map(|found| {
            (
                tree.relative(&found.info.path),
                found.matched_ids.as_slice(),
                found.matched_names.as_slice(),
            )
        })
        .collect();
    let id = [PDB_ID.to_string()];
    let exe = ["*.exe".to_string()];
    let libfoo = ["libfoo*".to_string()];
    assert_eq!(
        matched,
        [
            (
                "build/linux/libfoo.so.1.2".to_string(),
                &[][..],
                &libfoo[..]
            ),
            ("build/myapp.exe".to_string(), &id[..], &exe[..]),
            ("build/myapp.pdb".to_string(), &id[..], &[][..]),
            ("build/sym/myapp.sym".to_string(), &id[..], &[][..]),
        ]
    );
    assert!(result.missing_ids.is_empty());
    assert_eq!(result.missing_names, ["nothing.dll"]);
    assert!(!result.is_complete());
}

#[test]
fn rejects_a_name_that_is_not_a_valid_glob() {
    let tree = build_tree();
    let search = Search {
        names: vec!["[".to_string()],
        ..tree.search()
    };
    let err = find(&search, |path| FakeReader::default().read(path)).unwrap_err();

    assert!(matches!(err, Error::InvalidName { ref name, .. } if name == "["));
    assert!(err.to_string().contains("`[` is not a valid name pattern"));
}

#[test]
fn skips_version_control_and_dependency_folders_by_default() {
    let tree = Tree::new();
    tree.symbol("app.exe", PE, "AAAA");
    for folder in [
        ".git",
        ".hg",
        ".svn",
        "node_modules",
        "__pycache__",
        ".venv",
    ] {
        tree.symbol(&format!("{folder}/hidden.exe"), PE, "BBBB");
        tree.symbol(&format!("src/{folder}/deep/hidden.dll"), PE, "BBBB");
    }

    let (result, reader) = run(&tree.search());

    assert_eq!(tree.found(&result), ["app.exe"]);
    assert_eq!(reader.calls(&tree), ["app.exe"]);
}

#[test]
fn no_ignore_searches_the_folders_skipped_by_default() {
    let tree = Tree::new();
    tree.symbol("app.exe", PE, "AAAA");
    tree.symbol("node_modules/addon/addon.node", ELF, "BBBB");
    tree.symbol(".git/modules/dep/tool.exe", PE, "CCCC");

    let search = Search {
        no_ignore: true,
        ..tree.search()
    };
    let (result, _) = run(&search);

    assert_eq!(
        tree.found(&result),
        [
            ".git/modules/dep/tool.exe",
            "app.exe",
            "node_modules/addon/addon.node"
        ]
    );
}

#[test]
fn a_skipped_folder_is_searched_when_it_is_the_path() {
    let tree = Tree::new();
    tree.symbol("node_modules/addon/addon.node", ELF, "BBBB");
    tree.symbol("node_modules/addon/node_modules/dep/dep.node", ELF, "CCCC");

    let search = Search {
        paths: vec![tree.path("node_modules")],
        ..Search::default()
    };
    let (result, _) = run(&search);

    assert_eq!(tree.found(&result), ["node_modules/addon/addon.node"]);
}

#[test]
fn hidden_and_build_folders_are_searched() {
    let tree = Tree::new();
    tree.symbol(".build/debug/app", MACHO, "AAAA");
    tree.symbol("target/release/app", ELF, "BBBB");
    tree.symbol("bin/Release/app.pdb", PDB, "CCCC");
    tree.write(".gitignore", b"target\nbin\n.build\n");

    let (result, _) = run(&tree.search());

    assert_eq!(
        tree.found(&result),
        [
            ".build/debug/app",
            "bin/Release/app.pdb",
            "target/release/app"
        ]
    );
}

#[test]
fn max_depth_limits_how_far_below_each_path_to_look() {
    let tree = Tree::new();
    tree.symbol("one.exe", PE, "AAAA");
    tree.symbol("a/two.exe", PE, "BBBB");
    tree.symbol("a/b/three.exe", PE, "CCCC");
    tree.symbol("a/b/c/four.exe", PE, "DDDD");

    let at = |max_depth| {
        let search = Search {
            max_depth,
            ..tree.search()
        };
        let (result, _) = run(&search);
        tree.found(&result)
    };

    assert!(at(Some(0)).is_empty());
    assert_eq!(at(Some(1)), ["one.exe"]);
    assert_eq!(at(Some(2)), ["a/two.exe", "one.exe"]);
    assert_eq!(at(Some(3)), ["a/b/three.exe", "a/two.exe", "one.exe"]);
    assert_eq!(
        at(None),
        ["a/b/c/four.exe", "a/b/three.exe", "a/two.exe", "one.exe"]
    );
}

#[test]
fn max_depth_counts_from_each_path() {
    let tree = Tree::new();
    tree.symbol("a/two.exe", PE, "BBBB");
    tree.symbol("a/b/three.exe", PE, "CCCC");
    tree.symbol("z/y/x/deep.exe", PE, "DDDD");

    let search = Search {
        paths: vec![tree.path("a/b"), tree.path("z")],
        max_depth: Some(1),
        ..Search::default()
    };
    let (result, _) = run(&search);

    assert_eq!(tree.found(&result), ["a/b/three.exe"]);
}

#[cfg(unix)]
mod symlinks {
    use std::os::unix::fs::symlink;

    use super::*;

    fn linked_tree() -> (Tree, Tree) {
        let outside = Tree::new();
        outside.symbol("sdk/lib.dll", PE, "BBBB");
        outside.symbol("tool.exe", PE, "CCCC");

        let tree = Tree::new();
        tree.symbol("app.exe", PE, "AAAA");
        symlink(outside.path("sdk"), tree.path("sdk-link")).unwrap();
        symlink(outside.path("tool.exe"), tree.path("tool-link.exe")).unwrap();
        symlink(tree.path("app.exe"), tree.path("app-link.exe")).unwrap();
        symlink(tree.path("nowhere"), tree.path("broken.exe")).unwrap();
        (tree, outside)
    }

    #[test]
    fn links_are_not_followed_by_default() {
        let (tree, _outside) = linked_tree();
        let (result, reader) = run(&tree.search());

        assert_eq!(tree.found(&result), ["app.exe"]);
        assert_eq!(reader.calls(&tree), ["app.exe"]);
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn follow_searches_linked_files_and_folders() {
        let (tree, _outside) = linked_tree();
        let search = Search {
            follow_links: true,
            ..tree.search()
        };
        let (result, _) = run(&search);

        assert_eq!(
            tree.found(&result),
            ["app-link.exe", "sdk-link/lib.dll", "tool-link.exe"],
            "app.exe is the same file as app-link.exe and is reported once"
        );
    }

    #[test]
    fn a_link_loop_ends_with_a_warning() {
        let tree = Tree::new();
        tree.symbol("a/app.exe", PE, "AAAA");
        symlink(tree.path("a"), tree.path("a/again")).unwrap();

        let search = Search {
            follow_links: true,
            ..tree.search()
        };
        let (result, _) = run(&search);

        assert_eq!(tree.found(&result), ["a/app.exe"]);
        assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
        assert!(result.warnings[0].contains("again"));
    }

    #[test]
    fn a_path_that_is_a_link_is_searched_without_follow() {
        let (tree, outside) = linked_tree();
        let search = Search {
            paths: vec![tree.path("sdk-link"), tree.path("tool-link.exe")],
            ..Search::default()
        };
        let (result, _) = run(&search);

        let expected: Vec<PathBuf> = ["sdk/lib.dll", "tool.exe"]
            .iter()
            .map(|file| fs::canonicalize(outside.path(file)).unwrap())
            .collect();
        let found: Vec<PathBuf> = result
            .matches
            .iter()
            .map(|found| found.info.path.clone())
            .collect();
        assert_eq!(found, expected);
    }

    #[test]
    fn a_bundle_behind_a_link_is_skipped_unless_following() {
        let outside = Tree::new();
        outside.symbol(
            "App.dSYM/Contents/Resources/DWARF/App",
            MACHO,
            &format!("{ARM_ID} App arm64"),
        );
        let tree = Tree::new();
        symlink(outside.path("App.dSYM"), tree.path("App.dSYM")).unwrap();

        let (not_followed, reader) = run(&tree.search());
        assert!(not_followed.matches.is_empty());
        assert!(reader.calls(&tree).is_empty());

        let search = Search {
            follow_links: true,
            ..tree.search()
        };
        let (followed, reader) = run(&search);
        assert_eq!(
            tree.found(&followed),
            ["App.dSYM/Contents/Resources/DWARF/App"]
        );
        assert_eq!(reader.calls(&tree), ["App.dSYM"]);
    }
}

fn bundle_tree() -> Tree {
    let tree = Tree::new();
    tree.symbol(
        "out/MyApp.app.dSYM/Contents/Resources/DWARF/MyApp",
        FAT,
        &format!("{ARM_ID} MyApp arm64\n{X64_ID} MyApp x86_64"),
    );
    tree.write(
        "out/MyApp.app.dSYM/Contents/Info.plist",
        b"<?xml version=\"1.0\"?>",
    );
    tree.symbol(
        "out/MyApp.app/Contents/MacOS/MyApp",
        MACHO,
        &format!("{ARM_ID} MyApp arm64"),
    );
    tree.symbol(
        "out/MyApp.app/Contents/Frameworks/Helper.framework/Helper",
        MACHO,
        "CCCC Helper arm64",
    );
    tree.symbol(
        "out/Release.xcarchive/dSYMs/Other.app.dSYM/Contents/Resources/DWARF/Other",
        MACHO,
        "DDDD Other arm64",
    );
    tree.symbol("out/Loose.framework/Loose", MACHO, "EEEE Loose arm64");
    tree
}

#[test]
fn a_bundle_is_read_as_one_unit_and_each_slice_is_reported() {
    let tree = bundle_tree();
    let (result, reader) = run(&tree.search());

    assert_eq!(
        reader.calls(&tree),
        [
            "out/Loose.framework/Loose",
            "out/MyApp.app",
            "out/MyApp.app.dSYM",
            "out/Release.xcarchive"
        ],
        "the reader gets each bundle once and none of the files inside"
    );

    let found: Vec<(String, &str, Option<&str>, Option<String>)> = result
        .matches
        .iter()
        .map(|found| {
            (
                tree.relative(&found.info.path),
                found.info.debug_id.as_str(),
                found.info.arch.as_deref(),
                found.bundle.as_deref().map(|bundle| tree.relative(bundle)),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (
                "out/Loose.framework/Loose".to_string(),
                "EEEE",
                Some("arm64"),
                None
            ),
            (
                "out/MyApp.app/Contents/Frameworks/Helper.framework/Helper".to_string(),
                "CCCC",
                Some("arm64"),
                Some("out/MyApp.app".to_string())
            ),
            (
                "out/MyApp.app/Contents/MacOS/MyApp".to_string(),
                ARM_ID,
                Some("arm64"),
                Some("out/MyApp.app".to_string())
            ),
            (
                "out/MyApp.app.dSYM/Contents/Resources/DWARF/MyApp".to_string(),
                ARM_ID,
                Some("arm64"),
                Some("out/MyApp.app.dSYM".to_string())
            ),
            (
                "out/MyApp.app.dSYM/Contents/Resources/DWARF/MyApp".to_string(),
                X64_ID,
                Some("x86_64"),
                Some("out/MyApp.app.dSYM".to_string())
            ),
            (
                "out/Release.xcarchive/dSYMs/Other.app.dSYM/Contents/Resources/DWARF/Other"
                    .to_string(),
                "DDDD",
                Some("arm64"),
                Some("out/Release.xcarchive".to_string())
            ),
        ]
    );
}

#[test]
fn one_slice_of_a_bundle_matches_a_debug_id() {
    let tree = bundle_tree();
    let search = Search {
        debug_ids: vec![X64_ID.to_lowercase()],
        ..tree.search()
    };
    let (result, _) = run(&search);

    assert_eq!(result.matches.len(), 1);
    let found = &result.matches[0];
    assert_eq!(found.info.arch.as_deref(), Some("x86_64"));
    assert_eq!(found.info.slice.map(|slice| slice.offset), Some(8192));
    assert_eq!(
        found.bundle.as_deref().map(|bundle| tree.relative(bundle)),
        Some("out/MyApp.app.dSYM".to_string())
    );
    assert!(result.is_complete());
}

#[test]
fn a_bundle_matches_by_its_own_name_or_its_module_name() {
    let tree = bundle_tree();
    let names = |name: &str| Search {
        names: vec![name.to_string()],
        ..tree.search()
    };

    let (by_bundle, _) = run(&names("myapp.app.dsym"));
    assert_eq!(
        tree.found(&by_bundle),
        [
            "out/MyApp.app.dSYM/Contents/Resources/DWARF/MyApp",
            "out/MyApp.app.dSYM/Contents/Resources/DWARF/MyApp"
        ]
    );

    let (by_module, _) = run(&names("other"));
    assert_eq!(
        tree.found(&by_module),
        ["out/Release.xcarchive/dSYMs/Other.app.dSYM/Contents/Resources/DWARF/Other"]
    );
}

#[test]
fn a_bundle_counts_as_one_entry_for_max_depth() {
    let tree = bundle_tree();
    let at = |max_depth| {
        let search = Search {
            max_depth: Some(max_depth),
            ..tree.search()
        };
        let (result, _) = run(&search);
        result.matches.len()
    };

    assert_eq!(at(1), 0);
    assert_eq!(at(2), 5, "the three bundles sit two levels down");
    assert_eq!(
        at(3),
        6,
        "the loose framework's binary is one level further"
    );
}

#[test]
fn a_bundle_in_a_skipped_folder_is_skipped() {
    let tree = Tree::new();
    tree.symbol(
        "node_modules/electron/dist/Electron.app/Contents/MacOS/Electron",
        MACHO,
        "AAAA Electron arm64",
    );

    let (result, reader) = run(&tree.search());
    assert!(result.matches.is_empty());
    assert!(reader.calls(&tree).is_empty());

    let search = Search {
        no_ignore: true,
        ..tree.search()
    };
    let (result, _) = run(&search);
    assert_eq!(result.matches.len(), 1);
}

#[test]
fn a_path_can_be_a_bundle_or_a_file() {
    let tree = bundle_tree();
    tree.symbol("loose/app.pdb", PDB, PDB_ID);
    tree.write("loose/notes.txt", b"MZ, but a text file");

    let search = Search {
        paths: vec![
            tree.path("out/MyApp.app.dSYM"),
            tree.path("loose/app.pdb"),
            tree.path("loose/notes.txt"),
        ],
        ..Search::default()
    };
    let (result, reader) = run(&search);

    assert_eq!(
        tree.found(&result),
        [
            "loose/app.pdb",
            "out/MyApp.app.dSYM/Contents/Resources/DWARF/MyApp",
            "out/MyApp.app.dSYM/Contents/Resources/DWARF/MyApp"
        ]
    );
    assert_eq!(reader.calls(&tree), ["loose/app.pdb", "out/MyApp.app.dSYM"]);
}

#[test]
fn the_reader_is_only_called_for_files_that_could_be_symbols() {
    let tree = Tree::new();
    let mut others = 0;
    for folder in 0..40 {
        for file in 0..25 {
            let base = format!("src/module{folder}/file{file}");
            tree.write(&format!("{base}.c"), b"int f(void) { return 1; }\n");
            tree.write(&format!("{base}.h"), b"int f(void);\n");
            tree.write(&format!("{base}.json"), b"{}\n");
            tree.write(&format!("{base}.dat"), b"plain data\n");
            tree.write(&base, b"#!/bin/sh\necho hi\n");
            others += 5;
        }
    }
    tree.write("assets/model.bin", b"not an ELF file");
    tree.write(
        "lfs/app.pdb",
        b"version https://git-lfs.github.com/spec/v1\n",
    );
    tree.write("lfs/app.dll", b"");
    tree.write("java/Main.class", &[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 61]);
    tree.write("java/Main", &[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 61]);
    tree.write("src/elf.txt", b"\x7fELF in a text file");
    others += 6;
    tree.symbol("src/module7/app.exe", PE, "AAAA");
    tree.symbol("src/module21/libapp.so", ELF, "BBBB");
    tree.symbol("src/module39/app", MACHO, "CCCC");
    assert!(others > 5000);

    let (result, reader) = run(&tree.search());

    let symbols = [
        "src/module21/libapp.so",
        "src/module39/app",
        "src/module7/app.exe",
    ];
    assert_eq!(reader.calls(&tree), symbols);
    assert_eq!(tree.found(&result), symbols);
    assert!(result.warnings.is_empty());
}

#[test]
fn a_file_the_reader_rejects_is_left_out_quietly() {
    let tree = Tree::new();
    tree.symbol("app.exe", PE, "AAAA");
    tree.write("installer.exe", b"MZ");

    let reader = FakeReader::default();
    let result = find(&tree.search(), |path| {
        if path.ends_with("installer.exe") {
            return Err(ReadError::Unsupported(path.to_path_buf()));
        }
        reader.read(path)
    })
    .unwrap();

    assert_eq!(tree.found(&result), ["app.exe"]);
    assert!(result.warnings.is_empty());
}

#[test]
fn a_file_the_reader_cannot_parse_becomes_a_warning() {
    let tree = Tree::new();
    tree.symbol("app.exe", PE, "AAAA");
    tree.symbol("broken.pdb", PDB, "MALFORMED");

    let (result, _) = run(&tree.search());

    assert_eq!(tree.found(&result), ["app.exe"]);
    assert_eq!(result.warnings.len(), 1);
    assert!(result.warnings[0].ends_with("broken.pdb: truncated header"));
}

#[test]
fn a_path_that_does_not_exist_is_an_error() {
    let tree = Tree::new();
    let search = Search {
        paths: vec![tree.root().to_path_buf(), tree.path("missing")],
        ..Search::default()
    };
    let err = find(&search, |path| FakeReader::default().read(path)).unwrap_err();

    assert!(matches!(err, Error::Path { ref path, .. } if path.ends_with("missing")));
}

#[test]
fn overlapping_paths_report_a_file_once() {
    let tree = build_tree();
    let search = Search {
        paths: vec![
            tree.root().to_path_buf(),
            tree.path("build"),
            tree.path("build/myapp.pdb"),
            tree.root().to_path_buf(),
        ],
        names: vec!["myapp.pdb".to_string()],
        ..Search::default()
    };
    let (result, _) = run(&search);

    assert_eq!(
        tree.found(&result),
        ["build/myapp.pdb", "build/sym/myapp.sym"]
    );
}

#[test]
fn results_are_the_same_on_every_run() {
    let tree = bundle_tree();
    let (first, _) = run(&tree.search());
    for _ in 0..10 {
        let (again, _) = run(&tree.search());
        assert_eq!(again, first);
    }
}

#[test]
fn json_has_matches_and_what_is_missing() {
    let tree = Tree::new();
    let pdb = tree.symbol("build/myapp.pdb", PDB, PDB_ID);
    let dsym = tree.path("build/MyApp.app.dSYM");
    let dwarf = tree.symbol(
        "build/MyApp.app.dSYM/Contents/Resources/DWARF/MyApp",
        FAT,
        &format!("{ARM_ID} MyApp arm64"),
    );

    let search = Search {
        debug_ids: vec![PDB_ID.to_lowercase(), "FFFF".to_string()],
        names: vec!["MyApp".to_string(), "*.so".to_string()],
        ..tree.search()
    };
    let (result, _) = run(&search);

    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({
            "matches": [
                {
                    "path": dwarf,
                    "moduleName": "MyApp",
                    "debugId": ARM_ID,
                    "kind": "mach-o",
                    "arch": "arm64",
                    "slice": { "offset": 4096, "size": 4096 },
                    "bundle": dsym,
                    "matchedIds": [],
                    "matchedNames": ["MyApp"]
                },
                {
                    "path": pdb,
                    "moduleName": "myapp.pdb",
                    "debugId": PDB_ID,
                    "kind": "pdb",
                    "arch": null,
                    "slice": null,
                    "bundle": null,
                    "matchedIds": [PDB_ID.to_lowercase()],
                    "matchedNames": []
                }
            ],
            "missingIds": ["FFFF"],
            "missingNames": ["*.so"]
        })
    );
}

#[test]
fn json_for_an_empty_search_has_empty_lists() {
    let tree = Tree::new();
    let (result, _) = run(&tree.search());

    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({ "matches": [], "missingIds": [], "missingNames": [] })
    );
}

#[test]
fn an_empty_path_list_searches_the_current_folder() {
    let search = Search::default();
    let reader = FakeReader::default();
    let result = find(&search, |path| reader.read(path)).unwrap();

    assert!(
        result
            .matches
            .iter()
            .all(|found| found.info.path.starts_with("."))
    );
}

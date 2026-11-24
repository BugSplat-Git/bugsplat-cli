//! `bugsplat symbols find` from the outside. Checks that need real debug ids wait for
//! `bugsplat_debugid::read` (https://github.com/BugSplat-Git/bugsplat-cli/issues/5); the
//! search itself is tested in `bugsplat-symbols`.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use tempfile::TempDir;

const ID: &str = "E546B55B6D214E86871B40AC35CD0D461";

fn find() -> Command {
    let mut command = Command::cargo_bin("bugsplat").unwrap();
    for name in [
        "BUGSPLAT_ACCESS_TOKEN",
        "BUGSPLAT_API_URL",
        "BUGSPLAT_HOST",
        "BUGSPLAT_DATABASE",
        "BUGSPLAT_CLIENT_ID",
        "BUGSPLAT_CLIENT_SECRET",
        "SYMBOL_UPLOAD_CLIENT_ID",
        "SYMBOL_UPLOAD_CLIENT_SECRET",
    ] {
        command.env_remove(name);
    }
    command.args(["symbols", "find"]);
    command
}

/// A tree with source files only: nothing in it is a symbol file to any reader.
fn source_tree() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/main.c"), "int main() { return 0; }\n").unwrap();
    fs::write(dir.path().join("README.md"), "# My app\n").unwrap();
    fs::write(dir.path().join("notes"), "plain text\n").unwrap();
    dir
}

fn stdout_json(assert: &assert_cmd::assert::Assert) -> Value {
    serde_json::from_slice(&assert.get_output().stdout).expect("stdout is JSON")
}

fn path_arg(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn help_shows_every_option() {
    let assert = find().arg("--help").assert().success();
    let help = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for option in [
        "--debug-id <ID>",
        "--name <NAME>",
        "--path <DIR>",
        "--max-depth <N>",
        "--no-ignore",
        "--follow",
        "--json",
    ] {
        assert!(help.contains(option), "--help is missing `{option}`");
    }
}

#[test]
fn listing_a_tree_without_symbols_succeeds_with_nothing_on_stdout() {
    let tree = source_tree();
    find()
        .args(["--path", path_arg(tree.path())])
        .assert()
        .success()
        .stdout("")
        .stderr(predicate::str::contains("No symbol files found."));
}

#[test]
fn quiet_drops_the_nothing_found_note() {
    let tree = source_tree();
    find()
        .args(["--quiet", "--path", path_arg(tree.path())])
        .assert()
        .success()
        .stdout("")
        .stderr("");
}

#[test]
fn the_path_defaults_to_the_current_folder() {
    let tree = source_tree();
    find()
        .current_dir(tree.path())
        .arg("--json")
        .assert()
        .success()
        .stdout(predicate::function(|stdout: &[u8]| {
            serde_json::from_slice::<Value>(stdout).unwrap()
                == json!({ "matches": [], "missingIds": [], "missingNames": [] })
        }));
}

#[test]
fn json_for_a_tree_without_symbols_has_empty_lists() {
    let tree = source_tree();
    let assert = find()
        .args(["--json", "--path", path_arg(tree.path())])
        .assert()
        .success()
        .stderr("");

    assert_eq!(
        stdout_json(&assert),
        json!({ "matches": [], "missingIds": [], "missingNames": [] })
    );
}

#[test]
fn a_debug_id_that_is_not_found_is_listed_and_exits_1() {
    let tree = source_tree();
    find()
        .args(["--debug-id", ID, "--path", path_arg(tree.path())])
        .assert()
        .code(1)
        .stdout(format!("Not found: debug id {ID}\n"))
        .stderr(predicate::str::contains("1 debug id not found"));
}

#[test]
fn json_lists_the_ids_and_names_that_were_not_found_and_exits_1() {
    let tree = source_tree();
    let assert = find()
        .args(["--json", "--path", path_arg(tree.path())])
        .args(["--debug-id", "e546b55b-6d21-4e86-871b-40ac35cd0d46-1"])
        .args(["--debug-id", ID, "--debug-id", "abcd"])
        .args(["--name", "myapp.pdb", "--name", "*.so"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "2 debug ids and 2 names not found",
        ));

    assert_eq!(
        stdout_json(&assert),
        json!({
            "matches": [],
            "missingIds": ["e546b55b-6d21-4e86-871b-40ac35cd0d46-1", "abcd"],
            "missingNames": ["myapp.pdb", "*.so"]
        })
    );
}

#[test]
fn a_file_with_only_a_signature_does_not_match_a_debug_id() {
    let tree = source_tree();
    fs::write(tree.path().join("app.exe"), b"MZ").unwrap();
    fs::write(tree.path().join("app"), b"\x7fELF").unwrap();
    fs::create_dir_all(tree.path().join("App.dSYM/Contents/Resources/DWARF")).unwrap();

    let assert = find()
        .args(["--json", "--quiet", "--debug-id", ID])
        .args(["--path", path_arg(tree.path())])
        .assert()
        .code(1);

    assert_eq!(
        stdout_json(&assert),
        json!({ "matches": [], "missingIds": [ID], "missingNames": [] })
    );
}

#[test]
fn a_name_that_is_not_found_exits_1() {
    let tree = source_tree();
    find()
        .args(["--name", "myapp.pdb", "--path", path_arg(tree.path())])
        .assert()
        .code(1)
        .stdout("Not found: name myapp.pdb\n")
        .stderr(predicate::str::contains("1 name not found"));
}

#[test]
fn several_paths_and_the_walk_options_are_accepted() {
    let first = source_tree();
    let second = source_tree();
    find()
        .args(["--path", path_arg(first.path())])
        .args(["--path", path_arg(second.path())])
        .args(["--max-depth", "2", "--no-ignore", "--follow", "--json"])
        .assert()
        .success();
}

#[test]
fn a_path_that_does_not_exist_is_a_usage_error() {
    let tree = source_tree();
    let missing = tree.path().join("missing");
    find()
        .args(["--path", path_arg(&missing)])
        .assert()
        .code(2)
        .stdout("")
        .stderr(predicate::str::contains("missing"));
}

#[test]
fn a_debug_id_that_is_not_hexadecimal_is_a_usage_error() {
    find()
        .args(["--debug-id", "myapp.pdb"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(predicate::str::contains("`myapp.pdb` is not a debug id"));
}

#[test]
fn a_name_that_is_not_a_valid_glob_is_a_usage_error() {
    let tree = source_tree();
    find()
        .args(["--name", "[", "--path", path_arg(tree.path())])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("`[` is not a valid name pattern"));
}

#[test]
fn a_max_depth_that_is_not_a_number_is_a_usage_error() {
    find().args(["--max-depth", "deep"]).assert().code(2);
}

#[test]
fn an_unknown_option_is_a_usage_error() {
    find().arg("--report").assert().code(2);
}

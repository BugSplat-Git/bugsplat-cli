//! `bugsplat symbols info`.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};

fn bugsplat() -> Command {
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
    command
}

fn fixture(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bugsplat-debugid/tests/fixtures")
        .join(relative)
}

fn info_json(files: &[&Path]) -> Value {
    let assert = bugsplat()
        .args(["symbols", "info", "--json"])
        .args(files)
        .assert()
        .success();
    serde_json::from_slice(&assert.get_output().stdout).unwrap()
}

const HOCKEY: &str = "macho-uuid/bugsplat-ios.app/Frameworks/bugsplat.framework/HockeySDKResources.bundle/Contents/MacOS/HockeySDKResources";

#[test]
fn prints_a_pdb_as_text() {
    let pdb = fixture("pdb-guid/bugsplat.pdb");
    let expected = format!(
        "{}\n  module    bugsplat.pdb\n  debug id  E546B55B6D214E86871B40AC35CD0D461\n  kind      PDB\n",
        pdb.display()
    );
    bugsplat()
        .args(["symbols", "info"])
        .arg(&pdb)
        .assert()
        .success()
        .stdout(expected)
        .stderr("");
}

#[test]
fn prints_each_slice_of_a_fat_binary_as_text() {
    let fat = fixture(HOCKEY);
    let path = fat.display();
    let expected = format!(
        "{path}\n  module    HockeySDKResources\n  debug id  39d9cb72de663ca6b85bbfa04b60782e\n  kind      Mach-O\n  arch      x86_64\n  slice     16592 bytes at offset 16384\n\n\
         {path}\n  module    HockeySDKResources\n  debug id  7f2b35c3974533d1b991ec0da93cc085\n  kind      Mach-O\n  arch      arm64\n  slice     16592 bytes at offset 49152\n"
    );
    bugsplat()
        .args(["symbols", "info"])
        .arg(&fat)
        .assert()
        .success()
        .stdout(expected);
}

#[test]
fn prints_several_files_as_json() {
    let exe = fixture("pdb-guid/bssndrpt.exe");
    let sym = fixture("symbol-upload/windows.sym");
    let elf = fixture("elfy/hello.elf");
    assert_eq!(
        info_json(&[&exe, &sym, &elf]),
        json!([
            {
                "path": exe,
                "moduleName": "bssndrpt.exe",
                "debugId": "64FB82D565000",
                "kind": "pe",
                "arch": "x86",
                "slice": null,
            },
            {
                "path": sym,
                "moduleName": "windows.pdb",
                "debugId": "9DD7CE5C705C45B09BEE297B84B5B9881c",
                "kind": "breakpad-sym",
                "arch": "x86",
                "slice": null,
            },
            {
                "path": elf,
                "moduleName": "hello.elf",
                "debugId": "85fe216fc7dd441f04c237310a56081fbf23c082",
                "kind": "elf",
                "arch": "x86_64",
                "slice": null,
            },
        ])
    );
}

#[test]
fn prints_a_pdb_and_a_portable_pdb_as_json() {
    let native = fixture("pdb-guid/bugsplat.pdb");
    let portable = fixture("pdb-guid/testapp.pdb");
    assert_eq!(
        info_json(&[&native, &portable]),
        json!([
            {
                "path": native,
                "moduleName": "bugsplat.pdb",
                "debugId": "E546B55B6D214E86871B40AC35CD0D461",
                "kind": "pdb",
                "arch": null,
                "slice": null,
            },
            {
                "path": portable,
                "moduleName": "testapp.pdb",
                "debugId": "8FD634ED5F9B446F94D0A5FC87765B43FFFFFFFF",
                "kind": "portable-pdb",
                "arch": null,
                "slice": null,
            },
        ])
    );
}

#[test]
fn prints_a_dsym_bundle_as_json() {
    let bundle = fixture("macho-uuid/bugsplat.app.dSYM");
    assert_eq!(
        info_json(&[&bundle]),
        json!([{
            "path": bundle.join("Contents/Resources/DWARF/bugsplat"),
            "moduleName": "bugsplat",
            "debugId": "ac15902fba2e31c18b29da51e889b321",
            "kind": "mach-o",
            "arch": "x86_64",
            "slice": null,
        }])
    );
}

#[test]
fn prints_the_slices_of_a_fat_binary_as_json() {
    let fat = fixture(HOCKEY);
    let slices = info_json(&[&fat]);
    assert_eq!(slices[0]["debugId"], "39d9cb72de663ca6b85bbfa04b60782e");
    assert_eq!(
        slices[0]["slice"],
        json!({ "offset": 16384, "size": 16592 })
    );
    assert_eq!(slices[1]["debugId"], "7f2b35c3974533d1b991ec0da93cc085");
    assert_eq!(slices[1]["arch"], "arm64");
    assert_eq!(
        slices[1]["slice"],
        json!({ "offset": 49152, "size": 16592 })
    );
}

#[test]
fn a_file_that_is_not_a_symbol_file_fails() {
    let dir = tempfile::tempdir().unwrap();
    let notes = dir.path().join("notes.txt");
    std::fs::write(&notes, "not a symbol file").unwrap();

    for format in [&[][..], &["--json"][..]] {
        bugsplat()
            .args(["symbols", "info"])
            .args(format)
            .arg(&notes)
            .assert()
            .code(1)
            .stdout("")
            .stderr(predicate::str::contains("notes.txt"))
            .stderr(predicate::str::contains(
                "not a PE, PDB, ELF, Mach-O or Breakpad .sym file",
            ));
    }
}

#[test]
fn a_missing_file_fails() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.pdb");

    for format in [&[][..], &["--json"][..]] {
        bugsplat()
            .args(["symbols", "info"])
            .args(format)
            .arg(&missing)
            .assert()
            .code(1)
            .stdout("")
            .stderr(predicate::str::starts_with(format!(
                "error: {}: ",
                missing.display()
            )));
    }
}

#[test]
fn a_folder_that_is_not_a_bundle_fails() {
    let dir = tempfile::tempdir().unwrap();
    bugsplat()
        .args(["symbols", "info"])
        .arg(dir.path())
        .assert()
        .code(1)
        .stderr(predicate::str::contains("is a folder"));
}

#[test]
fn the_files_that_can_be_read_are_printed_when_another_fails() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.pdb");
    let assert = bugsplat()
        .args(["symbols", "info", "--json"])
        .arg(&missing)
        .arg(fixture("elfy/hello.elf"))
        .arg(fixture("symbol-upload/corrupt.exe"))
        .assert()
        .code(1)
        .stderr(predicate::str::contains("missing.pdb"))
        .stderr(predicate::str::contains("corrupt.exe"))
        .stderr(predicate::str::contains("2 of 3 files could not be read"));
    let objects: Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(objects.as_array().unwrap().len(), 1);
    assert_eq!(objects[0]["kind"], "elf");
}

#[test]
fn a_misnamed_file_is_read_by_its_contents_and_noted() {
    let dir = tempfile::tempdir().unwrap();
    let misnamed = dir.path().join("hello.dll");
    std::fs::copy(fixture("elfy/hello.elf"), &misnamed).unwrap();

    bugsplat()
        .args(["symbols", "info"])
        .arg(&misnamed)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "85fe216fc7dd441f04c237310a56081fbf23c082",
        ))
        .stderr(predicate::str::contains(
            "read as ELF; its name suggests PE",
        ));

    bugsplat()
        .args(["symbols", "info", "--quiet"])
        .arg(&misnamed)
        .assert()
        .success()
        .stderr("");
}

#[test]
fn info_needs_a_file() {
    bugsplat().args(["symbols", "info"]).assert().code(2);
}

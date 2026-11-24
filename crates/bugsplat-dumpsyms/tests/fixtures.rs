//! `.sym` files generated from the fixtures `bugsplat-debugid` reads ids from.
//!
//! The `MODULE` lines are the ones `node-dump-syms` 3.0.13, which `symbol-upload --dumpSyms`
//! runs, wrote for the same files on an arm64 Mac, except where a test says otherwise.

use std::fs;
use std::path::{Path, PathBuf};

use bugsplat_dumpsyms::{Error, Symbols, read};

const HOCKEY: &str = "macho-uuid/bugsplat-ios.app/Frameworks/bugsplat.framework/HockeySDKResources.bundle/Contents/MacOS/HockeySDKResources";

fn fixture(relative: &str) -> PathBuf {
    let mut path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("bugsplat-debugid")
        .join("tests")
        .join("fixtures");
    path.extend(relative.split('/'));
    path
}

fn sym_text(symbols: &Symbols) -> String {
    let mut out = Vec::new();
    symbols.write(&mut out).unwrap();
    String::from_utf8(out).unwrap()
}

fn module_lines(all: &[Symbols]) -> Vec<String> {
    all.iter()
        .map(|symbols| sym_text(symbols).lines().next().unwrap().to_string())
        .collect()
}

fn records(text: &str, kind: &str) -> usize {
    text.lines().filter(|line| line.starts_with(kind)).count()
}

/// `(debug id, module name)` as an upload reads them from the written file.
fn stored(symbols: &Symbols) -> (String, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = symbols.write_to(dir.path()).unwrap();
    assert_eq!(path, dir.path().join(&symbols.file_name));
    let object = bugsplat_debugid::read(&path).unwrap().remove(0);
    assert_eq!(object.kind, bugsplat_debugid::ObjectKind::BreakpadSym);
    (object.debug_id, object.module_name)
}

#[test]
fn elf() {
    let all = read(&fixture("elfy/hello.elf")).unwrap();
    assert_eq!(
        module_lines(&all),
        ["MODULE Linux x86_64 6F21FE85DDC71F4404C237310A56081F0 hello.elf"]
    );
    assert_eq!(all[0].file_name, "hello.sym");
    assert_eq!(all[0].module.os, "Linux");
    assert_eq!(all[0].module.arch, "x86_64");
    assert_eq!(all[0].module.debug_id, "6F21FE85DDC71F4404C237310A56081F0");
    assert_eq!(all[0].module.name, "hello.elf");
    let text = sym_text(&all[0]);
    assert!(
        text.contains("INFO CODE_ID 85FE216FC7DD441F04C237310A56081FBF23C082\n"),
        "{text}"
    );
    assert_eq!(records(&text, "PUBLIC"), 14);
    assert_eq!(records(&text, "STACK"), 10);
    assert_eq!(
        stored(&all[0]),
        (
            "6F21FE85DDC71F4404C237310A56081F0".to_string(),
            "hello".to_string()
        )
    );
}

#[test]
fn mach_o() {
    let all = read(&fixture("macho-uuid/bugsplat-ios.app/bugsplat-ios")).unwrap();
    assert_eq!(
        module_lines(&all),
        ["MODULE Mac arm64 59686D50E0A43FB9AD27BAADAD47E0BC0 bugsplat-ios"]
    );
    assert_eq!(all[0].file_name, "bugsplat-ios.sym");
    assert_eq!(
        stored(&all[0]),
        (
            "59686D50E0A43FB9AD27BAADAD47E0BC0".to_string(),
            "bugsplat-ios".to_string()
        )
    );
}

/// `node-dump-syms` converts only the architecture of the machine it runs on (the arm64 line
/// here is the one it wrote); every architecture has its own debug id, so each gets a file.
#[test]
fn a_universal_mach_o_file_yields_every_architecture() {
    let all = read(&fixture(HOCKEY)).unwrap();
    assert_eq!(
        module_lines(&all),
        [
            "MODULE Mac x86_64 39D9CB72DE663CA6B85BBFA04B60782E0 HockeySDKResources",
            "MODULE Mac arm64 7F2B35C3974533D1B991EC0DA93CC0850 HockeySDKResources",
        ]
    );
    assert!(
        all.iter()
            .all(|symbols| symbols.file_name == "HockeySDKResources.sym")
    );
}

/// `node-dump-syms` puts the bundle's name on the line, `bugsplat.app.dSYM`, which BugSplat
/// normalizes to `bugsplat`; here the line names the DWARF file, as an upload of the bundle
/// without `--dump-syms` does.
#[test]
fn dsym_bundle() {
    let bundle = fixture("macho-uuid/bugsplat.app.dSYM");
    let all = read(&bundle).unwrap();
    assert_eq!(
        module_lines(&all),
        ["MODULE Mac x86_64 AC15902FBA2E31C18B29DA51E889B3210 bugsplat"]
    );
    assert_eq!(all[0].file_name, "bugsplat.sym");
    assert_eq!(
        all[0].source,
        bundle
            .join("Contents")
            .join("Resources")
            .join("DWARF")
            .join("bugsplat")
    );
    let text = sym_text(&all[0]);
    assert_eq!(records(&text, "FUNC"), 7);
    assert_eq!(records(&text, "FILE"), 2);
    assert_eq!(
        stored(&all[0]),
        (
            "AC15902FBA2E31C18B29DA51E889B3210".to_string(),
            "bugsplat".to_string()
        )
    );

    let dwarf = read(&all[0].source).unwrap();
    assert_eq!(module_lines(&dwarf), module_lines(&all));
}

#[test]
fn pe_is_named_after_the_pdb_in_its_debug_directory() {
    let all = read(&fixture("pdb-guid/bssndrpt.exe")).unwrap();
    assert_eq!(
        module_lines(&all),
        ["MODULE windows x86 1B2C11749B0541AF95DA13C6186D83F41 BsSndRpt.pdb"]
    );
    assert_eq!(all[0].file_name, "BsSndRpt.sym");
    let text = sym_text(&all[0]);
    assert!(
        text.contains("INFO CODE_ID 64FB82D565000 bssndrpt.exe\n"),
        "{text}"
    );
    assert_eq!(
        stored(&all[0]),
        (
            "1B2C11749B0541AF95DA13C6186D83F41".to_string(),
            "BsSndRpt.pdb".to_string()
        )
    );

    let dll = read(&fixture("pdb-guid/bugsplat.dll")).unwrap();
    assert_eq!(
        module_lines(&dll),
        ["MODULE windows x86 E546B55B6D214E86871B40AC35CD0D461 BugSplat.pdb"]
    );
    assert_eq!(dll[0].file_name, "BugSplat.sym");
}

#[test]
fn pdb_takes_the_code_id_and_unwind_data_from_the_binary_next_to_it() {
    let all = read(&fixture("pdb-guid/bugsplat.pdb")).unwrap();
    assert_eq!(
        module_lines(&all),
        ["MODULE windows x86 E546B55B6D214E86871B40AC35CD0D461 bugsplat.pdb"]
    );
    assert_eq!(all[0].file_name, "bugsplat.sym");
    let text = sym_text(&all[0]);
    assert!(
        text.contains("INFO CODE_ID 64FB82ED7A000 bugsplat.dll\n"),
        "{}",
        &text[..200]
    );
    assert_eq!(records(&text, "FUNC"), 4134);
    assert_eq!(records(&text, "PUBLIC"), 180);
    assert_eq!(records(&text, "STACK"), 6973);
    assert_eq!(
        stored(&all[0]),
        (
            "E546B55B6D214E86871B40AC35CD0D461".to_string(),
            "bugsplat.pdb".to_string()
        )
    );
}

#[test]
fn pe_is_combined_with_the_pdb_next_to_it() {
    let dir = tempfile::tempdir().unwrap();
    let dll = dir.path().join("bugsplat.dll");
    fs::copy(fixture("pdb-guid/bugsplat.dll"), &dll).unwrap();

    let alone = sym_text(&read(&dll).unwrap()[0]);
    assert_eq!(records(&alone, "FUNC"), 0);

    // Named as the debug directory spells it, which matters where file names are case sensitive.
    fs::copy(
        fixture("pdb-guid/bugsplat.pdb"),
        dir.path().join("BugSplat.pdb"),
    )
    .unwrap();
    let all = read(&dll).unwrap();
    assert_eq!(
        module_lines(&all),
        ["MODULE windows x86 E546B55B6D214E86871B40AC35CD0D461 BugSplat.pdb"]
    );
    let combined = sym_text(&all[0]);
    assert_eq!(records(&combined, "FUNC"), 4134);
    assert_eq!(records(&combined, "STACK"), 6973);
}

#[test]
fn a_pdb_or_binary_from_another_build_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("bssndrpt.exe");
    fs::copy(fixture("pdb-guid/bssndrpt.exe"), &exe).unwrap();
    let pdb = dir.path().join("BsSndRpt.pdb");
    fs::copy(fixture("pdb-guid/bugsplat.pdb"), &pdb).unwrap();

    assert_eq!(
        module_lines(&read(&exe).unwrap()),
        ["MODULE windows x86 1B2C11749B0541AF95DA13C6186D83F41 BsSndRpt.pdb"]
    );

    let lone_pdb = dir.path().join("bssndrpt.pdb");
    fs::rename(&pdb, &lone_pdb).unwrap();
    let text = sym_text(&read(&lone_pdb).unwrap()[0]);
    assert!(
        text.starts_with("MODULE windows x86 E546B55B6D214E86871B40AC35CD0D461 bssndrpt.pdb\n"),
        "{}",
        &text[..200]
    );
    assert!(!text.contains("INFO CODE_ID"), "{}", &text[..200]);
}

#[test]
fn files_dump_syms_cannot_convert_are_unsupported() {
    for relative in [
        "symbol-upload/portable.pdb",
        "pdb-guid/testapp.pdb",
        "symbol-upload/android.sym",
        "symbol-upload/corrupt.exe",
        "macho-uuid/corrupt.app/Contents/Resources/DWARF/corrupt",
        "macho-uuid/bugsplat-ios.app",
        "macho-uuid",
    ] {
        let path = fixture(relative);
        assert!(
            matches!(read(&path), Err(Error::Unsupported(unsupported)) if unsupported == path),
            "{relative}"
        );
    }
    assert!(matches!(
        read(&fixture("no-such-file")),
        Err(Error::Io { .. })
    ));
}

#[test]
fn a_bundle_yields_every_dwarf_file_in_it() {
    let dir = tempfile::tempdir().unwrap();
    let dwarf = dir
        .path()
        .join("Two.dSYM")
        .join("Contents")
        .join("Resources")
        .join("DWARF");
    fs::create_dir_all(&dwarf).unwrap();
    fs::copy(
        fixture("macho-uuid/bugsplat.app.dSYM/Contents/Resources/DWARF/bugsplat"),
        dwarf.join("bugsplat"),
    )
    .unwrap();
    fs::copy(fixture(HOCKEY), dwarf.join("Hockey")).unwrap();
    fs::write(dwarf.join(".DS_Store"), b"not an object").unwrap();
    fs::write(dwarf.join("notes.txt"), b"not an object either").unwrap();

    let all = read(&dir.path().join("Two.dSYM")).unwrap();
    let names: Vec<_> = all
        .iter()
        .map(|symbols| (symbols.file_name.as_str(), symbols.module.arch.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("Hockey.sym", "x86_64"),
            ("Hockey.sym", "arm64"),
            ("bugsplat.sym", "x86_64"),
        ]
    );
}

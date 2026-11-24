//! The spec assertions whose fixtures are too large to commit. They read the npm packages'
//! own checkouts and are skipped unless `BUGSPLAT_REFERENCE_DIR` names the folder that holds
//! the `pdb-guid`, `macho-uuid` and `symbol-upload` clones.

use std::path::PathBuf;

use bugsplat_debugid::{DebugInfo, Error, ObjectKind, read};

fn reference(relative: &str) -> Option<PathBuf> {
    match std::env::var_os("BUGSPLAT_REFERENCE_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir).join(relative)),
        None => {
            eprintln!("skipped: BUGSPLAT_REFERENCE_DIR is not set");
            None
        }
    }
}

/// `(path under the bundle, debug id)` of each object, sorted.
fn paths_and_ids(root: &str, objects: &[DebugInfo]) -> Vec<(String, String)> {
    let root = reference(root).unwrap();
    let mut found: Vec<_> = objects
        .iter()
        .map(|object| {
            let relative = object.path.strip_prefix(&root).unwrap();
            let relative = relative.to_string_lossy().replace('\\', "/");
            (relative, object.debug_id.clone())
        })
        .collect();
    found.sort();
    found
}

fn expected(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut pairs: Vec<_> = pairs
        .iter()
        .map(|(path, id)| (path.to_string(), id.to_string()))
        .collect();
    pairs.sort();
    pairs
}

const HOCKEY: &str = "HockeySDKResources.bundle/Contents/MacOS/HockeySDKResources";

#[test]
fn pdb_guid_native_pdb() {
    let Some(path) = reference("pdb-guid/spec/support/bugsplat.pdb") else {
        return;
    };
    let objects = read(&path).unwrap();
    assert_eq!(objects[0].debug_id, "E546B55B6D214E86871B40AC35CD0D461");
}

#[test]
fn pdb_guid_unreal_pdb() {
    let Some(path) = reference("pdb-guid/spec/support/unrealeditor-myunrealcrasher.pdb") else {
        return;
    };
    let objects = read(&path).unwrap();
    assert_eq!(objects[0].debug_id, "369E999AF40647DF8A09C12F8B3BC2661");
    assert_eq!(objects[0].kind, ObjectKind::Pdb);
}

/// No spec asserts this file's id. It is a compiler PDB with an empty DBI stream, and the
/// expected id is what pdb-guid 2.1.2 prints for it.
#[test]
fn symbol_upload_compiler_pdb_has_no_age() {
    let Some(path) = reference("symbol-upload/spec/support/myQtCrasher.vc.pdb") else {
        return;
    };
    let objects = read(&path).unwrap();
    assert_eq!(objects[0].debug_id, "400951E2119D471A87663ABC7B1D2DE6");
}

#[test]
fn symbol_upload_symsrv_dll() {
    let dll = "symbol-upload/spec/support/symsrv/bugsplat.dll/64FB82ED7A000/bugsplat.dll";
    let Some(path) = reference(dll) else {
        return;
    };
    assert_eq!(read(&path).unwrap()[0].debug_id, "64FB82ED7A000");
}

#[test]
fn symbol_upload_whole_android_sym() {
    let Some(path) = reference("symbol-upload/spec/support/android.sym") else {
        return;
    };
    let objects = read(&path).unwrap();
    assert_eq!(objects[0].debug_id, "83AE77A7796BDF7DCE8CE3564B3B5D650");
    assert_eq!(objects[0].module_name, "libc++_shared.so");
}

#[test]
fn symbol_upload_dsym() {
    let root = "symbol-upload/spec/support/bugsplat.app.dSYM";
    let Some(path) = reference(root) else {
        return;
    };
    let objects = read(&path).unwrap();
    assert_eq!(
        paths_and_ids(root, &objects),
        expected(&[
            (
                "Contents/Resources/DWARF/BugsplatTester",
                "2dd1bd2706fa384da5a3a8265921cf9a"
            ),
            (
                "Contents/Resources/DWARF/BugsplatTester",
                "2ce192f6c5963e66b06aa22bde5756a0"
            ),
        ])
    );
    assert!(
        objects
            .iter()
            .all(|object| object.module_name == "BugsplatTester")
    );
}

#[test]
fn symbol_upload_pdb_is_not_a_dsym() {
    let Some(path) = reference("symbol-upload/spec/support/bugsplat.pdb") else {
        return;
    };
    assert_eq!(read(&path).unwrap()[0].kind, ObjectKind::Pdb);
}

#[test]
fn macho_uuid_app_with_a_universal_binary() {
    let root = "macho-uuid/spec/support/bugsplat-ios.app";
    let Some(path) = reference(root) else {
        return;
    };
    let hockey = format!("Frameworks/bugsplat.framework/{HOCKEY}");
    assert_eq!(
        paths_and_ids(root, &read(&path).unwrap()),
        expected(&[
            ("bugsplat-ios", "59686d50e0a43fb9ad27baadad47e0bc"),
            (
                "Frameworks/bugsplat.framework/bugsplat",
                "c3e6032cb73e39d59d872acc2a515240"
            ),
            (&hockey, "39d9cb72de663ca6b85bbfa04b60782e"),
            (&hockey, "7f2b35c3974533d1b991ec0da93cc085"),
        ])
    );
}

#[test]
fn macho_uuid_framework() {
    let root = "macho-uuid/spec/support/bugsplat-ios.app/Frameworks/bugsplat.framework";
    let Some(path) = reference(root) else {
        return;
    };
    assert_eq!(
        paths_and_ids(root, &read(&path).unwrap()),
        expected(&[
            ("bugsplat", "c3e6032cb73e39d59d872acc2a515240"),
            (HOCKEY, "39d9cb72de663ca6b85bbfa04b60782e"),
            (HOCKEY, "7f2b35c3974533d1b991ec0da93cc085"),
        ])
    );
}

#[test]
fn macho_uuid_xcarchive_keeps_the_dsym_copies() {
    let root = "macho-uuid/spec/support/bugsplat.xcarchive";
    let Some(path) = reference(root) else {
        return;
    };
    let dsym = "dSYMs/bugsplat.app.dSYM/Contents/Resources/DWARF/bugsplat";
    let framework = "Products/Applications/bugsplat.app/Contents/Frameworks/bugsplat.framework/Versions/A/bugsplat";
    assert_eq!(
        paths_and_ids(root, &read(&path).unwrap()),
        expected(&[
            (dsym, "e7647c7b5fac31cea802ba1d33948697"),
            (dsym, "eb307a227d663ccf8061cfabf3f9f40b"),
            (framework, "bd44d32ef7bb357c97e9e153c9afc1e2"),
            (framework, "bd29d69f1688362f81ae2e781d86c77e"),
        ])
    );
}

#[test]
fn macho_uuid_fat_dylib() {
    let Some(path) = reference("macho-uuid/spec/support/libggcurl.dylib") else {
        return;
    };
    let objects = read(&path).unwrap();
    let ids: Vec<_> = objects.iter().map(|o| o.debug_id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "6c40dc26058937a89df8060647aa0d35",
            "e7e2a3aa880b3b5386c2b618771528ff"
        ]
    );
    assert!(objects.iter().all(|object| object.slice.is_some()));
}

/// A universal static library: every slice is an archive, so macho-uuid finds no objects.
#[test]
fn macho_uuid_fat_static_library_is_unsupported() {
    let library = "macho-uuid/spec/support/bugsplat-ios.app/Frameworks/bugsplat.framework/Frameworks/CrashReporter.framework/CrashReporter";
    let Some(path) = reference(library) else {
        return;
    };
    assert!(matches!(read(&path), Err(Error::Unsupported(_))));
}

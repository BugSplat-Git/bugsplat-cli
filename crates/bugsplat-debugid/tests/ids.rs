//! The ids `pdb-guid`, `macho-uuid`, `elfy` and `symbol-upload` assert in their spec files,
//! asserted here against the same fixtures. Fixtures too large to commit are covered by
//! `reference.rs`. `android.sym` is the first eight lines of the 1.6 MB original; only its
//! first line is read.

use std::fs;
use std::path::{Path, PathBuf};

use bugsplat_debugid::{DebugInfo, Error, ObjectKind, Slice, is_candidate, read};

fn fixture(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(relative)
}

fn one(relative: &str) -> DebugInfo {
    let mut objects = read(&fixture(relative)).unwrap();
    assert_eq!(objects.len(), 1, "{relative}");
    objects.remove(0)
}

/// `(file name, debug id)` of each object, in the order `read` returned them.
fn names_and_ids(objects: &[DebugInfo]) -> Vec<(&str, &str)> {
    objects
        .iter()
        .map(|object| (object.module_name.as_str(), object.debug_id.as_str()))
        .collect()
}

fn write(dir: &Path, relative: &str, content: &[u8]) -> PathBuf {
    let path = dir.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, content).unwrap();
    path
}

const HOCKEY: &str = "macho-uuid/bugsplat-ios.app/Frameworks/bugsplat.framework/HockeySDKResources.bundle/Contents/MacOS/HockeySDKResources";
const DSYM_DWARF: &str = "macho-uuid/bugsplat.app.dSYM/Contents/Resources/DWARF/bugsplat";

mod pdb_guid {
    use super::*;

    #[test]
    fn native_pdb() {
        let pdb = one("pdb-guid/bugsplat.pdb");
        assert_eq!(pdb.debug_id, "E546B55B6D214E86871B40AC35CD0D461");
        assert_eq!(pdb.kind, ObjectKind::Pdb);
        assert_eq!(pdb.module_name, "bugsplat.pdb");
        assert_eq!(pdb.path, fixture("pdb-guid/bugsplat.pdb"));
        assert_eq!(pdb.slice, None);
    }

    #[test]
    fn exe() {
        let exe = one("pdb-guid/bssndrpt.exe");
        assert_eq!(exe.debug_id, "64FB82D565000");
        assert_eq!(exe.kind, ObjectKind::Pe);
        assert_eq!(exe.module_name, "bssndrpt.exe");
        assert_eq!(exe.arch.as_deref(), Some("x86"));
    }

    #[test]
    fn dll() {
        assert_eq!(one("pdb-guid/bugsplat.dll").debug_id, "64FB82ED7A000");
    }

    #[test]
    fn net_core_dll() {
        assert_eq!(one("pdb-guid/netcore.dll").debug_id, "9271620112000");
    }

    #[test]
    fn dll_without_a_timestamp() {
        assert_eq!(
            one("pdb-guid/unityengine.dll").debug_id,
            "0000000000000000000000000000000018000"
        );
    }

    #[test]
    fn portable_pdb() {
        let pdb = one("pdb-guid/testapp.pdb");
        assert_eq!(pdb.debug_id, "8FD634ED5F9B446F94D0A5FC87765B43FFFFFFFF");
        assert_eq!(pdb.kind, ObjectKind::PortablePdb);
    }

    #[test]
    fn a_native_pdb_is_not_a_portable_pdb() {
        assert_eq!(one("pdb-guid/bugsplat.pdb").kind, ObjectKind::Pdb);
    }
}

mod symbol_upload {
    use super::*;

    #[test]
    fn portable_pdb() {
        assert_eq!(
            one("symbol-upload/portable.pdb").debug_id,
            "153A24FA52FF4C03813A890A535486B8FFFFFFFF"
        );
    }

    #[test]
    fn an_exe_that_is_not_one_is_unsupported() {
        let err = read(&fixture("symbol-upload/corrupt.exe")).unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)), "{err}");
    }

    #[test]
    fn elf() {
        let elf = one("elfy/hello.elf");
        assert_eq!(elf.debug_id, "85fe216fc7dd441f04c237310a56081fbf23c082");
        assert_eq!(elf.kind, ObjectKind::Elf);
        assert_eq!(elf.module_name, "hello.elf");
        assert_eq!(elf.arch.as_deref(), Some("x86_64"));
    }

    #[test]
    fn sym_with_a_33_character_id() {
        let sym = one("symbol-upload/android.sym");
        assert_eq!(sym.debug_id, "83AE77A7796BDF7DCE8CE3564B3B5D650");
        assert_eq!(sym.module_name, "libc++_shared.so");
        assert_eq!(sym.kind, ObjectKind::BreakpadSym);
        assert_eq!(sym.arch.as_deref(), Some("arm64"));
    }

    #[test]
    fn sym_with_a_34_character_id_and_crlf() {
        let sym = one("symbol-upload/windows.sym");
        assert_eq!(sym.debug_id, "9DD7CE5C705C45B09BEE297B84B5B9881c");
        assert_eq!(sym.module_name, "windows.pdb");
    }

    #[test]
    fn sym_with_spaces_in_the_module_name() {
        let sym = one("symbol-upload/spaces.sym");
        assert_eq!(sym.debug_id, "4C4C445055553144A10453F91A3BB4950");
        assert_eq!(sym.module_name, "Electron Helper (GPU)");
    }

    #[test]
    fn sym_for_a_dylib_dsym() {
        let sym = one("symbol-upload/liba.dylib.sym");
        assert_eq!(sym.debug_id, "F2DF950B5C5633B88308722BDB7790100");
        assert_eq!(sym.module_name, "liba.dylib");
    }
}

mod macho_uuid {
    use super::*;

    #[test]
    fn dsym_file() {
        let dsym = one(DSYM_DWARF);
        assert_eq!(dsym.debug_id, "ac15902fba2e31c18b29da51e889b321");
        assert_eq!(dsym.kind, ObjectKind::MachO);
        assert_eq!(dsym.module_name, "bugsplat");
        assert_eq!(dsym.arch.as_deref(), Some("x86_64"));
        assert_eq!(dsym.slice, None);
    }

    #[test]
    fn app_binary() {
        let app = one("macho-uuid/bugsplat-ios.app/bugsplat-ios");
        assert_eq!(app.debug_id, "59686d50e0a43fb9ad27baadad47e0bc");
        assert_eq!(app.arch.as_deref(), Some("arm64"));
    }

    #[test]
    fn fat_binary() {
        let slices = read(&fixture(HOCKEY)).unwrap();
        assert_eq!(
            names_and_ids(&slices),
            [
                ("HockeySDKResources", "39d9cb72de663ca6b85bbfa04b60782e"),
                ("HockeySDKResources", "7f2b35c3974533d1b991ec0da93cc085"),
            ]
        );
        assert_eq!(slices[0].arch.as_deref(), Some("x86_64"));
        assert_eq!(slices[1].arch.as_deref(), Some("arm64"));
        let (offset, size) = (16384, 16592);
        assert_eq!(slices[0].slice, Some(Slice { offset, size }));
        let (offset, size) = (49152, 16592);
        assert_eq!(slices[1].slice, Some(Slice { offset, size }));
    }

    #[test]
    fn dsym_bundle() {
        let dsym = one("macho-uuid/bugsplat.app.dSYM");
        assert_eq!(dsym.debug_id, "ac15902fba2e31c18b29da51e889b321");
        assert_eq!(dsym.path, fixture(DSYM_DWARF));
    }

    #[test]
    fn bundle() {
        let bundle =
            "macho-uuid/bugsplat-ios.app/Frameworks/bugsplat.framework/HockeySDKResources.bundle";
        let objects = read(&fixture(bundle)).unwrap();
        assert_eq!(
            names_and_ids(&objects),
            [
                ("HockeySDKResources", "39d9cb72de663ca6b85bbfa04b60782e"),
                ("HockeySDKResources", "7f2b35c3974533d1b991ec0da93cc085"),
            ]
        );
        assert!(objects.iter().all(|object| object.path == fixture(HOCKEY)));
    }

    /// The committed copy of the app leaves out its two largest binaries; `reference.rs`
    /// reads the whole app.
    #[test]
    fn app_with_a_universal_binary() {
        let objects = read(&fixture("macho-uuid/bugsplat-ios.app")).unwrap();
        assert_eq!(
            names_and_ids(&objects),
            [
                ("HockeySDKResources", "39d9cb72de663ca6b85bbfa04b60782e"),
                ("HockeySDKResources", "7f2b35c3974533d1b991ec0da93cc085"),
                ("bugsplat-ios", "59686d50e0a43fb9ad27baadad47e0bc"),
            ]
        );
    }

    #[test]
    fn a_file_that_is_not_mach_o_is_unsupported() {
        let corrupt = "macho-uuid/corrupt.app/Contents/Resources/DWARF/corrupt";
        let err = read(&fixture(corrupt)).unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)), "{err}");
    }

    #[test]
    fn a_bundle_without_mach_o_files_is_unsupported() {
        let err = read(&fixture("macho-uuid/corrupt.app")).unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)), "{err}");
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        let err = read(Path::new("does-not-exist")).unwrap_err();
        assert!(matches!(err, Error::Io { .. }), "{err}");
    }

    #[test]
    fn fat_and_thin_files_and_bundles_are_candidates() {
        assert!(is_candidate(&fixture(HOCKEY)));
        assert!(is_candidate(&fixture(DSYM_DWARF)));
        assert!(is_candidate(&fixture("macho-uuid/bugsplat.app.dSYM")));
    }
}

mod bundles {
    use super::*;

    fn dwarf() -> Vec<u8> {
        fs::read(fixture(DSYM_DWARF)).unwrap()
    }

    fn padded(mut file: Vec<u8>, extra: usize) -> Vec<u8> {
        file.extend(vec![0; extra]);
        file
    }

    #[test]
    fn the_dsym_copy_of_a_uuid_wins_over_a_larger_executable() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.xcarchive");
        let dsym = "dSYMs/a.app.dSYM/Contents/Resources/DWARF/a";
        write(&archive, dsym, &dwarf());
        write(
            &archive,
            "Products/Applications/a.app/Contents/MacOS/a",
            &padded(dwarf(), 4096),
        );

        let objects = read(&archive).unwrap();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].path, archive.join(dsym));
        assert_eq!(objects[0].debug_id, "ac15902fba2e31c18b29da51e889b321");
    }

    #[test]
    fn outside_a_dsym_the_larger_copy_of_a_uuid_wins() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("a.app");
        write(
            &app,
            "Contents/Frameworks/a.framework/a",
            &padded(dwarf(), 4096),
        );
        write(&app, "Contents/MacOS/a", &dwarf());
        write(&app, "Contents/Resources/notes.txt", b"not Mach-O");

        let objects = read(&app).unwrap();
        assert_eq!(objects.len(), 1);
        assert_eq!(
            objects[0].path,
            app.join("Contents/Frameworks/a.framework/a")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_inside_a_bundle_are_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let framework = dir.path().join("a.framework");
        write(&framework, "Versions/A/a", &dwarf());
        std::os::unix::fs::symlink("A", framework.join("Versions/Current")).unwrap();
        std::os::unix::fs::symlink("Versions/Current/a", framework.join("a")).unwrap();

        let objects = read(&framework).unwrap();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].path, framework.join("Versions/A/a"));
    }

    #[test]
    fn a_folder_that_is_not_a_bundle_is_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a", &dwarf());
        let err = read(dir.path()).unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)), "{err}");
        assert!(!is_candidate(dir.path()));
    }
}

mod contents_decide {
    use super::*;

    #[test]
    fn an_elf_named_dll_is_read_as_elf() {
        let dir = tempfile::tempdir().unwrap();
        let elf = fs::read(fixture("elfy/hello.elf")).unwrap();
        let path = write(dir.path(), "hello.dll", &elf);

        let objects = read(&path).unwrap();
        assert_eq!(objects[0].kind, ObjectKind::Elf);
        assert_eq!(
            objects[0].debug_id,
            "85fe216fc7dd441f04c237310a56081fbf23c082"
        );
        assert_eq!(objects[0].module_name, "hello.dll");
    }

    #[test]
    fn a_pe_named_pdb_is_read_as_pe() {
        let dir = tempfile::tempdir().unwrap();
        let exe = fs::read(fixture("pdb-guid/bssndrpt.exe")).unwrap();
        let path = write(dir.path(), "bssndrpt.pdb", &exe);

        let objects = read(&path).unwrap();
        assert_eq!(objects[0].kind, ObjectKind::Pe);
        assert_eq!(objects[0].debug_id, "64FB82D565000");
    }

    #[test]
    fn a_sym_file_needs_no_extension() {
        let dir = tempfile::tempdir().unwrap();
        let sym = fs::read(fixture("symbol-upload/spaces.sym")).unwrap();
        let path = write(dir.path(), "symbols", &sym);
        assert_eq!(read(&path).unwrap()[0].kind, ObjectKind::BreakpadSym);
    }

    /// symbol-upload reads the first line of any `.sym` file, so a byte order mark in front
    /// of `MODULE` doesn't hide the id from it.
    #[test]
    fn a_sym_extension_is_enough_when_the_first_line_has_an_id() {
        let dir = tempfile::tempdir().unwrap();
        let line = "\u{feff}MODULE Linux x86_64 83AE77A7796BDF7DCE8CE3564B3B5D650 liba.so\n";
        let sym = write(dir.path(), "liba.so.sym", line.as_bytes());
        let text = write(dir.path(), "liba.so.txt", line.as_bytes());

        let objects = read(&sym).unwrap();
        assert_eq!(objects[0].debug_id, "83AE77A7796BDF7DCE8CE3564B3B5D650");
        assert_eq!(objects[0].module_name, "liba.so");
        assert!(matches!(read(&text), Err(Error::Unsupported(_))));
    }

    #[test]
    fn a_module_line_without_an_id_is_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "a.sym", b"MODULE Linux x86_64 000 a\n");
        let err = read(&path).unwrap_err();
        assert!(matches!(err, Error::Malformed { .. }), "{err}");
    }

    #[test]
    fn a_dos_executable_is_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let mut dos = b"MZ".to_vec();
        dos.extend([0; 126]);
        let path = write(dir.path(), "old.exe", &dos);
        assert!(matches!(read(&path), Err(Error::Unsupported(_))));
    }

    #[test]
    fn a_mach_o_without_a_uuid_has_an_empty_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut header = vec![0xcf, 0xfa, 0xed, 0xfe];
        header.extend(0x0100_000c_u32.to_le_bytes());
        header.extend([0; 24]);
        let path = write(dir.path(), "a.o", &header);

        let objects = read(&path).unwrap();
        assert_eq!(objects[0].kind, ObjectKind::MachO);
        assert_eq!(objects[0].debug_id, "");
        assert_eq!(objects[0].arch.as_deref(), Some("arm64"));
    }

    #[test]
    fn a_fat_file_without_mach_o_slices_is_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let mut fat = vec![0xca, 0xfe, 0xba, 0xbe];
        fat.extend(1u32.to_be_bytes());
        for field in [0x0100_000c_u32, 0, 32, 8, 0] {
            fat.extend(field.to_be_bytes());
        }
        fat.extend(b"!<arch>\n");
        let path = write(dir.path(), "liba.a", &fat);
        assert!(matches!(read(&path), Err(Error::Unsupported(_))));
    }

    #[test]
    fn a_truncated_pdb_is_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let pdb = fs::read(fixture("pdb-guid/bugsplat.pdb")).unwrap();
        let path = write(dir.path(), "bugsplat.pdb", &pdb[..4096]);
        let err = read(&path).unwrap_err();
        assert!(matches!(err, Error::Malformed { .. }), "{err}");
    }

    #[test]
    fn an_empty_file_is_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "empty.pdb", b"");
        assert!(matches!(read(&path), Err(Error::Unsupported(_))));
        assert!(!is_candidate(&path));
    }

    #[test]
    fn candidates_are_the_files_read_recognizes() {
        for relative in [
            "pdb-guid/bugsplat.pdb",
            "pdb-guid/testapp.pdb",
            "pdb-guid/bugsplat.dll",
            "elfy/hello.elf",
            "symbol-upload/windows.sym",
            "macho-uuid/bugsplat-ios.app/bugsplat-ios",
            "macho-uuid/bugsplat-ios.app",
        ] {
            assert!(is_candidate(&fixture(relative)), "{relative}");
        }
        for relative in [
            "symbol-upload/corrupt.exe",
            "macho-uuid/bugsplat.app.dSYM/Contents/Info.plist",
            "does-not-exist",
        ] {
            assert!(!is_candidate(&fixture(relative)), "{relative}");
        }
    }
}

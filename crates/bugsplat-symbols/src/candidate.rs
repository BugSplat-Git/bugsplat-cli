//! Decides which files are worth handing to the debug-id reader, without parsing them.

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// Long enough for the longest magic, the 24 bytes of the MSF 7.00 signature.
const HEAD_LEN: usize = 32;

const MAGICS: &[&[u8]] = &[
    b"MZ",
    b"Microsoft C/C++ MSF 7.00",
    b"BSJB",
    b"\x7fELF",
    b"MODULE ",
    &[0xfe, 0xed, 0xfa, 0xce],
    &[0xce, 0xfa, 0xed, 0xfe],
    &[0xfe, 0xed, 0xfa, 0xcf],
    &[0xcf, 0xfa, 0xed, 0xfe],
    &[0xbe, 0xba, 0xfe, 0xca],
    &[0xca, 0xfe, 0xba, 0xbf],
    &[0xbf, 0xba, 0xfe, 0xca],
];

const FAT_MAGIC: &[u8] = &[0xca, 0xfe, 0xba, 0xbe];

/// Java class files share the fat Mach-O magic. The next four bytes are the slice count in
/// a fat file and the class file version in a class file, and class versions start at 45.
const FIRST_JAVA_CLASS_VERSION: u32 = 45;

/// A `.dSYM`, `.app` or `.xcarchive` folder, which is read as one unit.
pub fn is_bundle(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            ["dsym", "app", "xcarchive"]
                .iter()
                .any(|bundle| extension.eq_ignore_ascii_case(bundle))
        })
}

/// Whether `path` could be a PE, PDB, ELF, Mach-O or Breakpad `.sym` file: its extension
/// doesn't rule it out and its first bytes are one of those formats' signatures.
pub fn could_be_symbol_file(path: &Path) -> io::Result<bool> {
    if !extension_could_be_symbol(path) {
        return Ok(false);
    }
    let mut head = [0u8; HEAD_LEN];
    let len = read_head(path, &mut head)?;
    Ok(has_symbol_magic(&head[..len]))
}

/// False for the source, text, media and archive files that fill a source tree, so they
/// are skipped without being opened. Every other file is judged by its first bytes, because
/// ELF and Mach-O binaries have no extension or an arbitrary one (`libfoo.so.1`,
/// `game.x86_64`).
pub(crate) fn extension_could_be_symbol(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(OsStr::to_str) else {
        return true;
    };
    !matches!(
        extension.to_ascii_lowercase().as_str(),
        "c" | "cc"
            | "cpp"
            | "cxx"
            | "h"
            | "hh"
            | "hpp"
            | "hxx"
            | "inl"
            | "ipp"
            | "m"
            | "mm"
            | "swift"
            | "rs"
            | "go"
            | "java"
            | "class"
            | "jar"
            | "kt"
            | "kts"
            | "cs"
            | "vb"
            | "fs"
            | "py"
            | "pyc"
            | "pyi"
            | "rb"
            | "php"
            | "pl"
            | "lua"
            | "js"
            | "mjs"
            | "cjs"
            | "jsx"
            | "ts"
            | "tsx"
            | "map"
            | "css"
            | "scss"
            | "sass"
            | "less"
            | "html"
            | "htm"
            | "xml"
            | "xaml"
            | "json"
            | "jsonc"
            | "yaml"
            | "yml"
            | "toml"
            | "ini"
            | "cfg"
            | "conf"
            | "lock"
            | "md"
            | "markdown"
            | "rst"
            | "txt"
            | "log"
            | "csv"
            | "tsv"
            | "sql"
            | "sh"
            | "bash"
            | "zsh"
            | "ps1"
            | "bat"
            | "cmd"
            | "cmake"
            | "mk"
            | "gradle"
            | "props"
            | "targets"
            | "sln"
            | "csproj"
            | "vcxproj"
            | "filters"
            | "plist"
            | "strings"
            | "storyboard"
            | "xib"
            | "pbxproj"
            | "xcconfig"
            | "entitlements"
            | "glsl"
            | "hlsl"
            | "shader"
            | "usf"
            | "ush"
            | "proto"
            | "meta"
            | "uasset"
            | "umap"
            | "prefab"
            | "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "bmp"
            | "ico"
            | "icns"
            | "svg"
            | "webp"
            | "tga"
            | "psd"
            | "tif"
            | "tiff"
            | "dds"
            | "exr"
            | "wav"
            | "mp3"
            | "ogg"
            | "flac"
            | "mp4"
            | "mov"
            | "avi"
            | "mkv"
            | "webm"
            | "ttf"
            | "otf"
            | "woff"
            | "woff2"
            | "eot"
            | "pdf"
            | "zip"
            | "gz"
            | "tgz"
            | "bz2"
            | "xz"
            | "zst"
            | "7z"
            | "rar"
            | "tar"
            | "o"
            | "obj"
            | "a"
            | "lib"
            | "rlib"
            | "rmeta"
            | "d"
            | "pch"
            | "gch"
            | "ilk"
            | "exp"
            | "tlog"
    )
}

pub(crate) fn has_symbol_magic(head: &[u8]) -> bool {
    if MAGICS.iter().any(|magic| head.starts_with(magic)) {
        return true;
    }
    head.starts_with(FAT_MAGIC)
        && head
            .get(4..8)
            .and_then(|count| count.try_into().ok())
            .map(u32::from_be_bytes)
            .is_some_and(|count| (1..FIRST_JAVA_CLASS_VERSION).contains(&count))
}

fn read_head(path: &Path, head: &mut [u8]) -> io::Result<usize> {
    let mut file = File::open(path)?;
    let mut len = 0;
    while len < head.len() {
        match file.read(&mut head[len..]) {
            Ok(0) => break,
            Ok(read) => len += read,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The extensions `symbol-upload` (src/info.ts) reads a debug id from.
    const SYMBOL_UPLOAD_EXTENSIONS: &[&str] = &[
        "pdb", "exe", "dll", "sym", "elf", "self", "prx", "sprx", "nss", "nrs", "bin",
    ];

    #[test]
    fn every_symbol_upload_extension_gets_its_first_bytes_checked() {
        for extension in SYMBOL_UPLOAD_EXTENSIONS {
            for name in [
                format!("module.{extension}"),
                format!("MODULE.{}", extension.to_uppercase()),
            ] {
                assert!(extension_could_be_symbol(Path::new(&name)), "{name}");
            }
        }
    }

    #[test]
    fn binaries_without_a_known_extension_get_their_first_bytes_checked() {
        for name in [
            "myapp",
            "libfoo.so",
            "libfoo.so.1.2.3",
            "libfoo.dylib",
            "game.x86_64",
            "myapp.debug",
            "addon.node",
            "driver.sys",
        ] {
            assert!(extension_could_be_symbol(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn source_and_text_files_are_ruled_out_by_extension() {
        for name in [
            "main.c",
            "Main.CPP",
            "index.ts",
            "bundle.js.map",
            "README.md",
            "logo.png",
            "main.o",
            "Foo.class",
        ] {
            assert!(!extension_could_be_symbol(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn recognizes_every_format_by_its_first_bytes() {
        let heads: &[(&str, &[u8])] = &[
            ("PE", b"MZ\x90\x00\x03\x00"),
            ("PDB", b"Microsoft C/C++ MSF 7.00\r\n\x1aDS\0\0\0"),
            ("portable PDB", b"BSJB\x01\x00\x01\x00"),
            ("ELF", b"\x7fELF\x02\x01\x01\x00"),
            ("Breakpad", b"MODULE Linux x86_64 0123 app"),
            ("Mach-O 32", &[0xfe, 0xed, 0xfa, 0xce, 0, 0, 0, 0]),
            ("Mach-O 32 LE", &[0xce, 0xfa, 0xed, 0xfe, 0, 0, 0, 0]),
            ("Mach-O 64", &[0xfe, 0xed, 0xfa, 0xcf, 0, 0, 0, 0]),
            ("Mach-O 64 LE", &[0xcf, 0xfa, 0xed, 0xfe, 0, 0, 0, 0]),
            ("fat", &[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2]),
            ("fat swapped", &[0xbe, 0xba, 0xfe, 0xca, 2, 0, 0, 0]),
            ("fat 64", &[0xca, 0xfe, 0xba, 0xbf, 0, 0, 0, 2]),
            ("fat 64 swapped", &[0xbf, 0xba, 0xfe, 0xca, 2, 0, 0, 0]),
        ];
        for (format, head) in heads {
            assert!(has_symbol_magic(head), "{format}");
        }
    }

    #[test]
    fn rejects_other_first_bytes() {
        let heads: &[(&str, &[u8])] = &[
            ("empty", b""),
            ("one byte", b"M"),
            ("text", b"#include <stdio.h>\n"),
            (
                "git lfs pointer",
                b"version https://git-lfs.github.com/spec/v1",
            ),
            ("zip", b"PK\x03\x04"),
            ("archive", b"!<arch>\n"),
            ("old PDB", b"Microsoft C/C++ program database 2.00"),
            ("module without a space", b"MODULES"),
            ("java class", &[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 52]),
            ("fat without slices", &[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 0]),
            ("fat magic alone", &[0xca, 0xfe, 0xba, 0xbe]),
        ];
        for (format, head) in heads {
            assert!(!has_symbol_magic(head), "{format}");
        }
    }

    #[test]
    fn a_file_is_a_candidate_when_extension_and_first_bytes_agree() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };

        assert!(could_be_symbol_file(&write("app.exe", b"MZ\x90\x00")).unwrap());
        assert!(could_be_symbol_file(&write("app", b"\x7fELF\x02")).unwrap());
        assert!(!could_be_symbol_file(&write("lfs.pdb", b"version https://git-lfs")).unwrap());
        assert!(!could_be_symbol_file(&write("notes", b"hello")).unwrap());
        assert!(!could_be_symbol_file(&write("empty.dll", b"")).unwrap());
        assert!(!could_be_symbol_file(&write("elf.txt", b"\x7fELF\x02")).unwrap());
        assert!(could_be_symbol_file(&dir.path().join("gone.exe")).is_err());
    }

    #[test]
    fn bundles_are_recognized_by_extension_in_any_case() {
        for name in [
            "MyApp.app.dSYM",
            "MyApp.app",
            "MyApp.xcarchive",
            "x/MyApp.DSYM",
        ] {
            assert!(is_bundle(Path::new(name)), "{name}");
        }
        for name in ["MyApp.framework", "MyApp", "dSYM", "app"] {
            assert!(!is_bundle(Path::new(name)), "{name}");
        }
    }
}

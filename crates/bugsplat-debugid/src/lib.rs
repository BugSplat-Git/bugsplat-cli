//! Debug ids from PE, PDB, ELF, Mach-O, dSYM and Breakpad .sym files.
//! See https://github.com/BugSplat-Git/bugsplat-cli/issues/5
//!
//! The ids match what BugSplat's npm packages (pdb-guid, macho-uuid, elfy, symbol-upload)
//! produce, because symbol uploads are keyed on them.

mod data;
mod elf;
mod macho;
mod pdb;
mod pe;
mod sniff;
mod sym;

use std::fs::File;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::data::Data;
use crate::sniff::Format;

pub use crate::sym::normalize_sym_module_name;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObjectKind {
    Pe,
    Pdb,
    PortablePdb,
    Elf,
    MachO,
    BreakpadSym,
}

/// Where one architecture's object sits inside a fat Mach-O file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Slice {
    pub offset: u64,
    pub size: u64,
}

/// One object in a file. A fat Mach-O binary or a dSYM bundle yields several.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugInfo {
    /// The file the object was read from.
    pub path: PathBuf,
    /// The name BugSplat stores the file under, usually the file name.
    pub module_name: String,
    /// The id as BugSplat stores it. Empty when the file is a known kind without an id.
    pub debug_id: String,
    pub kind: ObjectKind,
    pub arch: Option<String>,
    /// Set for each object of a fat Mach-O file; `None` when the object is the whole file.
    pub slice: Option<Slice>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("{}: not a PE, PDB, ELF, Mach-O or Breakpad .sym file", .0.display())]
    Unsupported(PathBuf),

    #[error("{}: {reason}", path.display())]
    Malformed { path: PathBuf, reason: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Every object in `path`, which may be a file or a `.dSYM`, `.app`, `.framework`, `.bundle`
/// or `.xcarchive` bundle.
///
/// The kind comes from the file's contents, never its extension. A bundle yields the Mach-O
/// objects inside it, one per UUID, preferring the copy in a dSYM. Any other folder, and any
/// file that isn't one of the known kinds, is [`Error::Unsupported`].
pub fn read(path: &Path) -> Result<Vec<DebugInfo>> {
    let metadata = std::fs::metadata(path).map_err(|source| io_error(path, source))?;
    if !metadata.is_dir() {
        return read_file(path);
    }
    if !is_bundle(path) {
        return Err(Error::Unsupported(path.to_path_buf()));
    }
    let objects = macho::read_bundle(path).map_err(|source| io_error(path, source))?;
    if objects.is_empty() {
        return Err(Error::Unsupported(path.to_path_buf()));
    }
    Ok(objects)
}

/// Whether `path` is worth passing to [`read`]: a bundle folder, a file that starts with one
/// of the known kinds' magic bytes, or a `.sym` file. Reads at most the first 32 bytes.
pub fn is_candidate(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if metadata.is_dir() {
        return is_bundle(path);
    }
    if has_extension(path, &["sym"]) {
        return true;
    }
    File::open(path)
        .map(|file| sniff::sniff(sniff::head(&Data::new(file))).is_some())
        .unwrap_or(false)
}

/// Whether `path` is named like a folder [`read`] searches for Mach-O objects. Looks only at
/// the extension.
pub fn is_bundle(path: &Path) -> bool {
    has_extension(path, &["dsym", "app", "framework", "bundle", "xcarchive"])
}

/// The kind `symbol-upload` and `macho-uuid` would assume from the extension alone. [`read`]
/// doesn't use it; callers can compare it with [`DebugInfo::kind`] to spot a misnamed file.
/// `.pdb` gives [`ObjectKind::Pdb`] for both native and portable PDBs.
pub fn kind_from_extension(path: &Path) -> Option<ObjectKind> {
    const KINDS: &[(&[&str], ObjectKind)] = &[
        (&["pdb"], ObjectKind::Pdb),
        (&["exe", "dll"], ObjectKind::Pe),
        (&["sym"], ObjectKind::BreakpadSym),
        (
            &["elf", "self", "prx", "sprx", "nss", "nrs", "bin"],
            ObjectKind::Elf,
        ),
        (
            &["dsym", "app", "framework", "bundle", "xcarchive", "dylib"],
            ObjectKind::MachO,
        ),
    ];
    KINDS
        .iter()
        .find(|(extensions, _)| has_extension(path, extensions))
        .map(|(_, kind)| *kind)
}

/// The form ids are compared in: uppercase, without dashes or surrounding space.
pub fn normalize_id(id: &str) -> String {
    id.trim()
        .chars()
        .filter(|c| *c != '-')
        .flat_map(char::to_uppercase)
        .collect()
}

fn read_file(path: &Path) -> Result<Vec<DebugInfo>> {
    let file = File::open(path).map_err(|source| io_error(path, source))?;
    let data = &Data::new(file);
    let unsupported = || Error::Unsupported(path.to_path_buf());
    let malformed = |reason: String| Error::Malformed {
        path: path.to_path_buf(),
        reason,
    };
    let object = |kind, debug_id: String, arch: Option<String>| DebugInfo {
        path: path.to_path_buf(),
        module_name: file_name(path),
        debug_id,
        kind,
        arch,
        slice: None,
    };

    let head = sniff::head(data);
    let Some(format) = sniff::sniff(head) else {
        // symbol-upload reads any `.sym` file's first line, whatever the line starts with.
        if has_extension(path, &["sym"])
            && let Some(module) = sym::read(data)
        {
            return Ok(vec![sym_object(path, module)]);
        }
        return Err(unsupported());
    };

    match format {
        Format::Msf => {
            let id = pdb::msf_id(data).map_err(malformed)?;
            Ok(vec![object(ObjectKind::Pdb, id, None)])
        }
        Format::PortablePdb => {
            let id = pdb::portable_id(data).map_err(malformed)?;
            Ok(vec![object(ObjectKind::PortablePdb, id, None)])
        }
        Format::Mz => {
            let pe = pe::read(data).ok_or_else(unsupported)?;
            Ok(vec![object(ObjectKind::Pe, pe.id, pe.arch)])
        }
        Format::Elf => {
            let elf = elf::read(data).map_err(malformed)?;
            Ok(vec![object(ObjectKind::Elf, elf.id, elf.arch)])
        }
        Format::MachO => {
            let thin = macho::read_thin(data, 0).map_err(malformed)?;
            Ok(vec![object(
                ObjectKind::MachO,
                thin.uuid.unwrap_or_default(),
                thin.arch,
            )])
        }
        Format::MachOFat => {
            let slices = macho::read_fat(data).map_err(malformed)?;
            if slices.is_empty() {
                return Err(unsupported());
            }
            Ok(slices
                .into_iter()
                .map(|(slice, thin)| DebugInfo {
                    slice: Some(slice),
                    ..object(ObjectKind::MachO, thin.uuid.unwrap_or_default(), thin.arch)
                })
                .collect())
        }
        Format::Sym => {
            let module = sym::read(data)
                .ok_or_else(|| malformed("the MODULE line has no debug id".to_string()))?;
            Ok(vec![sym_object(path, module)])
        }
    }
}

fn sym_object(path: &Path, module: sym::Module) -> DebugInfo {
    DebugInfo {
        path: path.to_path_buf(),
        module_name: module.name,
        debug_id: module.id,
        kind: ObjectKind::BreakpadSym,
        arch: module.arch,
        slice: None,
    }
}

fn io_error(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extensions.iter().any(|e| extension.eq_ignore_ascii_case(e)))
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_ids() {
        assert_eq!(
            normalize_id(" e546b55b-6d21-4e86-871b-40ac35cd0d46 "),
            "E546B55B6D214E86871B40AC35CD0D46"
        );
    }

    #[test]
    fn kinds_from_extensions() {
        let kind = |name: &str| kind_from_extension(Path::new(name));
        assert_eq!(kind("a/bugsplat.PDB"), Some(ObjectKind::Pdb));
        assert_eq!(kind("bugsplat.dll"), Some(ObjectKind::Pe));
        assert_eq!(kind("eboot.self"), Some(ObjectKind::Elf));
        assert_eq!(kind("libc++_shared.so.sym"), Some(ObjectKind::BreakpadSym));
        assert_eq!(kind("bugsplat.app.dSYM"), Some(ObjectKind::MachO));
        assert_eq!(kind("libfoo.so"), None);
        assert_eq!(kind("bugsplat"), None);
    }

    #[test]
    fn bundles_are_named_by_extension() {
        assert!(is_bundle(Path::new("build/bugsplat.app.dSYM")));
        assert!(is_bundle(Path::new("bugsplat.xcarchive")));
        assert!(!is_bundle(Path::new("libggcurl.dylib")));
        assert!(!is_bundle(Path::new("build")));
    }
}

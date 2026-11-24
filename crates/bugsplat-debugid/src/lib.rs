//! Debug ids from PE, PDB, ELF, Mach-O, dSYM and Breakpad .sym files.
//! See https://github.com/BugSplat-Git/bugsplat-cli/issues/5
//!
//! The ids match what BugSplat's npm packages (pdb-guid, macho-uuid, elfy, symbol-upload)
//! produce, because symbol uploads are keyed on them.

use std::path::{Path, PathBuf};

use serde::Serialize;

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

/// Every object in `path`, which may be a file or a `.dSYM`, `.app` or `.xcarchive` bundle.
pub fn read(path: &Path) -> Result<Vec<DebugInfo>> {
    Err(Error::Unsupported(path.to_path_buf()))
}

/// The form ids are compared in: uppercase, without dashes or surrounding space.
pub fn normalize_id(id: &str) -> String {
    id.trim()
        .chars()
        .filter(|c| *c != '-')
        .flat_map(char::to_uppercase)
        .collect()
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
}

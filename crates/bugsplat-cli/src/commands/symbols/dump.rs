//! `symbols dump`, and the `.sym` generation behind `symbols upload --dump-syms`.
//! https://github.com/BugSplat-Git/bugsplat-cli/issues/11

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::{Path, PathBuf};

use bugsplat_core::Error;
use clap::Args as ClapArgs;
use serde::Serialize;

use crate::context::Context;
use crate::output;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Binaries or debug files to convert: ELF, Mach-O, PE or PDB files, or .dSYM bundles
    #[arg(required = true, value_name = "FILE")]
    pub files: Vec<PathBuf>,
    /// Folder to write the .sym files to
    #[arg(short = 'o', long, value_name = "DIR", default_value = ".")]
    pub output: PathBuf,
}

/// One generated `.sym` file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dumped {
    pub path: PathBuf,
    /// The file the symbols came from.
    pub source: PathBuf,
    /// The name BugSplat stores the file under, as `symbols info` prints it.
    pub module_name: String,
    pub debug_id: String,
    pub os: String,
    pub arch: String,
}

/// `dump-syms` is a cargo feature so a release target that can't build dump_syms can go
/// without it.
const UNAVAILABLE: &str =
    "This build of bugsplat was made without dump_syms, so it can't generate .sym files.";

pub async fn run(args: Args, ctx: &Context) -> anyhow::Result<()> {
    if !cfg!(feature = "dump-syms") {
        return Err(Error::Other(UNAVAILABLE.to_string()).into());
    }
    let companions = Companions::new(&args.files);
    let mut dumped = Vec::new();
    let mut failures = Vec::new();
    let mut written = HashMap::new();
    for file in &args.files {
        match dump_into(file, &companions, &args.output, &mut written) {
            Ok(found) => dumped.extend(found),
            Err(error) => failures.push(error),
        }
    }

    if !dumped.is_empty() || failures.is_empty() {
        if ctx.config.is_json() {
            output::print_json(&dumped)?;
        } else {
            print!("{}", text(&dumped));
        }
    }

    match failures.len() {
        0 => Ok(()),
        1 if args.files.len() == 1 => Err(Error::Other(failures.remove(0)).into()),
        failed => {
            for failure in &failures {
                eprintln!("error: {failure}");
            }
            Err(Error::Other(format!(
                "{failed} of {} files could not be converted",
                args.files.len()
            ))
            .into())
        }
    }
}

/// Generates a `.sym` file from each of `paths`, each in a folder of its own under `dir`, for
/// `symbols upload --dump-syms`. A file `dump_syms` can't convert is warned about and left
/// out, as `symbol-upload --dumpSyms` does.
pub(super) fn dump_for_upload(
    paths: &[PathBuf],
    dir: &Path,
    quiet: bool,
) -> anyhow::Result<Vec<Dumped>> {
    if !cfg!(feature = "dump-syms") {
        return Err(Error::Other(UNAVAILABLE.to_string()).into());
    }
    let companions = Companions::new(paths);
    let mut dumped: Vec<Dumped> = Vec::new();
    let mut folders = 0usize;
    for path in paths {
        let found = dump(path, &companions, |_| {
            folders += 1;
            Ok(dir.join(folders.to_string()))
        });
        match found {
            Ok(found) => {
                if !quiet {
                    for sym in &found {
                        eprintln!(
                            "generated {} from {} ({})",
                            file_name(&sym.path),
                            sym.source.display(),
                            sym.arch
                        );
                    }
                }
                dumped.extend(found);
            }
            Err(error) => {
                if !quiet {
                    eprintln!("warning: could not generate a .sym file: {error}");
                }
            }
        }
    }
    if dumped.is_empty() {
        return Err(Error::Other(
            "No .sym files could be generated from the files found.".to_string(),
        )
        .into());
    }
    Ok(one_per_module(dumped))
}

/// A glob like `**/*.{pdb,exe,dll}` selects a binary and its PDB, and both yield the module's
/// `.sym` file, which BugSplat stores once. Each is generated from both halves when it finds
/// the other; when one doesn't, it is the smaller, so the larger is kept.
fn one_per_module(dumped: Vec<Dumped>) -> Vec<Dumped> {
    let size = |sym: &Dumped| std::fs::metadata(&sym.path).map_or(0, |metadata| metadata.len());
    let key = |sym: &Dumped| {
        let name = file_name(&sym.path);
        // Only Windows module names ignore case: `BugSplat.pdb` and `bugsplat.pdb` are one
        // module, `libFoo.so` and `libfoo.so` are two.
        if sym.os.eq_ignore_ascii_case("windows") {
            (
                sym.debug_id.to_lowercase(),
                sym.module_name.to_lowercase(),
                name.to_lowercase(),
            )
        } else {
            (sym.debug_id.clone(), sym.module_name.clone(), name)
        }
    };
    let mut kept: Vec<Dumped> = Vec::new();
    let mut positions = HashMap::new();
    for sym in dumped {
        match positions.entry(key(&sym)) {
            Entry::Occupied(position) => {
                let other = &mut kept[*position.get()];
                if size(&sym) > size(other) {
                    *other = sym;
                }
            }
            Entry::Vacant(position) => {
                position.insert(kept.len());
                kept.push(sym);
            }
        }
    }
    kept
}

/// Writes the `.sym` files of one input into `output`. A name that would land on a file this
/// run already wrote, or that two architectures of the input share, goes in a folder named
/// after its debug id instead, so the file keeps the name it has to be uploaded under.
/// `written` maps each file written so far to the input it came from.
fn dump_into(
    input: &Path,
    companions: &Companions,
    output: &Path,
    written: &mut HashMap<PathBuf, PathBuf>,
) -> Result<Vec<Dumped>, String> {
    dump(input, companions, |sym| {
        let key =
            |folder: &Path| PathBuf::from(fold_case(&folder.join(sym.file_name).to_string_lossy()));
        let mut folder = output.to_path_buf();
        if sym.shared || written.contains_key(&key(&folder)) {
            folder.push(sym.debug_id);
        }
        // Same name and same debug id: the same module from another input, such as a dSYM
        // and the binary it belongs to. Replacing the first file could swap full symbols for
        // a stripped binary's.
        if let Some(earlier) = written.get(&key(&folder)) {
            return Err(format!(
                "{}: its {} would replace the one written from {}; convert them into different folders",
                input.display(),
                sym.file_name,
                earlier.display()
            ));
        }
        written.insert(key(&folder), input.to_path_buf());
        Ok(folder)
    })
}

/// The form two file names are compared in: macOS and Windows file names ignore case, and
/// elsewhere `libFoo.so.sym` and `libfoo.so.sym` are two files.
fn fold_case(name: &str) -> String {
    if cfg!(any(target_os = "macos", windows)) {
        name.to_lowercase()
    } else {
        name.to_string()
    }
}

/// What the folder of a `.sym` file is chosen from.
struct Pending<'a> {
    file_name: &'a str,
    debug_id: &'a str,
    /// Another architecture of the same input has this file name too.
    shared: bool,
}

#[cfg(feature = "dump-syms")]
type Companions = bugsplat_dumpsyms::Companions;

#[cfg(feature = "dump-syms")]
fn dump(
    input: &Path,
    companions: &Companions,
    mut folder: impl FnMut(Pending) -> Result<PathBuf, String>,
) -> Result<Vec<Dumped>, String> {
    let all = bugsplat_dumpsyms::read_with(input, companions).map_err(|error| error.to_string())?;
    let mut dumped = Vec::new();
    for symbols in &all {
        let same_name = |other: &&bugsplat_dumpsyms::Symbols| {
            fold_case(&other.file_name) == fold_case(&symbols.file_name)
        };
        let folder = folder(Pending {
            file_name: &symbols.file_name,
            debug_id: &symbols.module.debug_id,
            shared: all.iter().filter(same_name).count() > 1,
        })?;
        let path = symbols
            .write_to(&folder)
            .map_err(|error| error.to_string())?;
        // Read back the way an upload reads it, so the id and name printed here are the ones
        // BugSplat stores the file under.
        let stored = bugsplat_debugid::read(&path)
            .ok()
            .and_then(|objects| objects.into_iter().next())
            .ok_or_else(|| format!("{}: could not read the MODULE line back", path.display()))?;
        dumped.push(Dumped {
            path,
            source: symbols.source.clone(),
            module_name: stored.module_name,
            debug_id: stored.debug_id,
            os: symbols.module.os.clone(),
            arch: symbols.module.arch.clone(),
        });
    }
    Ok(dumped)
}

#[cfg(not(feature = "dump-syms"))]
struct Companions;

#[cfg(not(feature = "dump-syms"))]
impl Companions {
    fn new(_: &[PathBuf]) -> Self {
        Self
    }
}

#[cfg(not(feature = "dump-syms"))]
fn dump(
    _: &Path,
    _: &Companions,
    _: impl FnMut(Pending) -> Result<PathBuf, String>,
) -> Result<Vec<Dumped>, String> {
    Err(UNAVAILABLE.to_string())
}

fn text(dumped: &[Dumped]) -> String {
    let mut out = String::new();
    for (index, sym) in dumped.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&format!("{}\n", sym.path.display()));
        let mut field = |name: &str, value: &str| out.push_str(&format!("  {name:<9} {value}\n"));
        field("source", &sym.source.display().to_string());
        field("module", &sym.module_name);
        field("debug id", &sym.debug_id);
        field("os", &sym.os);
        field("arch", &sym.arch);
    }
    out
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn sym(dir: &Path, folder: &str, name: &str, module: &str, os: &str, bytes: usize) -> Dumped {
        let folder = dir.join(folder);
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join(name);
        fs::write(&path, vec![b'x'; bytes]).unwrap();
        Dumped {
            path,
            source: PathBuf::from(folder.file_name().unwrap()),
            module_name: module.to_string(),
            debug_id: "ABC1".to_string(),
            os: os.to_string(),
            arch: "x86_64".to_string(),
        }
    }

    fn sources(kept: &[Dumped]) -> Vec<String> {
        kept.iter()
            .map(|sym| sym.source.display().to_string())
            .collect()
    }

    #[test]
    fn the_larger_sym_file_of_a_module_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let kept = one_per_module(vec![
            sym(dir, "1", "App.sym", "App.pdb", "windows", 10),
            sym(dir, "2", "other.sym", "other.pdb", "windows", 5),
            sym(dir, "3", "app.sym", "app.pdb", "windows", 20),
            sym(dir, "4", "App.sym", "App.pdb", "windows", 20),
        ]);
        assert_eq!(sources(&kept), ["3", "2"]);
    }

    #[test]
    fn only_windows_modules_are_matched_whatever_the_case() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let kept = one_per_module(vec![
            sym(dir, "1", "libFoo.so.sym", "libFoo.so", "Linux", 10),
            sym(dir, "2", "libfoo.so.sym", "libfoo.so", "Linux", 20),
            sym(dir, "3", "libfoo.so.sym", "libfoo.so", "Linux", 30),
        ]);
        assert_eq!(sources(&kept), ["1", "3"]);
    }
}

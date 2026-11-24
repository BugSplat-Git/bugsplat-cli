//! `symbols dump`, and the `.sym` generation behind `symbols upload --dump-syms`.
//! https://github.com/BugSplat-Git/bugsplat-cli/issues/11

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
    let mut dumped = Vec::new();
    let mut failures = Vec::new();
    let mut written = Vec::new();
    for file in &args.files {
        match dump_into(file, &args.output, &mut written) {
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
    let mut dumped: Vec<Dumped> = Vec::new();
    let mut folders = 0usize;
    for path in paths {
        let found = dump(path, |_| {
            folders += 1;
            dir.join(folders.to_string())
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
    Ok(dumped)
}

/// Writes the `.sym` files of one input into `output`. A name that would land on a file this
/// run already wrote, or that two architectures of the input share, goes in a folder named
/// after its debug id instead, so the file keeps the name it has to be uploaded under.
fn dump_into(
    input: &Path,
    output: &Path,
    written: &mut Vec<String>,
) -> Result<Vec<Dumped>, String> {
    dump(input, |sym| {
        // Compared in lowercase: macOS and Windows file names ignore case.
        let name = sym.file_name.to_lowercase();
        let taken = sym.shared || written.contains(&name);
        written.push(name);
        if taken {
            output.join(sym.debug_id)
        } else {
            output.to_path_buf()
        }
    })
}

/// What the folder of a `.sym` file is chosen from.
struct Pending<'a> {
    file_name: &'a str,
    debug_id: &'a str,
    /// Another architecture of the same input has this file name too.
    shared: bool,
}

#[cfg(feature = "dump-syms")]
fn dump(input: &Path, mut folder: impl FnMut(Pending) -> PathBuf) -> Result<Vec<Dumped>, String> {
    let all = bugsplat_dumpsyms::read(input).map_err(|error| error.to_string())?;
    let mut dumped = Vec::new();
    for symbols in &all {
        let same_name = |other: &&bugsplat_dumpsyms::Symbols| {
            other.file_name.to_lowercase() == symbols.file_name.to_lowercase()
        };
        let folder = folder(Pending {
            file_name: &symbols.file_name,
            debug_id: &symbols.module.debug_id,
            shared: all.iter().filter(same_name).count() > 1,
        });
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
fn dump(_: &Path, _: impl FnMut(Pending) -> PathBuf) -> Result<Vec<Dumped>, String> {
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

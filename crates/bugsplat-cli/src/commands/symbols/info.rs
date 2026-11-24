//! `symbols info`. https://github.com/BugSplat-Git/bugsplat-cli/issues/5

use std::path::Path;

use bugsplat_core::Error;
use bugsplat_debugid::{DebugInfo, ObjectKind};
use clap::Args as ClapArgs;

use crate::context::Context;
use crate::output;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Files or bundles (.dSYM, .app, .xcarchive) to inspect
    #[arg(required = true, value_name = "FILE")]
    pub files: Vec<std::path::PathBuf>,
}

pub async fn run(args: Args, ctx: &Context) -> anyhow::Result<()> {
    let mut objects = Vec::new();
    let mut failures = Vec::new();
    for file in &args.files {
        match bugsplat_debugid::read(file) {
            Ok(found) => {
                if !ctx.config.quiet
                    && let Some(note) = misnamed(file, &found)
                {
                    eprintln!("note: {note}");
                }
                objects.extend(found);
            }
            Err(err) => failures.push(describe(file, err)),
        }
    }

    if !objects.is_empty() || failures.is_empty() {
        if ctx.config.is_json() {
            output::print_json(&objects)?;
        } else {
            print!("{}", text(&objects));
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
                "{failed} of {} files could not be read",
                args.files.len()
            ))
            .into())
        }
    }
}

fn describe(file: &Path, err: bugsplat_debugid::Error) -> String {
    match err {
        bugsplat_debugid::Error::Unsupported(_) if file.is_dir() => {
            if bugsplat_debugid::is_bundle(file) {
                format!("{}: no Mach-O files in this bundle", file.display())
            } else {
                format!(
                    "{}: is a folder; pass files or a .dSYM, .app, .framework, .bundle or .xcarchive bundle",
                    file.display()
                )
            }
        }
        err => err.to_string(),
    }
}

/// A note for a file whose contents are one kind and whose extension says another, since
/// `symbol-upload` goes by the extension and would have read the file differently.
fn misnamed(file: &Path, found: &[DebugInfo]) -> Option<String> {
    let expected = bugsplat_debugid::kind_from_extension(file)?;
    let actual = found.first()?.kind;
    let same =
        expected == actual || (expected == ObjectKind::Pdb && actual == ObjectKind::PortablePdb);
    (!same).then(|| {
        format!(
            "{}: read as {}; its name suggests {}",
            file.display(),
            label(actual),
            label(expected)
        )
    })
}

fn label(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Pe => "PE",
        ObjectKind::Pdb => "PDB",
        ObjectKind::PortablePdb => "portable PDB",
        ObjectKind::Elf => "ELF",
        ObjectKind::MachO => "Mach-O",
        ObjectKind::BreakpadSym => "Breakpad .sym",
    }
}

fn text(objects: &[DebugInfo]) -> String {
    let mut out = String::new();
    for (index, object) in objects.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&format!("{}\n", object.path.display()));
        let mut field = |name: &str, value: &str| out.push_str(&format!("  {name:<9} {value}\n"));
        field("module", &object.module_name);
        field(
            "debug id",
            if object.debug_id.is_empty() {
                "(none)"
            } else {
                &object.debug_id
            },
        );
        field("kind", label(object.kind));
        if let Some(arch) = &object.arch {
            field("arch", arch);
        }
        if let Some(slice) = &object.slice {
            field(
                "slice",
                &format!("{} bytes at offset {}", slice.size, slice.offset),
            );
        }
    }
    out
}

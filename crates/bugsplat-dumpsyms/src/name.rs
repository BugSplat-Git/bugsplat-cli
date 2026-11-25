//! `.sym` file names as `symbol-upload --dumpSyms` chooses them (its `src/sym.ts`). BugSplat's
//! stack walker looks a module's symbols up as `<module>/<debug id>/<this name>`.

use bugsplat_debugid::normalize_sym_module_name;

/// The name of the `.sym` file generated from `input_file_name`, whose `MODULE` line names
/// `module_name`.
///
/// A module that is a `.pdb` names the file, because a PDB can be named differently from its
/// binary (Cargo: `my-app.exe`, `my_app.pdb`). Any other file is named after the input:
/// a trailing `.dSYM` or `.debug` is dropped, a name that ends in `.dylib` or contains `.so`
/// is then kept whole, and any other name is cut at its first dot.
pub fn sym_file_name(input_file_name: &str, module_name: &str) -> String {
    debug_file_sym_name(&normalize_sym_module_name(module_name))
        .unwrap_or_else(|| normalized_sym_file_name(input_file_name))
}

/// symbol-upload's `getSymFileNameForDebugFile`.
fn debug_file_sym_name(module_name: &str) -> Option<String> {
    // A drive letter counts as a separator: on Windows `C:app.sym` would be written outside
    // the output folder.
    let name = module_name.rsplit(['/', '\\', ':']).next()?;
    let stem = strip_suffix_ignore_case(name, ".pdb").filter(|stem| !stem.is_empty())?;
    Some(format!("{stem}.sym"))
}

/// symbol-upload's `getNormalizedSymFileName`.
fn normalized_sym_file_name(file_name: &str) -> String {
    let mut name = file_name;
    for extension in [".dsym", ".debug"] {
        name = strip_suffix_ignore_case(name, extension).unwrap_or(name);
    }
    if name.ends_with(".dylib") || name.contains(".so") {
        return format!("{name}.sym");
    }
    format!("{}.sym", name.split('.').next().unwrap_or_default())
}

fn strip_suffix_ignore_case<'a>(name: &'a str, suffix: &str) -> Option<&'a str> {
    let cut = name.len().checked_sub(suffix.len())?;
    name.as_bytes()[cut..]
        .eq_ignore_ascii_case(suffix.as_bytes())
        .then(|| &name[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cases of symbol-upload's `spec/sym.spec.ts`.
    #[test]
    fn names_a_file_after_its_input_as_symbol_upload_does() {
        for (input, expected) in [
            ("liba.dylib.dSYM", "liba.dylib.sym"),
            ("Electron.app.dSYM", "Electron.sym"),
            ("liba.so", "liba.so.sym"),
            ("liba.so.0", "liba.so.0.sym"),
            ("windows.pdb", "windows.sym"),
            ("linux.so.2.debug", "linux.so.2.sym"),
            ("linux.debug", "linux.sym"),
            ("bugsplat", "bugsplat.sym"),
            ("hello.elf", "hello.sym"),
            ("my.sound.dll", "my.sound.dll.sym"),
            ("a.DSYM.Debug", "a.sym"),
        ] {
            assert_eq!(normalized_sym_file_name(input), expected, "{input}");
        }
    }

    #[test]
    fn names_a_file_after_its_pdb_as_symbol_upload_does() {
        for (module, expected) in [
            ("clockwork_desktop.pdb", Some("clockwork_desktop.sym")),
            ("electron.exe.pdb", Some("electron.exe.sym")),
            ("BugSplat.PDB", Some("BugSplat.sym")),
            ("libc++_shared.so", None),
            ("liba.dylib", None),
            (".pdb", None),
            ("C:\\build\\app.pdb", Some("app.sym")),
            ("C:app.pdb", Some("app.sym")),
        ] {
            assert_eq!(debug_file_sym_name(module).as_deref(), expected, "{module}");
        }
    }

    #[test]
    fn the_pdb_on_the_module_line_wins_over_the_input_name() {
        assert_eq!(
            sym_file_name("bssndrpt.exe", "BsSndRpt.pdb"),
            "BsSndRpt.sym"
        );
        assert_eq!(sym_file_name("my-app.exe", "my_app.pdb"), "my_app.sym");
        // symbol-upload normalizes the module name first, and `.PDB` is not `.pdb` to it.
        assert_eq!(
            sym_file_name("bugsplat.dll", "BugSplat.PDB"),
            "bugsplat.sym"
        );
        assert_eq!(
            sym_file_name("libfoo.so.1", "libfoo.so.1"),
            "libfoo.so.1.sym"
        );
        assert_eq!(sym_file_name("App.app.dSYM", "App"), "App.sym");
    }
}

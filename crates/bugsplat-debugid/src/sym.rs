//! Breakpad `.sym` ids and module names as `symbol-upload`'s `getSymFileInfo` produces them.

use crate::data::{Data, bytes_up_to};

/// A `MODULE` line is a few hundred bytes at most; a file with no newline this far in is
/// not a `.sym` file, and reading on would pull the whole file in.
const MAX_FIRST_LINE: u64 = 64 * 1024;

pub(crate) struct Module {
    pub id: String,
    pub name: String,
    pub arch: Option<String>,
}

pub(crate) fn read(data: &Data) -> Option<Module> {
    let line = first_line(data);
    let (id, name) = parse_first_line(&line)?;
    // MODULE <os> <arch> <id> <name>
    let mut fields = line.split_whitespace();
    let arch = (fields.next() == Some("MODULE"))
        .then(|| fields.nth(1))
        .flatten()
        .map(str::to_string);
    Some(Module {
        id: id.to_string(),
        name: normalize_sym_module_name(name),
        arch,
    })
}

fn first_line(data: &Data) -> String {
    let mut line = Vec::new();
    while (line.len() as u64) < MAX_FIRST_LINE {
        let chunk = bytes_up_to(data, line.len() as u64, 4096);
        if chunk.is_empty() {
            break;
        }
        if let Some(newline) = chunk.iter().position(|b| *b == b'\n') {
            line.extend_from_slice(&chunk[..newline]);
            break;
        }
        line.extend_from_slice(chunk);
    }
    String::from_utf8_lossy(&line).into_owned()
}

/// The first match of symbol-upload's `/([0-9a-fA-F]{33,34})\s+(.*)$/gm` in `line`: a run of
/// 33 or 34 hex digits that ends at whitespace, then the rest of the line. The id keeps its
/// case.
fn parse_first_line(line: &str) -> Option<(&str, &str)> {
    for (start, _) in line.char_indices() {
        let rest = &line[start..];
        let hex_digits = rest
            .bytes()
            .take(34)
            .take_while(u8::is_ascii_hexdigit)
            .count();
        if hex_digits < 33 {
            continue;
        }
        let (id, after) = rest.split_at(hex_digits);
        if !after.starts_with(is_js_whitespace) {
            continue;
        }
        let name = after.trim_start_matches(is_js_whitespace);
        let end = name.find(is_js_line_terminator).unwrap_or(name.len());
        return Some((id, &name[..end]));
    }
    None
}

fn is_js_whitespace(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

fn is_js_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// The module name BugSplat stores a `.sym` file under, given the name on its `MODULE` line.
/// This is symbol-upload's `getNormalizedSymModuleName`: a trailing `.dSYM` or `.debug` is
/// dropped; a name that ends in `.pdb` or `.dylib`, or contains `.so`, is then kept as it is;
/// any other name is cut at its first dot.
pub fn normalize_sym_module_name(name: &str) -> String {
    let mut name = name;
    for extension in [".dsym", ".debug"] {
        let cut = name.len().saturating_sub(extension.len());
        if name.as_bytes()[cut..].eq_ignore_ascii_case(extension.as_bytes()) {
            name = &name[..cut];
        }
    }
    if name.ends_with(".pdb") || name.contains(".so") || name.ends_with(".dylib") {
        return name.to_string();
    }
    name.split('.').next().unwrap_or_default().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_module_names() {
        for (name, expected) in [
            ("liba.dylib.dSYM", "liba.dylib"),
            ("Electron.app.dSYM", "Electron"),
            ("liba.so", "liba.so"),
            ("liba.so.0", "liba.so.0"),
            ("windows.pdb", "windows.pdb"),
            ("linux.so.2.debug", "linux.so.2"),
            ("linux.debug", "linux"),
            ("Electron Helper (GPU)", "Electron Helper (GPU)"),
            ("libc++_shared.so", "libc++_shared.so"),
            ("my.sound.dll", "my.sound.dll"),
            ("crasher.exe", "crasher"),
            ("WINDOWS.PDB", "WINDOWS"),
            ("a.DSYM.debug", "a"),
            ("x.Debug", "x"),
            ("", ""),
        ] {
            assert_eq!(normalize_sym_module_name(name), expected, "{name}");
        }
    }

    #[test]
    fn parses_33_and_34_digit_ids() {
        assert_eq!(
            parse_first_line(
                "MODULE Linux arm64 83AE77A7796BDF7DCE8CE3564B3B5D650 libc++_shared.so"
            ),
            Some(("83AE77A7796BDF7DCE8CE3564B3B5D650", "libc++_shared.so"))
        );
        assert_eq!(
            parse_first_line("MODULE windows x86 9DD7CE5C705C45B09BEE297B84B5B9881c windows.pdb\r"),
            Some(("9DD7CE5C705C45B09BEE297B84B5B9881c", "windows.pdb"))
        );
    }

    #[test]
    fn keeps_spaces_in_the_module_name() {
        assert_eq!(
            parse_first_line(
                "MODULE mac arm64 4C4C445055553144A10453F91A3BB4950 Electron Helper (GPU)"
            ),
            Some(("4C4C445055553144A10453F91A3BB4950", "Electron Helper (GPU)"))
        );
    }

    #[test]
    fn the_name_starts_after_the_whitespace_and_runs_to_the_line_end() {
        assert_eq!(
            parse_first_line(
                "MODULE Linux x86_64 83AE77A7796BDF7DCE8CE3564B3B5D650 \t  two  words "
            ),
            Some(("83AE77A7796BDF7DCE8CE3564B3B5D650", "two  words "))
        );
    }

    #[test]
    fn matches_what_the_regex_matches() {
        assert_eq!(parse_first_line("MODULE Linux arm64 0 a"), None);
        assert_eq!(
            parse_first_line("MODULE Linux arm64 83AE77A7796BDF7DCE8CE3564B3B5D65 a"),
            None
        );
        assert_eq!(
            parse_first_line("MODULE Linux arm64 83AE77A7796BDF7DCE8CE3564B3B5D650"),
            None
        );
        // A longer run of digits matches at its last 34, as the unanchored regex does.
        assert_eq!(
            parse_first_line("MODULE Linux arm64 FFFF83AE77A7796BDF7DCE8CE3564B3B5D6500 a"),
            Some(("83AE77A7796BDF7DCE8CE3564B3B5D6500", "a"))
        );
    }
}

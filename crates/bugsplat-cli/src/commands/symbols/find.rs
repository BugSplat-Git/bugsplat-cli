//! `symbols find`. https://github.com/BugSplat-Git/bugsplat-cli/issues/6

use std::path::PathBuf;

use bugsplat_core::Error;
use bugsplat_symbols::{Match, Search, SearchResult};
use clap::Args as ClapArgs;

use crate::context::Context;
use crate::output::print_json;

#[derive(Debug, ClapArgs)]
#[command(after_help = "\
With no --debug-id or --name, every symbol file found is listed. With both, a file matching \
either is reported. Exits with 1 when a requested id or name was not found.

Examples:
  bugsplat symbols find --debug-id E546B55B6D214E86871B40AC35CD0D461 --path ./build --path ~/symbols
  bugsplat symbols find --name myapp.pdb --path .
  bugsplat symbols find --path ./build")]
pub struct Args {
    /// Debug id to look for, in any case, with or without dashes; repeat for several
    #[arg(long = "debug-id", value_name = "ID")]
    pub debug_ids: Vec<String>,
    /// File or module name to look for, in any case, with globs; repeat for several
    #[arg(long = "name", value_name = "NAME")]
    pub names: Vec<String>,
    /// Folder to search; repeat for several [default: .]
    #[arg(long = "path", value_name = "DIR")]
    pub paths: Vec<PathBuf>,
    /// How many levels below each --path to search; 1 is only the files directly in it
    #[arg(long, value_name = "N")]
    pub max_depth: Option<usize>,
    /// Also search .git, node_modules and the other folders skipped by default
    #[arg(long)]
    pub no_ignore: bool,
    /// Follow symbolic links
    #[arg(long)]
    pub follow: bool,
}

pub async fn run(args: Args, ctx: &Context) -> anyhow::Result<()> {
    let search = search(args)?;
    let result = tokio::task::spawn_blocking(move || {
        bugsplat_symbols::find(&search, bugsplat_debugid::read)
    })
    .await?
    .map_err(|err| Error::Usage(err.to_string()))?;

    if !ctx.config.quiet {
        for warning in &result.warnings {
            eprintln!("warning: {warning}");
        }
    }

    if ctx.config.is_json() {
        print_json(&result)?;
    } else {
        print!("{}", render(&result));
        if result.matches.is_empty() && result.is_complete() && !ctx.config.quiet {
            eprintln!("No symbol files found.");
        }
    }

    match not_found(&result) {
        Some(message) => Err(Error::Other(message).into()),
        None => Ok(()),
    }
}

fn search(args: Args) -> Result<Search, Error> {
    for id in &args.debug_ids {
        let normalized = bugsplat_debugid::normalize_id(id);
        if normalized.is_empty() || !normalized.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::Usage(format!(
                "`{id}` is not a debug id: expected hexadecimal digits, with or without dashes"
            )));
        }
    }
    if args.names.iter().any(|name| name.is_empty()) {
        return Err(Error::Usage("--name can't be empty".to_string()));
    }
    Ok(Search {
        paths: args.paths,
        debug_ids: args.debug_ids,
        names: args.names,
        max_depth: args.max_depth,
        no_ignore: args.no_ignore,
        follow_links: args.follow,
    })
}

/// One line per match, in columns, then one line per requested id or name without a match.
fn render(result: &SearchResult) -> String {
    let rows: Vec<Vec<String>> = result.matches.iter().map(row).collect();
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();

    let mut out = String::new();
    for row in &rows {
        let last = row.len() - 1;
        for (column, cell) in row.iter().enumerate() {
            out.push_str(cell);
            if column < last {
                let padding = widths[column] - cell.chars().count() + 2;
                out.extend(std::iter::repeat_n(' ', padding));
            }
        }
        out.push('\n');
    }
    for id in &result.missing_ids {
        out.push_str(&format!("Not found: debug id {id}\n"));
    }
    for name in &result.missing_names {
        out.push_str(&format!("Not found: name {name}\n"));
    }
    out
}

fn row(found: &Match) -> Vec<String> {
    let info = &found.info;
    let kind = serde_json::to_value(info.kind)
        .ok()
        .and_then(|kind| kind.as_str().map(str::to_string))
        .unwrap_or_default();
    let mut row = vec![
        info.path.display().to_string(),
        info.module_name.clone(),
        if info.debug_id.is_empty() {
            "-".to_string()
        } else {
            info.debug_id.clone()
        },
        match &info.arch {
            Some(arch) => format!("{kind} {arch}"),
            None => kind,
        },
    ];
    let matched: Vec<String> = found
        .matched_ids
        .iter()
        .map(|id| format!("id {id}"))
        .chain(
            found
                .matched_names
                .iter()
                .map(|name| format!("name {name}")),
        )
        .collect();
    if !matched.is_empty() {
        row.push(format!("matched {}", matched.join(", ")));
    }
    row
}

fn not_found(result: &SearchResult) -> Option<String> {
    let count = |count: usize, what: &str| match count {
        0 => None,
        1 => Some(format!("1 {what}")),
        _ => Some(format!("{count} {what}s")),
    };
    let parts: Vec<String> = [
        count(result.missing_ids.len(), "debug id"),
        count(result.missing_names.len(), "name"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        None
    } else {
        Some(format!("{} not found", parts.join(" and ")))
    }
}

#[cfg(test)]
mod tests {
    use bugsplat_symbols::{DebugInfo, ObjectKind, Slice};

    use super::*;

    fn args() -> Args {
        Args {
            debug_ids: Vec::new(),
            names: Vec::new(),
            paths: Vec::new(),
            max_depth: None,
            no_ignore: false,
            follow: false,
        }
    }

    fn found(path: &str, module: &str, id: &str, kind: ObjectKind) -> Match {
        Match {
            info: DebugInfo {
                path: PathBuf::from(path),
                module_name: module.to_string(),
                debug_id: id.to_string(),
                kind,
                arch: None,
                slice: None,
            },
            bundle: None,
            matched_ids: Vec::new(),
            matched_names: Vec::new(),
        }
    }

    #[test]
    fn arguments_become_a_search() {
        let search = search(Args {
            debug_ids: vec!["e546b55b-6d21-4e86-871b-40ac35cd0d46".to_string()],
            names: vec!["*.pdb".to_string()],
            paths: vec![PathBuf::from("build")],
            max_depth: Some(3),
            no_ignore: true,
            follow: true,
        })
        .unwrap();

        assert_eq!(search.debug_ids, ["e546b55b-6d21-4e86-871b-40ac35cd0d46"]);
        assert_eq!(search.names, ["*.pdb"]);
        assert_eq!(search.paths, [PathBuf::from("build")]);
        assert_eq!(search.max_depth, Some(3));
        assert!(search.no_ignore);
        assert!(search.follow_links);
    }

    #[test]
    fn a_debug_id_must_be_hexadecimal() {
        for id in ["", "  ", "---", "myapp.pdb", "{E546B55B}"] {
            let err = search(Args {
                debug_ids: vec![id.to_string()],
                ..args()
            })
            .unwrap_err();
            assert_eq!(err.exit_code(), 2, "{id:?}");
        }
    }

    #[test]
    fn a_name_cannot_be_empty() {
        let err = search(Args {
            names: vec![String::new()],
            ..args()
        })
        .unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn listing_prints_path_module_id_and_kind_in_columns() {
        let mut slice = found("build/MyApp", "MyApp", "AAAA", ObjectKind::MachO);
        slice.info.arch = Some("arm64".to_string());
        slice.info.slice = Some(Slice {
            offset: 4096,
            size: 4096,
        });
        let result = SearchResult {
            matches: vec![
                slice,
                found("build/myapp.pdb", "myapp.pdb", "BBBBBBBB", ObjectKind::Pdb),
                found("build/stripped", "stripped", "", ObjectKind::Elf),
            ],
            ..SearchResult::default()
        };

        assert_eq!(
            render(&result),
            "build/MyApp      MyApp      AAAA      mach-o arm64\n\
             build/myapp.pdb  myapp.pdb  BBBBBBBB  pdb\n\
             build/stripped   stripped   -         elf\n"
        );
    }

    #[test]
    fn a_match_ends_with_what_it_matched_and_missing_ids_come_last() {
        let mut by_id = found("a.pdb", "a.pdb", "AAAA", ObjectKind::Pdb);
        by_id.matched_ids = vec!["aaaa".to_string()];
        let mut by_both = found("b.exe", "b.exe", "BBBB", ObjectKind::Pe);
        by_both.matched_ids = vec!["BBBB".to_string()];
        by_both.matched_names = vec!["*.exe".to_string()];
        let result = SearchResult {
            matches: vec![by_id, by_both],
            missing_ids: vec!["CCCC".to_string()],
            missing_names: vec!["*.so".to_string()],
            warnings: Vec::new(),
        };

        assert_eq!(
            render(&result),
            "a.pdb  a.pdb  AAAA  pdb  matched id aaaa\n\
             b.exe  b.exe  BBBB  pe   matched id BBBB, name *.exe\n\
             Not found: debug id CCCC\n\
             Not found: name *.so\n"
        );
    }

    #[test]
    fn nothing_found_prints_nothing() {
        assert_eq!(render(&SearchResult::default()), "");
    }

    #[test]
    fn the_failure_says_how_many_are_missing() {
        let missing = |ids: &[&str], names: &[&str]| {
            not_found(&SearchResult {
                missing_ids: ids.iter().map(|id| id.to_string()).collect(),
                missing_names: names.iter().map(|name| name.to_string()).collect(),
                ..SearchResult::default()
            })
        };

        assert_eq!(missing(&[], &[]), None);
        assert_eq!(missing(&["A"], &[]).unwrap(), "1 debug id not found");
        assert_eq!(missing(&["A", "B"], &[]).unwrap(), "2 debug ids not found");
        assert_eq!(missing(&[], &["a"]).unwrap(), "1 name not found");
        assert_eq!(
            missing(&["A", "B"], &["a"]).unwrap(),
            "2 debug ids and 1 name not found"
        );
    }
}

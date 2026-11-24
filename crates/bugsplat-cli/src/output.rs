//! Printing and prompting helpers shared by the commands.
#![allow(dead_code)]

use std::io::{IsTerminal, Write};

use bugsplat_core::Error;
use serde::Serialize;

pub fn print_json<T: Serialize + ?Sized>(value: &T) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Asks before a change that can't be undone from the CLI. `assume_yes` is the command's
/// `--yes` flag. Without a terminal there is nobody to ask, so `--yes` is required.
pub fn confirm(prompt: &str, assume_yes: bool) -> bugsplat_core::Result<bool> {
    if assume_yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        return Err(Error::Usage(format!(
            "{prompt} Pass --yes to confirm when not running in a terminal."
        )));
    }
    eprint!("{prompt} [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

pub fn stdout_is_terminal() -> bool {
    std::io::stdout().is_terminal()
}

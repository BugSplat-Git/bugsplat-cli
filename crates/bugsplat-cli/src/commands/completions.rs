//! `completions`: print a shell completion script.

use clap::{Args, CommandFactory};
use clap_complete::Shell;

use crate::cli::Cli;

#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// Shell to generate the script for
    pub shell: Shell,
}

pub fn run(args: CompletionsArgs) -> anyhow::Result<()> {
    let mut command = Cli::command();
    clap_complete::generate(args.shell, &mut command, "bugsplat", &mut std::io::stdout());
    Ok(())
}

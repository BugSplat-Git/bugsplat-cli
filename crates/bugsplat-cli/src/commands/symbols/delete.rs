//! `symbols delete`. https://github.com/BugSplat-Git/bugsplat-cli/issues/7

use bugsplat_core::Error;
use clap::Args as ClapArgs;

use crate::context::Context;

const ISSUE: &str = "https://github.com/BugSplat-Git/bugsplat-cli/issues/7";

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Application name
    #[arg(short = 'a', long)]
    pub application: String,
    /// Application version whose symbols to delete
    #[arg(short = 'v', long)]
    pub version: String,
    /// Don't ask for confirmation
    #[arg(short, long)]
    pub yes: bool,
}

pub async fn run(_args: Args, _ctx: &Context) -> anyhow::Result<()> {
    Err(Error::NotImplemented {
        command: "symbols delete",
        issue: ISSUE,
    }
    .into())
}

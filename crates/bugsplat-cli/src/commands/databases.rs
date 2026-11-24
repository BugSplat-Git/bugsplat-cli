//! `databases`. https://github.com/BugSplat-Git/bugsplat-cli/issues/4

use bugsplat_core::Error;
use clap::{Args, Subcommand};

use crate::context::Context;

const ISSUE: &str = "https://github.com/BugSplat-Git/bugsplat-cli/issues/4";

#[derive(Debug, Subcommand)]
pub enum DatabasesCommand {
    /// List the databases you can open
    List(ListArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {}

pub async fn run(_command: DatabasesCommand, _ctx: &Context) -> anyhow::Result<()> {
    Err(Error::NotImplemented {
        command: "databases",
        issue: ISSUE,
    }
    .into())
}

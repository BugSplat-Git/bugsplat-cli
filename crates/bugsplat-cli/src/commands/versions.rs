//! `versions`. https://github.com/BugSplat-Git/bugsplat-cli/issues/4

use bugsplat_core::Error;
use clap::{Args, Subcommand};

use crate::context::Context;

const ISSUE: &str = "https://github.com/BugSplat-Git/bugsplat-cli/issues/4";

#[derive(Debug, Subcommand)]
pub enum VersionsCommand {
    /// List application versions
    List(ListArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {}

pub async fn run(_command: VersionsCommand, _ctx: &Context) -> anyhow::Result<()> {
    Err(Error::NotImplemented {
        command: "versions",
        issue: ISSUE,
    }
    .into())
}

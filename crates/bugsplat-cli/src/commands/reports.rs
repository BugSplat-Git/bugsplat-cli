//! `reports`. https://github.com/BugSplat-Git/bugsplat-cli/issues/4

use bugsplat_core::Error;
use clap::{Args, Subcommand};

use crate::context::Context;

const ISSUE: &str = "https://github.com/BugSplat-Git/bugsplat-cli/issues/4";

#[derive(Debug, Subcommand)]
pub enum ReportsCommand {
    /// List crash reports, newest first
    List(ListArgs),
    /// Show one crash report
    Get(GetArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {}

#[derive(Debug, Args)]
pub struct GetArgs {
    /// Crash id
    pub id: u64,
}

pub async fn run(_command: ReportsCommand, _ctx: &Context) -> anyhow::Result<()> {
    Err(Error::NotImplemented {
        command: "reports",
        issue: ISSUE,
    }
    .into())
}

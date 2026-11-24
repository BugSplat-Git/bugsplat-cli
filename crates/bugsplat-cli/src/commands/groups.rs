//! `groups`. https://github.com/BugSplat-Git/bugsplat-cli/issues/4

use bugsplat_core::Error;
use clap::{Args, Subcommand};

use crate::context::Context;

const ISSUE: &str = "https://github.com/BugSplat-Git/bugsplat-cli/issues/4";

#[derive(Debug, Subcommand)]
pub enum GroupsCommand {
    /// List crash groups
    List(ListArgs),
    /// Show one crash group
    Get(GetArgs),
    /// Mark a crash group open, fixed, regression or ignored
    SetStatus(SetStatusArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {}

#[derive(Debug, Args)]
pub struct GetArgs {
    /// Crash group id
    pub id: u64,
}

#[derive(Debug, Args)]
pub struct SetStatusArgs {
    /// Crash group id
    pub id: u64,
    /// open, fixed, regression or ignored
    pub status: String,
}

pub async fn run(_command: GroupsCommand, _ctx: &Context) -> anyhow::Result<()> {
    Err(Error::NotImplemented {
        command: "groups",
        issue: ISSUE,
    }
    .into())
}

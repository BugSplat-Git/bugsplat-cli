//! `defects`. https://github.com/BugSplat-Git/bugsplat-cli/issues/4

use bugsplat_core::Error;
use clap::{Args, Subcommand};

use crate::context::Context;

const ISSUE: &str = "https://github.com/BugSplat-Git/bugsplat-cli/issues/4";

#[derive(Debug, Subcommand)]
pub enum DefectsCommand {
    /// File a crash group as a new defect in the connected issue tracker
    Create(CreateArgs),
    /// Link a crash group to a defect that already exists
    Link(LinkArgs),
    /// Remove the link between a crash group and its defect
    Unlink(UnlinkArgs),
}

#[derive(Debug, Args)]
pub struct CreateArgs {
    /// Crash group id
    pub group_id: u64,
    /// Text to add to the defect's description
    #[arg(long)]
    pub notes: Option<String>,
    /// Don't ask for confirmation
    #[arg(short, long)]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct LinkArgs {
    /// Crash group id
    pub group_id: u64,
    /// The defect's id or key as your tracker shows it, for example PROJ-123
    pub defect_id: String,
    /// Text to add to the defect
    #[arg(long)]
    pub notes: Option<String>,
    /// Don't ask for confirmation
    #[arg(short, long)]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct UnlinkArgs {
    /// Crash group id
    pub group_id: u64,
}

pub async fn run(_command: DefectsCommand, _ctx: &Context) -> anyhow::Result<()> {
    Err(Error::NotImplemented {
        command: "defects",
        issue: ISSUE,
    }
    .into())
}

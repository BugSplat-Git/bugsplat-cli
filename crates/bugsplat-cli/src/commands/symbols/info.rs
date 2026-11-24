//! `symbols info`. https://github.com/BugSplat-Git/bugsplat-cli/issues/5

use bugsplat_core::Error;
use clap::Args as ClapArgs;

use crate::context::Context;

const ISSUE: &str = "https://github.com/BugSplat-Git/bugsplat-cli/issues/5";

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Files or bundles (.dSYM, .app, .xcarchive) to inspect
    #[arg(required = true, value_name = "FILE")]
    pub files: Vec<std::path::PathBuf>,
}

pub async fn run(_args: Args, _ctx: &Context) -> anyhow::Result<()> {
    Err(Error::NotImplemented {
        command: "symbols info",
        issue: ISSUE,
    }
    .into())
}

//! `symbols list`. https://github.com/BugSplat-Git/bugsplat-cli/issues/4

use bugsplat_core::Error;
use clap::Args as ClapArgs;

use crate::context::Context;

const ISSUE: &str = "https://github.com/BugSplat-Git/bugsplat-cli/issues/4";

#[derive(Debug, ClapArgs)]
pub struct Args {}

pub async fn run(_args: Args, _ctx: &Context) -> anyhow::Result<()> {
    Err(Error::NotImplemented {
        command: "symbols list",
        issue: ISSUE,
    }
    .into())
}

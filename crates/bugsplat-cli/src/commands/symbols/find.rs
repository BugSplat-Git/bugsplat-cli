//! `symbols find`. https://github.com/BugSplat-Git/bugsplat-cli/issues/6

use bugsplat_core::Error;
use clap::Args as ClapArgs;

use crate::context::Context;

const ISSUE: &str = "https://github.com/BugSplat-Git/bugsplat-cli/issues/6";

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Debug id to look for; repeat for several
    #[arg(long = "debug-id", value_name = "ID")]
    pub debug_ids: Vec<String>,
    /// File or module name to look for, with globs; repeat for several
    #[arg(long = "name", value_name = "NAME")]
    pub names: Vec<String>,
    /// Folder to search; repeat for several [default: .]
    #[arg(long = "path", value_name = "DIR")]
    pub paths: Vec<std::path::PathBuf>,
}

pub async fn run(_args: Args, _ctx: &Context) -> anyhow::Result<()> {
    Err(Error::NotImplemented {
        command: "symbols find",
        issue: ISSUE,
    }
    .into())
}

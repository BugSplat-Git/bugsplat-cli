//! `symbols upload`. https://github.com/BugSplat-Git/bugsplat-cli/issues/7

use bugsplat_core::Error;
use clap::Args as ClapArgs;

use crate::context::Context;

const ISSUE: &str = "https://github.com/BugSplat-Git/bugsplat-cli/issues/7";

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Application name, as the app reports it in crashes
    #[arg(short = 'a', long)]
    pub application: String,
    /// Application version, as the app reports it in crashes
    #[arg(short = 'v', long)]
    pub version: String,
    /// Base directory to search for symbol files
    #[arg(short = 'd', long, default_value = ".")]
    pub directory: std::path::PathBuf,
    /// Glob, relative to the directory, selecting the files to upload
    #[arg(short = 'f', long, default_value = "*.js.map")]
    pub files: String,
}

pub async fn run(_args: Args, _ctx: &Context) -> anyhow::Result<()> {
    Err(Error::NotImplemented {
        command: "symbols upload",
        issue: ISSUE,
    }
    .into())
}

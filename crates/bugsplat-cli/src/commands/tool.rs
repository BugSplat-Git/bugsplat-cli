//! `tool`. https://github.com/BugSplat-Git/bugsplat-cli/issues/4

use bugsplat_core::Error;
use clap::{Args, Subcommand};

use crate::context::Context;

const ISSUE: &str = "https://github.com/BugSplat-Git/bugsplat-cli/issues/4";

#[derive(Debug, Subcommand)]
pub enum ToolCommand {
    /// List the MCP server's tools
    List(ListArgs),
    /// Call a tool by name
    Call(CallArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {}

#[derive(Debug, Args)]
pub struct CallArgs {
    /// Tool name, for example list-groups
    pub name: String,
    /// The tool's arguments as a JSON object
    #[arg(long, value_name = "JSON")]
    pub args: Option<String>,
    /// Arguments as key=value pairs; a value that parses as JSON is sent as JSON
    #[arg(value_name = "KEY=VALUE")]
    pub pairs: Vec<String>,
}

pub async fn run(_command: ToolCommand, _ctx: &Context) -> anyhow::Result<()> {
    Err(Error::NotImplemented {
        command: "tool",
        issue: ISSUE,
    }
    .into())
}

//! `mcp`: run as a local MCP server. https://github.com/BugSplat-Git/bugsplat-cli/issues/9

use bugsplat_core::Error;
use clap::Args;

use crate::context::Context;

const ISSUE: &str = "https://github.com/BugSplat-Git/bugsplat-cli/issues/9";

#[derive(Debug, Args)]
pub struct McpArgs {}

pub async fn run(_args: McpArgs, _ctx: &Context) -> anyhow::Result<()> {
    Err(Error::NotImplemented {
        command: "mcp",
        issue: ISSUE,
    }
    .into())
}

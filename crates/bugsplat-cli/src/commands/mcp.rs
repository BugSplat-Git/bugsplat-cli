//! `mcp`: run as a local MCP server. https://github.com/BugSplat-Git/bugsplat-cli/issues/9

use std::io::IsTerminal;

use clap::Args;

use crate::context::Context;

#[derive(Debug, Args)]
#[command(after_help = "\
Started by an MCP client, not by hand: the client speaks the Model Context Protocol on this \
command's stdin and stdout. It serves the tools that need this computer's disk (symbols-info, \
symbols-find, symbols-upload); crash data comes from BugSplat's hosted MCP server, which the \
client connects to separately.

symbols-upload uses the session from `bugsplat login`, or a Client ID and Client Secret from \
the environment, and uploads to --database unless the tool call names another.

Client configuration:
  { \"mcpServers\": { \"bugsplat-cli\": { \"command\": \"bugsplat\", \"args\": [\"mcp\"] } } }")]
pub struct McpArgs {}

pub async fn run(_args: McpArgs, ctx: &Context) -> anyhow::Result<()> {
    if std::io::stdin().is_terminal() {
        eprintln!(
            "bugsplat mcp is waiting for an MCP client on stdin. Add it to your client's MCP \
             servers instead of running it here (see `bugsplat mcp --help`); Ctrl-C stops it."
        );
    }
    bugsplat_mcp_server::serve_stdio(ctx.config.clone()).await?;
    Ok(())
}

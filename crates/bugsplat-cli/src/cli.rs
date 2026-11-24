//! The command tree. Each command's arguments and `run` function live in its own file
//! under `commands/`; this file only wires them together.

use clap::{Args, Parser, Subcommand};

use crate::commands::{
    auth, completions, databases, defects, groups, mcp, reports, symbols, tool, versions,
};

#[derive(Debug, Parser)]
#[command(
    name = "bugsplat",
    version,
    about = "Read BugSplat crash data, find the symbol files a crash needs, and upload them."
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Args)]
pub struct GlobalArgs {
    /// BugSplat database. Defaults to your default database for commands that read crash data.
    #[arg(
        short = 'b',
        long,
        global = true,
        help_heading = "Global options",
        env = "BUGSPLAT_DATABASE"
    )]
    pub database: Option<String>,

    /// Base URL of the BugSplat API [default: https://api.bugsplat.com] [env: BUGSPLAT_API_URL, BUGSPLAT_HOST]
    #[arg(
        long,
        global = true,
        help_heading = "Global options",
        value_name = "URL"
    )]
    pub api_url: Option<String>,

    /// OAuth Client ID for the database, for CI [env: BUGSPLAT_CLIENT_ID, SYMBOL_UPLOAD_CLIENT_ID]
    #[arg(
        long,
        global = true,
        help_heading = "Global options",
        value_name = "ID"
    )]
    pub client_id: Option<String>,

    /// OAuth Client Secret for the database, for CI [env: BUGSPLAT_CLIENT_SECRET, SYMBOL_UPLOAD_CLIENT_SECRET]
    #[arg(
        long,
        global = true,
        help_heading = "Global options",
        value_name = "SECRET"
    )]
    pub client_secret: Option<String>,

    /// Print JSON instead of text
    #[arg(long, global = true, help_heading = "Global options")]
    pub json: bool,

    /// Print only results and errors
    #[arg(short, long, global = true, help_heading = "Global options")]
    pub quiet: bool,

    /// Print more detail about what the command is doing
    #[arg(long, global = true, help_heading = "Global options")]
    pub verbose: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Sign in to BugSplat in your browser
    Login(auth::LoginArgs),
    /// Sign out and delete the stored session
    Logout(auth::LogoutArgs),
    /// Show who is signed in
    Whoami(auth::WhoamiArgs),
    /// List the databases you can open
    #[command(subcommand)]
    Databases(databases::DatabasesCommand),
    /// List and read crash reports
    #[command(subcommand)]
    Reports(reports::ReportsCommand),
    /// List, read and update crash groups
    #[command(subcommand)]
    Groups(groups::GroupsCommand),
    /// List application versions
    #[command(subcommand)]
    Versions(versions::VersionsCommand),
    /// File crash groups as defects in your issue tracker
    #[command(subcommand)]
    Defects(defects::DefectsCommand),
    /// Inspect, find, upload and delete symbol files
    #[command(subcommand)]
    Symbols(symbols::SymbolsCommand),
    /// Call any tool of BugSplat's MCP server
    #[command(subcommand)]
    Tool(tool::ToolCommand),
    /// Print a shell completion script
    Completions(completions::CompletionsArgs),
    /// Run as a local MCP server over stdio
    Mcp(mcp::McpArgs),
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn command_tree_is_consistent() {
        Cli::command().debug_assert();
    }
}

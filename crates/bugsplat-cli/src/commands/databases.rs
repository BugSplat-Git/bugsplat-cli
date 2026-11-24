//! `databases`: `list-databases`. https://github.com/BugSplat-Git/bugsplat-cli/issues/4

use clap::{Args, Subcommand};

use super::tool::{self, ToolArguments};
use crate::context::Context;

#[derive(Debug, Subcommand)]
pub enum DatabasesCommand {
    /// List the databases you can open
    List(ListArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Text the database name contains (not case-sensitive)
    #[arg(long, value_name = "TEXT")]
    pub name: Option<String>,
    /// Text the company name contains (not case-sensitive)
    #[arg(long, value_name = "TEXT")]
    pub company: Option<String>,
}

pub async fn run(command: DatabasesCommand, ctx: &Context) -> anyhow::Result<()> {
    match command {
        // `list-databases` is the one tool without a `database` argument, so the global
        // --database is not sent.
        DatabasesCommand::List(args) => {
            let arguments = ToolArguments::default()
                .opt("name", args.name)
                .opt("company", args.company);
            tool::call(ctx, "list-databases", arguments).await
        }
    }
}

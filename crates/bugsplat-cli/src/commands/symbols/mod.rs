//! `symbols`: each subcommand lives in its own file.

pub mod delete;
pub mod find;
pub mod info;
pub mod list;
pub mod upload;

use clap::Subcommand;

use crate::context::Context;

#[derive(Debug, Subcommand)]
pub enum SymbolsCommand {
    /// List the symbol files uploaded to a database
    List(list::Args),
    /// Print the debug id, module name and type of local files
    Info(info::Args),
    /// Search folders for symbol files by debug id or name
    Find(find::Args),
    /// Upload symbol files for an application version
    Upload(upload::Args),
    /// Delete an application version's symbol files
    Delete(delete::Args),
}

pub async fn run(command: SymbolsCommand, ctx: &Context) -> anyhow::Result<()> {
    match command {
        SymbolsCommand::List(args) => list::run(args, ctx).await,
        SymbolsCommand::Info(args) => info::run(args, ctx).await,
        SymbolsCommand::Find(args) => find::run(args, ctx).await,
        SymbolsCommand::Upload(args) => upload::run(args, ctx).await,
        SymbolsCommand::Delete(args) => delete::run(args, ctx).await,
    }
}

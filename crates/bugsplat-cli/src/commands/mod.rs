pub mod auth;
pub mod completions;
pub mod databases;
pub mod defects;
pub mod groups;
pub mod mcp;
pub mod reports;
pub mod symbols;
pub mod tool;
pub mod versions;

use crate::cli::Command;
use crate::context::Context;

pub async fn run(command: Command, ctx: &Context) -> anyhow::Result<()> {
    match command {
        Command::Login(args) => auth::login(args, ctx).await,
        Command::Logout(args) => auth::logout(args, ctx).await,
        Command::Whoami(args) => auth::whoami(args, ctx).await,
        Command::Databases(command) => databases::run(command, ctx).await,
        Command::Reports(command) => reports::run(command, ctx).await,
        Command::Groups(command) => groups::run(command, ctx).await,
        Command::Versions(command) => versions::run(command, ctx).await,
        Command::Defects(command) => defects::run(command, ctx).await,
        Command::Symbols(command) => symbols::run(command, ctx).await,
        Command::Tool(command) => tool::run(command, ctx).await,
        Command::Completions(args) => completions::run(args),
        Command::Mcp(args) => mcp::run(args, ctx).await,
    }
}

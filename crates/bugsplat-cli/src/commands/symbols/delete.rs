//! `symbols delete`. https://github.com/BugSplat-Git/bugsplat-cli/issues/7

use bugsplat_upload::{Client, Target};
use clap::Args as ClapArgs;
use serde_json::json;

use crate::context::Context;
use crate::output::{confirm, print_json};

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Application name
    #[arg(short = 'a', long)]
    pub application: String,
    /// Application version whose symbols to delete
    #[arg(short = 'v', long)]
    pub version: String,
    /// Don't ask for confirmation
    #[arg(short, long)]
    pub yes: bool,
}

pub async fn run(args: Args, ctx: &Context) -> anyhow::Result<()> {
    let target = Target {
        database: ctx.config.database()?.to_string(),
        application: args.application,
        version: args.version,
    };
    // Fail before asking when there is nothing to authenticate with.
    bugsplat_auth::access_token(&ctx.config).await?;

    let prompt = format!(
        "Delete every symbol file for {} {} in database {}?",
        target.application, target.version, target.database
    );
    let deleted = confirm(&prompt, args.yes)?;
    if deleted {
        // Asked for again: the prompt can stay open longer than the first token lasts.
        let token = bugsplat_auth::access_token(&ctx.config).await?;
        Client::new(bugsplat_core::http_client()?, &ctx.config, token.secret)
            .delete_symbols(&target)
            .await?;
    }

    if ctx.config.is_json() {
        return print_json(&json!({
            "database": target.database,
            "application": target.application,
            "version": target.version,
            "deleted": deleted,
        }));
    }
    if deleted {
        println!(
            "Deleted the symbol files for {} {} in {}.",
            target.application, target.version, target.database
        );
    } else {
        eprintln!("Nothing was deleted.");
    }
    Ok(())
}

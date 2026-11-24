//! `defects`: `create-defect`, `add-defect-link` and `remove-defect-link`.
//! https://github.com/BugSplat-Git/bugsplat-cli/issues/4

use bugsplat_core::Error;
use clap::{Args, Subcommand};

use super::tool::{self, ToolArguments};
use crate::context::Context;
use crate::output::confirm;

#[derive(Debug, Subcommand)]
pub enum DefectsCommand {
    /// File a crash group as a new defect in the connected issue tracker
    Create(CreateArgs),
    /// Link a crash group to a defect that already exists
    Link(LinkArgs),
    /// Remove the link between a crash group and its defect
    Unlink(UnlinkArgs),
}

#[derive(Debug, Args)]
pub struct CreateArgs {
    /// Crash group id
    pub group_id: u64,
    /// Text to add to the defect's description
    #[arg(long)]
    pub notes: Option<String>,
    /// Don't ask for confirmation
    #[arg(short, long)]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct LinkArgs {
    /// Crash group id
    pub group_id: u64,
    /// The defect's id or key as your tracker shows it, for example PROJ-123
    pub defect_id: String,
    /// Text to add to the defect along with the group's details
    #[arg(long)]
    pub notes: Option<String>,
    /// Don't ask for confirmation
    #[arg(short, long)]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct UnlinkArgs {
    /// Crash group id
    pub group_id: u64,
}

pub async fn run(command: DefectsCommand, ctx: &Context) -> anyhow::Result<()> {
    match command {
        DefectsCommand::Create(args) => {
            let prompt = format!(
                "File crash group {} in {} as a new defect in the connected issue tracker? BugSplat can't delete the defect from the tracker afterwards.",
                args.group_id,
                database_label(ctx)
            );
            confirmed(&prompt, args.yes)?;
            let arguments = ToolArguments::for_database(ctx)
                .set("stackKeyId", args.group_id)
                .opt("notes", args.notes);
            tool::call(ctx, "create-defect", arguments).await
        }
        DefectsCommand::Link(args) => {
            let prompt = format!(
                "Link crash group {} in {} to defect {}? Most trackers also add the group's details and your notes to the defect.",
                args.group_id,
                database_label(ctx),
                args.defect_id
            );
            confirmed(&prompt, args.yes)?;
            let arguments = ToolArguments::for_database(ctx)
                .set("stackKeyId", args.group_id)
                .set("defectId", args.defect_id)
                .opt("notes", args.notes);
            tool::call(ctx, "add-defect-link", arguments).await
        }
        DefectsCommand::Unlink(args) => {
            let arguments = ToolArguments::for_database(ctx).set("stackKeyId", args.group_id);
            tool::call(ctx, "remove-defect-link", arguments).await
        }
    }
}

fn database_label(ctx: &Context) -> String {
    match &ctx.config.database {
        Some(database) => format!("database {database}"),
        None => "your default database".to_string(),
    }
}

fn confirmed(prompt: &str, assume_yes: bool) -> bugsplat_core::Result<()> {
    if confirm(prompt, assume_yes)? {
        Ok(())
    } else {
        Err(Error::Other("Cancelled. Nothing was changed.".to_string()))
    }
}

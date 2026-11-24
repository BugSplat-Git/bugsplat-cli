//! `versions`: `list-versions`. https://github.com/BugSplat-Git/bugsplat-cli/issues/4

use clap::{Args, Subcommand};

use super::tool::{self, FILTER_HELP, ToolArguments};
use crate::context::Context;

#[derive(Debug, Subcommand)]
pub enum VersionsCommand {
    /// List application versions, most recently reported first
    #[command(after_help = FILTER_HELP)]
    List(ListArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Application name (pattern filter)
    #[arg(long, value_name = "FILTER")]
    pub application: Option<String>,
    /// Application version (pattern filter)
    #[arg(long, value_name = "FILTER")]
    pub version: Option<String>,
    /// Include retired versions, whose new crashes are rejected
    #[arg(long)]
    pub include_retired: bool,
    /// Only versions whose last report is after this date, for example 2026-11-01
    #[arg(long, value_name = "DATE")]
    pub start_date: Option<String>,
    /// Only versions whose first report is before this date
    #[arg(long, value_name = "DATE")]
    pub end_date: Option<String>,
    /// Versions per page, 1 to 100; the server's default is 25
    #[arg(long, value_name = "N")]
    pub page_size: Option<u32>,
    /// Page to return, starting at 1
    #[arg(long, value_name = "N")]
    pub page: Option<u32>,
    /// lastReport (the default), firstReport, crashes, reportsPerDay, symbolSize or symbolsUpdated
    #[arg(long, value_name = "FIELD")]
    pub sort_by: Option<String>,
    /// Reverse the direction --sort-by implies
    #[arg(long, value_name = "ORDER", value_parser = ["asc", "desc"])]
    pub sort_order: Option<String>,
}

pub async fn run(command: VersionsCommand, ctx: &Context) -> anyhow::Result<()> {
    match command {
        VersionsCommand::List(args) => {
            let arguments = ToolArguments::for_database(ctx)
                .text_filter("application", args.application.as_ref())?
                .text_filter("version", args.version.as_ref())?
                .flag("includeRetired", args.include_retired)
                .opt("startDate", args.start_date)
                .opt("endDate", args.end_date)
                .opt("pageSize", args.page_size)
                .opt("page", args.page)
                .opt("sortBy", args.sort_by)
                .opt("sortOrder", args.sort_order);
            tool::call(ctx, "list-versions", arguments).await
        }
    }
}

//! `reports`: `list-reports` and `get-report`.
//! https://github.com/BugSplat-Git/bugsplat-cli/issues/4

use clap::{Args, Subcommand};

use super::tool::{self, FILTER_HELP, ToolArguments};
use crate::context::Context;

#[derive(Debug, Subcommand)]
pub enum ReportsCommand {
    /// List crash reports, newest first
    #[command(after_help = FILTER_HELP)]
    List(Box<ListArgs>),
    /// Show one crash report in full
    Get(GetArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Application name (pattern filter)
    #[arg(long, value_name = "FILTER")]
    pub application: Option<String>,
    /// Application version (pattern filter)
    #[arg(long, value_name = "FILTER")]
    pub version: Option<String>,
    /// Stack key of the crash group (pattern filter)
    #[arg(long, value_name = "FILTER")]
    pub stack_group: Option<String>,
    /// Crash group id, or -1 for reports that are not grouped yet (integer filter)
    #[arg(long, value_name = "FILTER", allow_negative_numbers = true)]
    pub stack_key_id: Option<String>,
    /// User value reported with the crash (pattern filter)
    #[arg(long, value_name = "FILTER")]
    pub user: Option<String>,
    /// Email value reported with the crash (pattern filter)
    #[arg(long, value_name = "FILTER")]
    pub email: Option<String>,
    /// Text in the user's description of the crash (contains filter)
    #[arg(long, value_name = "FILTER")]
    pub description: Option<String>,
    /// Exception code, for example c0000005 (exact filter)
    #[arg(long, value_name = "FILTER")]
    pub exception_code: Option<String>,
    /// Text in the exception message (contains filter)
    #[arg(long, value_name = "FILTER")]
    pub exception_message: Option<String>,
    /// Custom attribute to match (pattern filter), for example GPU='GeForce*'; repeat for more
    #[arg(long = "attribute", value_name = "NAME=FILTER")]
    pub attributes: Vec<String>,
    /// Only reports after this date, for example 2026-11-01 or 2026-11-01T12:00:00Z
    #[arg(long, value_name = "DATE")]
    pub start_date: Option<String>,
    /// Only reports before this date
    #[arg(long, value_name = "DATE")]
    pub end_date: Option<String>,
    /// Reports per page, 1 to 100; the server's default is 10
    #[arg(long, value_name = "N")]
    pub page_size: Option<u32>,
    /// Page to return, starting at 1
    #[arg(long, value_name = "N")]
    pub page: Option<u32>,
    /// date: newest crash time first (the default); id: highest crash id first
    #[arg(long, value_name = "FIELD")]
    pub sort_by: Option<String>,
    /// Reverse the direction --sort-by implies
    #[arg(long, value_name = "ORDER", value_parser = ["asc", "desc"])]
    pub sort_order: Option<String>,
}

#[derive(Debug, Args)]
pub struct GetArgs {
    /// Crash id, from `reports list` or `groups get`
    pub id: u64,
}

pub async fn run(command: ReportsCommand, ctx: &Context) -> anyhow::Result<()> {
    match command {
        ReportsCommand::List(args) => {
            let args = *args;
            let arguments = ToolArguments::for_database(ctx)
                .text_filter("application", args.application.as_ref())?
                .text_filter("version", args.version.as_ref())?
                .text_filter("stackGroup", args.stack_group.as_ref())?
                .integer_filter("stackKeyId", args.stack_key_id.as_ref())?
                .text_filter("user", args.user.as_ref())?
                .text_filter("email", args.email.as_ref())?
                .text_filter("description", args.description.as_ref())?
                .text_filter("exceptionCode", args.exception_code.as_ref())?
                .text_filter("exceptionMessage", args.exception_message.as_ref())?
                .attributes(&args.attributes)?
                .opt("startDate", args.start_date)
                .opt("endDate", args.end_date)
                .opt("pageSize", args.page_size)
                .opt("page", args.page)
                .opt("sortBy", args.sort_by)
                .opt("sortOrder", args.sort_order);
            tool::call(ctx, "list-reports", arguments).await
        }
        ReportsCommand::Get(args) => {
            let arguments = ToolArguments::for_database(ctx).set("id", args.id);
            tool::call(ctx, "get-report", arguments).await
        }
    }
}

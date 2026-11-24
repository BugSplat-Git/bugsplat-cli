//! `groups`: `list-groups`, `get-group` and `set-group-status`.
//! https://github.com/BugSplat-Git/bugsplat-cli/issues/4

use clap::{Args, Subcommand};

use super::tool::{self, FILTER_HELP, ToolArguments};
use crate::context::Context;

#[derive(Debug, Subcommand)]
pub enum GroupsCommand {
    /// List crash groups, the ones with the most crashes first
    #[command(after_help = FILTER_HELP)]
    List(Box<ListArgs>),
    /// Show one crash group
    Get(GetArgs),
    /// Mark a crash group open, fixed, regression or ignored
    SetStatus(SetStatusArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Application name (pattern filter)
    #[arg(long, value_name = "FILTER")]
    pub application: Option<String>,
    /// Application version (pattern filter)
    #[arg(long, value_name = "FILTER")]
    pub version: Option<String>,
    /// open, fixed, ignored or regression (status filter)
    #[arg(long, value_name = "FILTER")]
    pub status: Option<String>,
    /// Text the stack key contains, such as a function or module name (contains filter)
    #[arg(long, value_name = "FILTER")]
    pub stack_key: Option<String>,
    /// Number of crashes in the group, for example '{"gte":10}' (integer filter)
    #[arg(long, value_name = "FILTER")]
    pub crashes: Option<String>,
    /// Number of affected users, for example '{"gte":5}' (integer filter)
    #[arg(long, value_name = "FILTER")]
    pub users: Option<String>,
    /// Count only reports after this date, for example 2026-11-01 or 2026-11-01T12:00:00Z
    #[arg(long, value_name = "DATE")]
    pub start_date: Option<String>,
    /// Count only reports before this date
    #[arg(long, value_name = "DATE")]
    pub end_date: Option<String>,
    /// true: only groups linked to a defect; false: only groups without one
    #[arg(long, value_name = "BOOL")]
    pub linked_defect: Option<bool>,
    /// Groups per page, 1 to 100; the server's default is 10
    #[arg(long, value_name = "N")]
    pub page_size: Option<u32>,
    /// Page to return, starting at 1
    #[arg(long, value_name = "N")]
    pub page: Option<u32>,
    /// crashes: most crashes first (the default); users: most affected users first;
    /// newest: most recent last report first; oldest: earliest first report first
    #[arg(long, value_name = "FIELD")]
    pub sort_by: Option<String>,
    /// Reverse the direction --sort-by implies
    #[arg(long, value_name = "ORDER", value_parser = ["asc", "desc"])]
    pub sort_order: Option<String>,
}

#[derive(Debug, Args)]
pub struct GetArgs {
    /// Crash group id, from `groups list`
    pub id: u64,
    /// Number of recent reports to include, 1 to 100; the server's default is 5
    #[arg(long, value_name = "N")]
    pub page_size: Option<u32>,
}

#[derive(Debug, Args)]
pub struct SetStatusArgs {
    /// Crash group id
    pub id: u64,
    /// The group's new status; the web app shows fixed as Closed
    #[arg(value_parser = ["open", "fixed", "regression", "ignored"])]
    pub status: String,
}

pub async fn run(command: GroupsCommand, ctx: &Context) -> anyhow::Result<()> {
    match command {
        GroupsCommand::List(args) => {
            let args = *args;
            let arguments = ToolArguments::for_database(ctx)
                .text_filter("application", args.application.as_ref())?
                .text_filter("version", args.version.as_ref())?
                .text_filter("status", args.status.as_ref())?
                .text_filter("stackKey", args.stack_key.as_ref())?
                .integer_filter("crashes", args.crashes.as_ref())?
                .integer_filter("users", args.users.as_ref())?
                .opt("startDate", args.start_date)
                .opt("endDate", args.end_date)
                .opt("linkedDefect", args.linked_defect)
                .opt("pageSize", args.page_size)
                .opt("page", args.page)
                .opt("sortBy", args.sort_by)
                .opt("sortOrder", args.sort_order);
            tool::call(ctx, "list-groups", arguments).await
        }
        GroupsCommand::Get(args) => {
            let arguments = ToolArguments::for_database(ctx)
                .set("stackKeyId", args.id)
                .opt("pageSize", args.page_size);
            tool::call(ctx, "get-group", arguments).await
        }
        GroupsCommand::SetStatus(args) => {
            let arguments = ToolArguments::for_database(ctx)
                .set("stackKeyId", args.id)
                .set("status", args.status);
            tool::call(ctx, "set-group-status", arguments).await
        }
    }
}

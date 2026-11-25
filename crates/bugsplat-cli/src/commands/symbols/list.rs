//! `symbols list`: `list-symbols`. https://github.com/BugSplat-Git/bugsplat-cli/issues/4

use clap::Args as ClapArgs;

use crate::commands::tool::{self, FILTER_HELP, ToolArguments};
use crate::context::Context;

#[derive(Debug, ClapArgs)]
#[command(after_help = FILTER_HELP)]
pub struct Args {
    /// Application name (pattern filter)
    #[arg(long, value_name = "FILTER")]
    pub application: Option<String>,
    /// Application version (pattern filter)
    #[arg(long, value_name = "FILTER")]
    pub version: Option<String>,
    /// File name, for example myapp.pdb (contains filter)
    #[arg(long, value_name = "FILTER")]
    pub name: Option<String>,
    /// Debug id (GUID and age) of the file (exact filter)
    #[arg(long, value_name = "FILTER")]
    pub debug_id: Option<String>,
    /// Files per page, 1 to 100; the server's default is 25
    #[arg(long, value_name = "N")]
    pub page_size: Option<u32>,
    /// Page to return, starting at 1
    #[arg(long, value_name = "N")]
    pub page: Option<u32>,
    /// modified: most recently modified first (the default); uploaded; lastUsed;
    /// size: largest first; name: file name A to Z
    #[arg(long, value_name = "FIELD")]
    pub sort_by: Option<String>,
    /// Reverse the direction --sort-by implies
    #[arg(long, value_name = "ORDER", value_parser = ["asc", "desc"])]
    pub sort_order: Option<String>,
}

pub async fn run(args: Args, ctx: &Context) -> anyhow::Result<()> {
    let arguments = ToolArguments::for_database(ctx)
        .text_filter("application", args.application.as_ref())?
        .text_filter("version", args.version.as_ref())?
        .text_filter("name", args.name.as_ref())?
        .text_filter("debugId", args.debug_id.as_ref())?
        .opt("pageSize", args.page_size)
        .opt("page", args.page)
        .opt("sortBy", args.sort_by)
        .opt("sortOrder", args.sort_order);
    tool::call(ctx, "list-symbols", arguments).await
}

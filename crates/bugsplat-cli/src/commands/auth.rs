//! `login`, `logout`, `whoami`. https://github.com/BugSplat-Git/bugsplat-cli/issues/3

use bugsplat_auth::LoginOptions;
use clap::Args;

use crate::context::Context;

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Print the sign-in URL instead of opening a browser
    #[arg(long)]
    pub no_browser: bool,
}

#[derive(Debug, Args)]
pub struct LogoutArgs {}

#[derive(Debug, Args)]
pub struct WhoamiArgs {}

pub async fn login(args: LoginArgs, ctx: &Context) -> anyhow::Result<()> {
    let options = LoginOptions {
        no_browser: args.no_browser,
    };
    bugsplat_auth::login(&ctx.config, &options).await?;
    Ok(())
}

pub async fn logout(_args: LogoutArgs, ctx: &Context) -> anyhow::Result<()> {
    bugsplat_auth::logout(&ctx.config).await?;
    Ok(())
}

pub async fn whoami(_args: WhoamiArgs, ctx: &Context) -> anyhow::Result<()> {
    bugsplat_auth::whoami(&ctx.config).await?;
    Ok(())
}

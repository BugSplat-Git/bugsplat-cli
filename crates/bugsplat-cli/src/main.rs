mod cli;
mod commands;
mod context;
mod output;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::Cli;
use crate::context::Context;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            let code = err
                .downcast_ref::<bugsplat_core::Error>()
                .map_or(1, bugsplat_core::Error::exit_code);
            ExitCode::from(code)
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let ctx = Context::from_global(&cli.global)?;
    commands::run(cli.command, &ctx).await
}

//! `symbols upload`. https://github.com/BugSplat-Git/bugsplat-cli/issues/7

use std::collections::HashMap;
use std::io::IsTerminal;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bugsplat_core::Error;
use bugsplat_upload::{
    Client, Event, LocalCopy, Options, Route, Summary, SymbolFile, Target, TokenProvider,
};
use clap::Args as ClapArgs;
use indicatif::{DecimalBytes, MultiProgress, ProgressBar, ProgressStyle};
use serde::Serialize;

use super::dump::{self, Dumped};
use crate::context::Context;
use crate::output::print_json;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Application name, as the app reports it in crashes [default: name in ./package.json]
    #[arg(short = 'a', long)]
    pub application: Option<String>,
    /// Application version, as the app reports it in crashes [default: version in ./package.json]
    #[arg(short = 'v', long)]
    pub version: Option<String>,
    /// Base directory to search for symbol files
    #[arg(short = 'd', long, default_value = ".")]
    pub directory: PathBuf,
    /// Glob, relative to the directory, selecting the files to upload
    #[arg(short = 'f', long, default_value = "*.js.map")]
    pub files: String,
    /// Files to upload at once [default: the number of CPU cores or 8, whichever is larger]
    #[arg(short = 'w', long, value_name = "COUNT")]
    pub workers: Option<NonZeroUsize>,
    /// Copy the files into a symbol server folder layout here instead of uploading them
    #[arg(short = 'l', long, value_name = "DIR")]
    pub local_path: Option<PathBuf>,
    /// List what would be uploaded, with each file's debug id, and upload nothing
    #[arg(long)]
    pub dry_run: bool,
    /// Generate a Breakpad .sym file from each selected binary with dump_syms, and upload
    /// that instead of the binary
    #[arg(short = 'm', long)]
    pub dump_syms: bool,
}

pub async fn run(args: Args, ctx: &Context) -> anyhow::Result<()> {
    let uploading = !args.dry_run && args.local_path.is_none();
    let names = Names::resolve(&args, ctx);

    // Checked before the search so a missing flag or session fails at once, not after
    // reading every file's debug id.
    let destination = if uploading {
        let target = names.target(ctx)?;
        // Fail before reading any file when there is nothing to authenticate with.
        bugsplat_auth::access_token(&ctx.config).await?;
        let config = ctx.config.clone();
        let provider: TokenProvider = Arc::new(move || {
            let config = config.clone();
            Box::pin(async move { Ok(bugsplat_auth::access_token(&config).await?.secret) })
        });
        let client =
            Client::with_token_provider(bugsplat_core::http_client()?, &ctx.config, provider);
        Some((client, target))
    } else {
        None
    };

    let paths = bugsplat_upload::find_files(&args.directory, &args.files)?;
    if paths.is_empty() {
        return Err(Error::Other(format!(
            "Could not find any files to upload using glob {}",
            args.directory.join(&args.files).display()
        ))
        .into());
    }
    // Holds the generated .sym files until they are uploaded or copied.
    let scratch;
    let (paths, generated) = if args.dump_syms {
        scratch = tempfile::Builder::new()
            .prefix("bugsplat-dump-syms-")
            .tempdir()?;
        let generated = dump::dump_for_upload(&paths, scratch.path(), ctx.config.quiet)?;
        let paths = generated.iter().map(|sym| sym.path.clone()).collect();
        (paths, Some(generated))
    } else {
        (paths, None)
    };
    let generated = generated.as_deref();
    let files = bugsplat_upload::plan(&paths, bugsplat_debugid::read);

    if args.dry_run {
        let summary = Summary::dry_run(&files);
        if ctx.config.is_json() {
            return print_json(&Report::new(&names, true, &summary, generated));
        }
        print_plan(&files, generated.unwrap_or_default());
        return Ok(());
    }

    if let Some(local_path) = &args.local_path {
        let copy = bugsplat_upload::copy_to_local(&files, local_path)?;
        if !ctx.config.quiet {
            for file in copy.files.iter().filter(|file| file.destination.is_none()) {
                eprintln!("{}", skip_warning(&file.file.name, &file.file.skip_reason));
            }
        }
        if ctx.config.is_json() {
            return print_json(&copy);
        }
        print_local_copy(&copy);
        return Ok(());
    }

    let (client, target) = destination.expect("a destination when uploading");
    let options = Options {
        workers: args
            .workers
            .map_or_else(bugsplat_upload::default_workers, NonZeroUsize::get),
        ..Options::default()
    };
    let progress = Arc::new(Progress::new(ctx.config.quiet));
    let sink = Arc::clone(&progress);
    let result = bugsplat_upload::upload(
        &client,
        &target,
        &files,
        &options,
        Arc::new(move |event| sink.handle(event)),
    )
    .await;
    progress.finish();
    let summary = result?;

    if ctx.config.is_json() {
        return print_json(&Report::new(&names, false, &summary, generated));
    }
    println!(
        "Uploaded {} ({}) to {} for {} {} in {:.1}s ({}/s).{}",
        count(summary.uploaded, "file"),
        DecimalBytes(summary.bytes),
        target.database,
        target.application,
        target.version,
        summary.seconds,
        DecimalBytes(summary.bytes_per_second as u64),
        skipped_note(summary.skipped),
    );
    Ok(())
}

/// The database, application and version, from flags, the environment, or `./package.json`
/// (its `database`, `name` and `version`), which is where `symbol-upload` looks too.
struct Names {
    database: Option<String>,
    application: Option<String>,
    version: Option<String>,
}

impl Names {
    fn resolve(args: &Args, ctx: &Context) -> Self {
        let mut names = Self {
            database: ctx.config.database.clone(),
            application: args.application.clone(),
            version: args.version.clone(),
        };
        if names.database.is_some() && names.application.is_some() && names.version.is_some() {
            return names;
        }
        let Some(package) = std::fs::read("package.json")
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        else {
            return names;
        };
        let field = |key: &str| {
            package
                .get(key)
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        names.database = names.database.or_else(|| field("database"));
        names.application = names.application.or_else(|| field("name"));
        names.version = names.version.or_else(|| field("version"));
        names
    }

    fn target(&self, ctx: &Context) -> bugsplat_core::Result<Target> {
        let database = match &self.database {
            Some(database) => database.clone(),
            None => ctx.config.database()?.to_string(),
        };
        let missing = |flag: &str, field: &str| {
            Error::Usage(format!(
                "No {flag}. Pass --{flag}, or run in a folder whose package.json has a {field}."
            ))
        };
        Ok(Target {
            database,
            application: self
                .application
                .clone()
                .ok_or_else(|| missing("application", "name"))?,
            version: self
                .version
                .clone()
                .ok_or_else(|| missing("version", "version"))?,
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report<'a> {
    database: Option<&'a str>,
    application: Option<&'a str>,
    version: Option<&'a str>,
    dry_run: bool,
    #[serde(flatten)]
    summary: &'a Summary,
    /// With `--dump-syms`: the generated files and what each was generated from. They are
    /// temporary, so their paths are gone once the command ends.
    #[serde(skip_serializing_if = "Option::is_none")]
    generated: Option<&'a [Dumped]>,
}

impl<'a> Report<'a> {
    fn new(
        names: &'a Names,
        dry_run: bool,
        summary: &'a Summary,
        generated: Option<&'a [Dumped]>,
    ) -> Self {
        Self {
            database: names.database.as_deref(),
            application: names.application.as_deref(),
            version: names.version.as_deref(),
            dry_run,
            summary,
            generated,
        }
    }
}

fn print_plan(files: &[SymbolFile], generated: &[Dumped]) {
    let id_width = files
        .iter()
        .map(|file| file.debug_id.len())
        .max()
        .unwrap_or(0)
        .max("DEBUG ID".len());
    println!("{:<13}  {:<id_width$}  FILE", "ROUTE", "DEBUG ID");
    for file in files {
        let route = match file.route {
            Route::SymbolServer => "symbol-server",
            Route::Versions => "versions",
            Route::Skip => "skip",
        };
        let debug_id = if file.debug_id.is_empty() {
            "-"
        } else {
            &file.debug_id
        };
        // A generated file is temporary; what it was generated from is the path that lasts.
        let shown = match generated.iter().find(|sym| sym.path == file.path) {
            Some(sym) => format!("{} from {}", file.name, sym.source.display()),
            None => file.path.display().to_string(),
        };
        let mut line = format!("{route:<13}  {debug_id:<id_width$}  {shown}");
        if let Some(arch) = &file.arch {
            line.push_str(&format!(" ({arch})"));
        }
        if let Some(reason) = &file.skip_reason {
            line.push_str(&format!(" ({reason})"));
        }
        println!("{line}");
    }
    let skipped = files
        .iter()
        .filter(|file| file.route == Route::Skip)
        .count();
    println!(
        "{} would be uploaded.{}",
        count(files.len() - skipped, "file"),
        skipped_note(skipped)
    );
}

fn print_local_copy(copy: &LocalCopy) {
    println!(
        "Copied {} ({}) to {}.{}",
        count(copy.copied, "file"),
        DecimalBytes(copy.bytes),
        copy.local_path.display(),
        skipped_note(copy.skipped),
    );
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

fn skipped_note(skipped: usize) -> String {
    if skipped == 0 {
        String::new()
    } else {
        format!(" Skipped {}.", count(skipped, "file"))
    }
}

fn skip_warning(name: &str, reason: &Option<String>) -> String {
    format!(
        "warning: skipping {name}: {}",
        reason.as_deref().unwrap_or("it has no debug id")
    )
}

/// Shows uploads on stderr: a live line per file on a terminal, a plain line per finished
/// file otherwise.
struct Progress {
    quiet: bool,
    bars: Option<MultiProgress>,
    active: Mutex<HashMap<usize, ProgressBar>>,
}

impl Progress {
    fn new(quiet: bool) -> Self {
        Self {
            quiet,
            bars: (!quiet && std::io::stderr().is_terminal()).then(MultiProgress::new),
            active: Mutex::new(HashMap::new()),
        }
    }

    fn handle(&self, event: Event) {
        if self.quiet {
            return;
        }
        match event {
            Event::Skipped { name, reason, .. } => {
                self.line(skip_warning(&name, &Some(reason)));
            }
            Event::Started { index, name, bytes } => {
                if let Some(bars) = &self.bars {
                    let bar = bars.add(ProgressBar::new(bytes));
                    bar.set_style(
                        ProgressStyle::with_template(
                            "{spinner} {prefix} {decimal_bytes}/{decimal_total_bytes} ({decimal_bytes_per_sec})",
                        )
                        .expect("a valid progress template"),
                    );
                    bar.set_prefix(name);
                    bar.enable_steady_tick(Duration::from_millis(120));
                    self.active().insert(index, bar);
                }
            }
            Event::Progress { index, sent } => {
                if let Some(bar) = self.active().get(&index) {
                    bar.set_position(sent);
                }
            }
            Event::Uploaded {
                index,
                name,
                bytes,
                elapsed,
            } => {
                if let Some(bar) = self.active().remove(&index) {
                    bar.finish_and_clear();
                }
                let rate = bytes as f64 / elapsed.as_secs_f64().max(0.001);
                self.line(format!(
                    "uploaded {name} ({} at {}/s)",
                    DecimalBytes(bytes),
                    DecimalBytes(rate as u64)
                ));
            }
            Event::Retrying {
                name, error, delay, ..
            } => {
                self.line(format!(
                    "{name}: {error}; trying again in {:.1}s",
                    delay.as_secs_f64()
                ));
            }
            Event::RateLimited { pause } => {
                self.line(format!(
                    "BugSplat is rate limiting uploads; pausing every upload for {:.0}s",
                    pause.as_secs_f64()
                ));
            }
        }
    }

    fn finish(&self) {
        for (_, bar) in self.active().drain() {
            bar.finish_and_clear();
        }
    }

    fn active(&self) -> std::sync::MutexGuard<'_, HashMap<usize, ProgressBar>> {
        self.active.lock().expect("progress lock")
    }

    fn line(&self, line: String) {
        match &self.bars {
            // Printed through the bars so a line doesn't land in the middle of one.
            Some(bars) => {
                let _ = bars.println(line);
            }
            None => eprintln!("{line}"),
        }
    }
}

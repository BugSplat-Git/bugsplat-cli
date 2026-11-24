use bugsplat_core::{Config, OutputFormat};

use crate::cli::GlobalArgs;

/// What a command's `run` function receives besides its own arguments.
pub struct Context {
    pub config: Config,
}

impl Context {
    pub fn from_global(global: &GlobalArgs) -> bugsplat_core::Result<Self> {
        let api_url = first_set(&global.api_url, &["BUGSPLAT_API_URL", "BUGSPLAT_HOST"]);
        let mut config = Config::new(api_url.as_deref())?;
        config.database = global.database.clone().filter(|d| !d.is_empty());
        config.client_id = first_set(
            &global.client_id,
            &["BUGSPLAT_CLIENT_ID", "SYMBOL_UPLOAD_CLIENT_ID"],
        );
        config.client_secret = first_set(
            &global.client_secret,
            &["BUGSPLAT_CLIENT_SECRET", "SYMBOL_UPLOAD_CLIENT_SECRET"],
        );
        config.output = if global.json {
            OutputFormat::Json
        } else {
            OutputFormat::Text
        };
        config.quiet = global.quiet;
        config.verbose = global.verbose;
        Ok(Self { config })
    }
}

/// The flag if given, otherwise the first of `env_names` that is set and not empty.
fn first_set(flag: &Option<String>, env_names: &[&str]) -> Option<String> {
    flag.clone().filter(|v| !v.is_empty()).or_else(|| {
        env_names
            .iter()
            .find_map(|name| std::env::var(name).ok().filter(|v| !v.is_empty()))
    })
}

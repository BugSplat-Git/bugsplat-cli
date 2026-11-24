use std::time::Duration;

use crate::Result;

/// One client for the whole run: shared connection pool, our user agent, sane timeouts.
/// No overall request timeout, because symbol uploads can be large.
pub fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("bugsplat-cli/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        .build()?)
}

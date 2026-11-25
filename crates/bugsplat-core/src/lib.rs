//! Configuration, errors and the HTTP client shared by the BugSplat CLI crates.

mod config;
mod error;
mod http;

pub use config::{Config, DEFAULT_API_URL, OutputFormat};
pub use error::{Error, Result};
pub use http::{REQUEST_TIMEOUT, http_client, http_client_with_timeout};

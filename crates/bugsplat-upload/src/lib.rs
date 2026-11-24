//! Upload symbol files to BugSplat, and delete a version's symbols.
//! See https://github.com/BugSplat-Git/bugsplat-cli/issues/7
//!
//! [`find_files`] turns a directory and a glob into paths, [`plan`] decides how each file is
//! uploaded (it takes the debug id reader as a parameter), and [`upload`] or [`copy_to_local`]
//! carries the plan out.

mod client;
mod compress;
mod find;
mod local;
mod plan;
mod retry;
mod time;
mod upload;

pub use bugsplat_debugid::{DebugInfo, Slice};

pub use client::{Client, Target};
pub use find::find_files;
pub use local::{LocalCopy, LocalFile, copy_to_local};
pub use plan::{Route, SymbolFile, plan};
pub use retry::RetryPolicy;
pub use upload::{
    Event, EventSink, FileResult, FileStatus, Options, Summary, default_workers, upload,
};

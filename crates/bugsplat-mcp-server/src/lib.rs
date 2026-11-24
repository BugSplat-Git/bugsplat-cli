//! `bugsplat mcp`: a local MCP server over stdio.
//! See https://github.com/BugSplat-Git/bugsplat-cli/issues/9
//!
//! It has the tools BugSplat's hosted MCP server can't have, because they read the user's
//! disk: `symbols-info`, `symbols-find` and `symbols-upload`. Crash data stays with the hosted
//! server, and a client connects to both.
//!
//! stdout is the protocol channel, so nothing in this crate prints to it.

mod server;
mod tools;

pub use server::serve_stdio;

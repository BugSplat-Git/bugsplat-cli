//! A minimal Streamable HTTP client for BugSplat's hosted MCP server.
//! See https://github.com/BugSplat-Git/bugsplat-cli/issues/4

use bugsplat_core::{Error, Result};
use serde_json::Value;
use url::Url;

const ISSUE: &str = "https://github.com/BugSplat-Git/bugsplat-cli/issues/4";

#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    /// The result's text content blocks, joined with newlines.
    pub text: String,
    /// The result's `structuredContent`, when the server sends it.
    pub structured: Option<Value>,
    pub is_error: bool,
    /// The whole `result` object as the server sent it.
    pub raw: Value,
}

pub struct McpClient {
    _endpoint: Url,
}

impl McpClient {
    /// Runs the `initialize` handshake against `endpoint` (see `Config::mcp_url`).
    /// A 401 is `Error::NotSignedIn`.
    pub async fn connect(endpoint: Url, _bearer_token: &str) -> Result<Self> {
        let _ = McpClient {
            _endpoint: endpoint,
        };
        Err(Error::NotImplemented {
            command: "tool",
            issue: ISSUE,
        })
    }

    pub async fn list_tools(&mut self) -> Result<Vec<Tool>> {
        Err(Error::NotImplemented {
            command: "tool list",
            issue: ISSUE,
        })
    }

    /// `arguments` is the tool's argument object. A result with `isError` is returned,
    /// not turned into an `Err`; the caller decides how to report it.
    pub async fn call_tool(&mut self, _name: &str, _arguments: Value) -> Result<ToolResult> {
        Err(Error::NotImplemented {
            command: "tool call",
            issue: ISSUE,
        })
    }
}

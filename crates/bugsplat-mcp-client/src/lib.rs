//! A minimal Streamable HTTP client for BugSplat's hosted MCP server.
//! See https://github.com/BugSplat-Git/bugsplat-cli/issues/4

mod sse;

use std::time::Duration;

use bugsplat_core::{Error, Result};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::{Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

use crate::sse::SseParser;

/// The protocol version this client asks for; the server may answer with an older one.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

const SESSION_HEADER: &str = "Mcp-Session-Id";
const PROTOCOL_HEADER: &str = "Mcp-Protocol-Version";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
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

impl ToolResult {
    fn from_raw(raw: Value) -> Self {
        let text = raw
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        let structured = raw
            .get("structuredContent")
            .filter(|content| !content.is_null())
            .cloned();
        let is_error = raw.get("isError").and_then(Value::as_bool).unwrap_or(false);
        Self {
            text,
            structured,
            is_error,
            raw,
        }
    }
}

pub struct McpClient {
    http: reqwest::Client,
    endpoint: Url,
    bearer_token: String,
    session_id: Option<String>,
    protocol_version: Option<String>,
    next_id: u64,
}

// Not derived: the token must never be printed.
impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("endpoint", &self.endpoint.as_str())
            .field("bearer_token", &"***")
            .field("session_id", &self.session_id)
            .field("protocol_version", &self.protocol_version)
            .finish()
    }
}

impl McpClient {
    /// Runs the `initialize` handshake against `endpoint` (see `Config::mcp_url`).
    /// A 401 is `Error::NotSignedIn`.
    pub async fn connect(endpoint: Url, bearer_token: &str) -> Result<Self> {
        let mut client = McpClient {
            http: bugsplat_core::http_client()?,
            endpoint,
            bearer_token: bearer_token.to_string(),
            session_id: None,
            protocol_version: None,
            next_id: 1,
        };
        let result = client
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": "bugsplat-cli",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                }),
            )
            .await?;
        let negotiated = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(PROTOCOL_VERSION);
        client.protocol_version = Some(negotiated.to_string());
        client.notify("notifications/initialized").await?;
        Ok(client)
    }

    pub async fn list_tools(&mut self) -> Result<Vec<Tool>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = cursor
                .as_ref()
                .map_or(Value::Null, |c| json!({"cursor": c}));
            let mut result = self.request("tools/list", params).await?;
            let page = result.get_mut("tools").map(Value::take).unwrap_or_default();
            if !page.is_null() {
                tools.extend(serde_json::from_value::<Vec<Tool>>(page)?);
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
                .map(str::to_string);
            if cursor.is_none() {
                return Ok(tools);
            }
        }
    }

    /// `arguments` is the tool's argument object. A result with `isError` is returned,
    /// not turned into an `Err`; the caller decides how to report it.
    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<ToolResult> {
        let arguments = if arguments.is_null() {
            json!({})
        } else {
            arguments
        };
        let raw = self
            .request("tools/call", json!({"name": name, "arguments": arguments}))
            .await?;
        Ok(ToolResult::from_raw(raw))
    }

    /// Ends the session so the server can drop it. Failures are ignored: by now the
    /// caller has its answer, and the server expires sessions on its own.
    pub async fn close(self) {
        let Some(session_id) = &self.session_id else {
            return;
        };
        let _ = self
            .http
            .delete(self.endpoint.clone())
            .timeout(CLOSE_TIMEOUT)
            .bearer_auth(&self.bearer_token)
            .header(SESSION_HEADER, session_id)
            .send()
            .await;
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut body = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if !params.is_null() {
            body["params"] = params;
        }
        let response = self.post(&body).await?;
        if self.session_id.is_none() {
            self.session_id = response
                .headers()
                .get(SESSION_HEADER)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
        }
        let message = if is_event_stream(&response) {
            read_event_stream(response, id).await?
        } else {
            serde_json::from_slice(&response.bytes().await?).map_err(|_| not_json_rpc())?
        };
        into_result(message)
    }

    async fn notify(&self, method: &str) -> Result<()> {
        self.post(&json!({"jsonrpc": "2.0", "method": method}))
            .await?;
        Ok(())
    }

    async fn post(&self, body: &Value) -> Result<Response> {
        let mut request = self
            .http
            .post(self.endpoint.clone())
            .timeout(REQUEST_TIMEOUT)
            .bearer_auth(&self.bearer_token)
            .header(ACCEPT, "application/json, text/event-stream")
            .json(body);
        if let Some(session_id) = &self.session_id {
            request = request.header(SESSION_HEADER, session_id);
        }
        if let Some(version) = &self.protocol_version {
            request = request.header(PROTOCOL_HEADER, version);
        }
        let response = request.send().await?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(Error::NotSignedIn);
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::api(
                status.as_u16(),
                http_error_message(status, &body),
            ));
        }
        Ok(response)
    }
}

fn is_event_stream(response: &Response) -> bool {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim_start().starts_with("text/event-stream"))
}

/// Reads events until the response to request `id` arrives. The server may send its own
/// notifications and requests on the same stream first; those are skipped.
async fn read_event_stream(mut response: Response, id: u64) -> Result<Value> {
    let mut parser = SseParser::default();
    while let Some(chunk) = response.chunk().await? {
        if let Some(message) = find_response(parser.push(&chunk), id) {
            return Ok(message);
        }
    }
    find_response(parser.finish(), id).ok_or_else(|| {
        Error::api(
            None,
            "The MCP server closed the connection before answering",
        )
    })
}

fn find_response(events: Vec<String>, id: u64) -> Option<Value> {
    events
        .iter()
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .find(|message| {
            message.get("id").and_then(Value::as_u64) == Some(id)
                && (message.get("result").is_some() || message.get("error").is_some())
        })
}

fn into_result(message: Value) -> Result<Value> {
    let Value::Object(mut message) = message else {
        return Err(not_json_rpc());
    };
    if let Some(error) = message.get("error") {
        let text = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("The MCP server returned an error without a message");
        return Err(Error::api(None, text));
    }
    message.remove("result").ok_or_else(not_json_rpc)
}

fn not_json_rpc() -> Error {
    Error::api(
        None,
        "The MCP server sent a response that is not a JSON-RPC result",
    )
}

/// The server's own words when the body has them (a JSON-RPC error, or the
/// `message`/`error_description` of an OAuth-style error), otherwise the status.
fn http_error_message(status: StatusCode, body: &str) -> String {
    let parsed = serde_json::from_str::<Value>(body).ok();
    let text = parsed.as_ref().and_then(|body| {
        body.pointer("/error/message")
            .or_else(|| body.get("message"))
            .or_else(|| body.get("error_description"))
            .and_then(Value::as_str)
    });
    match text {
        Some(text) if !text.is_empty() => text.to_string(),
        _ => format!("The MCP server answered HTTP {status}"),
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockBuilder, MockServer, Request, ResponseTemplate};

    use super::*;

    const TOKEN: &str = "test-token";
    const SESSION: &str = "session-1";

    fn rpc(method_name: &str) -> MockBuilder {
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(body_partial_json(json!({"method": method_name})))
    }

    fn body(request: &Request) -> Value {
        serde_json::from_slice(&request.body).unwrap()
    }

    fn result(request: &Request, result: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": body(request)["id"],
            "result": result,
        }))
    }

    async fn server() -> MockServer {
        let server = MockServer::start().await;
        rpc("initialize")
            .respond_with(|request: &Request| {
                result(
                    request,
                    json!({
                        "protocolVersion": "2025-06-18",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "BugSplat", "version": "test"},
                    }),
                )
                .insert_header(SESSION_HEADER, SESSION)
            })
            .mount(&server)
            .await;
        rpc("notifications/initialized")
            .respond_with(ResponseTemplate::new(202))
            .mount(&server)
            .await;
        server
    }

    async fn connect(server: &MockServer) -> McpClient {
        let endpoint = Url::parse(&format!("{}/mcp", server.uri())).unwrap();
        McpClient::connect(endpoint, TOKEN).await.unwrap()
    }

    async fn requests(server: &MockServer, method_name: &str) -> Vec<Request> {
        server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|request| {
                serde_json::from_slice::<Value>(&request.body)
                    .is_ok_and(|body| body["method"] == method_name)
            })
            .collect()
    }

    fn header_value<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
        request.headers.get(name).and_then(|v| v.to_str().ok())
    }

    #[tokio::test]
    async fn connect_runs_the_handshake() {
        let server = server().await;
        connect(&server).await;

        let initialize = &requests(&server, "initialize").await[0];
        let sent = body(initialize);
        assert_eq!(sent["jsonrpc"], "2.0");
        assert_eq!(sent["params"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(sent["params"]["clientInfo"]["name"], "bugsplat-cli");
        assert_eq!(
            header_value(initialize, "authorization"),
            Some("Bearer test-token")
        );
        assert_eq!(
            header_value(initialize, "accept"),
            Some("application/json, text/event-stream")
        );
        assert_eq!(header_value(initialize, SESSION_HEADER), None);
        assert_eq!(header_value(initialize, PROTOCOL_HEADER), None);

        let initialized = &requests(&server, "notifications/initialized").await[0];
        assert!(body(initialized).get("id").is_none());
        assert_eq!(header_value(initialized, SESSION_HEADER), Some(SESSION));
        assert_eq!(
            header_value(initialized, PROTOCOL_HEADER),
            Some("2025-06-18")
        );
    }

    #[tokio::test]
    async fn sends_the_version_the_server_negotiated() {
        let server = MockServer::start().await;
        rpc("initialize")
            .respond_with(|request: &Request| {
                result(request, json!({"protocolVersion": "2025-03-26"}))
            })
            .mount(&server)
            .await;
        rpc("notifications/initialized")
            .respond_with(ResponseTemplate::new(202))
            .mount(&server)
            .await;
        connect(&server).await;

        let initialized = &requests(&server, "notifications/initialized").await[0];
        assert_eq!(
            header_value(initialized, PROTOCOL_HEADER),
            Some("2025-03-26")
        );
        assert_eq!(header_value(initialized, SESSION_HEADER), None);
    }

    #[tokio::test]
    async fn calls_a_tool_with_a_json_response() {
        let server = server().await;
        rpc("tools/call")
            .and(header(SESSION_HEADER, SESSION))
            .and(header(PROTOCOL_HEADER, "2025-06-18"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(|request: &Request| {
                result(
                    request,
                    json!({
                        "content": [
                            {"type": "text", "text": "first"},
                            {"type": "image", "data": "", "mimeType": "image/png"},
                            {"type": "text", "text": "second"},
                        ],
                        "isError": false,
                    }),
                )
            })
            .mount(&server)
            .await;
        let mut client = connect(&server).await;

        let result = client
            .call_tool("list-groups", json!({"pageSize": 2}))
            .await
            .unwrap();

        assert_eq!(result.text, "first\nsecond");
        assert_eq!(result.structured, None);
        assert!(!result.is_error);
        assert_eq!(result.raw["content"][0]["text"], "first");
        let call = &requests(&server, "tools/call").await[0];
        assert_eq!(
            body(call)["params"],
            json!({"name": "list-groups", "arguments": {"pageSize": 2}})
        );
    }

    #[tokio::test]
    async fn null_arguments_are_sent_as_an_empty_object() {
        let server = server().await;
        rpc("tools/call")
            .respond_with(|request: &Request| result(request, json!({"content": []})))
            .mount(&server)
            .await;
        let mut client = connect(&server).await;

        client
            .call_tool("list-databases", Value::Null)
            .await
            .unwrap();

        let call = &requests(&server, "tools/call").await[0];
        assert_eq!(body(call)["params"]["arguments"], json!({}));
    }

    #[tokio::test]
    async fn reads_structured_content() {
        let server = server().await;
        rpc("tools/call")
            .respond_with(|request: &Request| {
                result(
                    request,
                    json!({
                        "content": [{"type": "text", "text": "1 group"}],
                        "structuredContent": {"groups": [{"id": 7}]},
                    }),
                )
            })
            .mount(&server)
            .await;
        let mut client = connect(&server).await;

        let result = client.call_tool("list-groups", json!({})).await.unwrap();

        assert_eq!(result.structured, Some(json!({"groups": [{"id": 7}]})));
    }

    #[tokio::test]
    async fn calls_a_tool_with_an_sse_response() {
        let server = server().await;
        rpc("tools/call")
            .respond_with(|request: &Request| {
                let notification = json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/message",
                    "params": {"level": "info", "data": "working"},
                });
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": body(request)["id"],
                    "result": {"content": [{"type": "text", "text": "from a stream"}]},
                });
                let stream = format!(
                    ": keep-alive\n\nevent: message\ndata: {notification}\n\nid: 1\nevent: message\ndata: {response}\n\n"
                );
                ResponseTemplate::new(200).set_body_raw(stream, "text/event-stream; charset=utf-8")
            })
            .mount(&server)
            .await;
        let mut client = connect(&server).await;

        let result = client.call_tool("list-groups", json!({})).await.unwrap();

        assert_eq!(result.text, "from a stream");
    }

    #[tokio::test]
    async fn an_sse_stream_that_ends_without_an_answer_is_an_error() {
        let server = server().await;
        rpc("tools/call")
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(": nothing\n\n", "text/event-stream"),
            )
            .mount(&server)
            .await;
        let mut client = connect(&server).await;

        let error = client
            .call_tool("list-groups", json!({}))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("before answering"), "{error}");
    }

    #[tokio::test]
    async fn a_tool_error_is_returned_not_raised() {
        let server = server().await;
        rpc("tools/call")
            .respond_with(|request: &Request| {
                result(
                    request,
                    json!({
                        "content": [{"type": "text", "text": "Crash #9 was not found"}],
                        "isError": true,
                    }),
                )
            })
            .mount(&server)
            .await;
        let mut client = connect(&server).await;

        let result = client
            .call_tool("get-report", json!({"id": 9}))
            .await
            .unwrap();

        assert!(result.is_error);
        assert_eq!(result.text, "Crash #9 was not found");
    }

    #[tokio::test]
    async fn a_json_rpc_error_carries_the_servers_message() {
        let server = server().await;
        rpc("tools/call")
            .respond_with(|request: &Request| {
                ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0",
                    "id": body(request)["id"],
                    "error": {"code": -32602, "message": "Tool not found: \"nope\"."},
                }))
            })
            .mount(&server)
            .await;
        let mut client = connect(&server).await;

        let error = client.call_tool("nope", json!({})).await.unwrap_err();

        assert!(matches!(error, Error::Api { status: None, .. }));
        assert_eq!(error.to_string(), "Tool not found: \"nope\".");
    }

    #[tokio::test]
    async fn a_401_is_not_signed_in() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "message": "Send a BugSplat access token in the Authorization: Bearer header",
            })))
            .mount(&server)
            .await;
        let endpoint = Url::parse(&format!("{}/mcp", server.uri())).unwrap();

        let error = McpClient::connect(endpoint, TOKEN).await.unwrap_err();

        assert!(matches!(error, Error::NotSignedIn));
    }

    #[tokio::test]
    async fn a_401_after_connecting_is_not_signed_in() {
        let server = server().await;
        rpc("tools/call")
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let mut client = connect(&server).await;

        let error = client
            .call_tool("list-groups", json!({}))
            .await
            .unwrap_err();

        assert!(matches!(error, Error::NotSignedIn));
    }

    #[tokio::test]
    async fn another_http_error_carries_the_status_and_the_servers_message() {
        let server = server().await;
        rpc("tools/list")
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "jsonrpc": "2.0",
                "error": {"code": -32600, "message": "Session not found or has expired."},
            })))
            .mount(&server)
            .await;
        let mut client = connect(&server).await;

        let error = client.list_tools().await.unwrap_err();

        assert!(matches!(
            error,
            Error::Api {
                status: Some(404),
                ..
            }
        ));
        assert_eq!(error.to_string(), "Session not found or has expired.");
    }

    #[tokio::test]
    async fn an_http_error_without_a_message_names_the_status() {
        let server = server().await;
        rpc("tools/list")
            .respond_with(ResponseTemplate::new(502).set_body_string("<html>Bad Gateway</html>"))
            .mount(&server)
            .await;
        let mut client = connect(&server).await;

        let error = client.list_tools().await.unwrap_err();

        assert!(error.to_string().contains("502"), "{error}");
    }

    #[tokio::test]
    async fn a_body_that_is_not_json_rpc_is_an_error() {
        let server = server().await;
        rpc("tools/list")
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>hello</html>"))
            .mount(&server)
            .await;
        let mut client = connect(&server).await;

        let error = client.list_tools().await.unwrap_err();

        assert!(
            error.to_string().contains("not a JSON-RPC result"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn lists_tools_across_pages() {
        let server = server().await;
        rpc("tools/list")
            .and(body_partial_json(json!({"params": {"cursor": "page-2"}})))
            .respond_with(|request: &Request| {
                result(
                    request,
                    json!({"tools": [{"name": "get-report", "inputSchema": {"type": "object"}}]}),
                )
            })
            .with_priority(1)
            .mount(&server)
            .await;
        rpc("tools/list")
            .respond_with(|request: &Request| {
                result(
                    request,
                    json!({
                        "tools": [{
                            "name": "list-reports",
                            "title": "List reports",
                            "description": "List crash reports",
                            "inputSchema": {"type": "object", "properties": {}},
                        }],
                        "nextCursor": "page-2",
                    }),
                )
            })
            .mount(&server)
            .await;
        let mut client = connect(&server).await;

        let tools = client.list_tools().await.unwrap();

        assert_eq!(
            tools,
            vec![
                Tool {
                    name: "list-reports".into(),
                    title: Some("List reports".into()),
                    description: Some("List crash reports".into()),
                    input_schema: json!({"type": "object", "properties": {}}),
                },
                Tool {
                    name: "get-report".into(),
                    title: None,
                    description: None,
                    input_schema: json!({"type": "object"}),
                },
            ]
        );
        let first = &requests(&server, "tools/list").await[0];
        assert!(body(first).get("params").is_none());
    }

    #[tokio::test]
    async fn request_ids_are_unique() {
        let server = server().await;
        rpc("tools/call")
            .respond_with(|request: &Request| result(request, json!({"content": []})))
            .mount(&server)
            .await;
        let mut client = connect(&server).await;
        client.call_tool("a", json!({})).await.unwrap();
        client.call_tool("b", json!({})).await.unwrap();

        let ids: std::collections::BTreeSet<u64> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter_map(|request| body(request)["id"].as_u64())
            .collect();
        assert_eq!(ids.len(), 3);
    }

    #[tokio::test]
    async fn close_deletes_the_session() {
        let server = server().await;
        Mock::given(method("DELETE"))
            .and(path("/mcp"))
            .and(header(SESSION_HEADER, SESSION))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        connect(&server).await.close().await;
    }

    #[tokio::test]
    async fn debug_hides_the_token() {
        let server = server().await;
        let client = connect(&server).await;

        assert!(!format!("{client:?}").contains(TOKEN));
    }
}

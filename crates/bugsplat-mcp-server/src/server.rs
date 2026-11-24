use std::future::Future;
use std::sync::Arc;

use bugsplat_core::{Config, Error, Result};
use bugsplat_upload::TokenProvider;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};

use crate::tools::{self, ToolError, ToolResult, find, info, upload};

/// Serves the local tools on stdin and stdout until the client closes stdin.
pub async fn serve_stdio(config: Config) -> Result<()> {
    let running = LocalServer::new(config)
        .serve(rmcp::transport::stdio_pipes())
        .await
        .map_err(|err| Error::Other(format!("The MCP handshake failed: {err}")))?;
    running
        .waiting()
        .await
        .map_err(|err| Error::Other(format!("The MCP server stopped unexpectedly: {err}")))?;
    Ok(())
}

struct LocalServer {
    config: Config,
    token: TokenProvider,
}

impl LocalServer {
    fn new(config: Config) -> Self {
        let for_token = config.clone();
        let token: TokenProvider = Arc::new(move || {
            let config = for_token.clone();
            Box::pin(async move { Ok(bugsplat_auth::access_token(&config).await?.secret) })
        });
        Self { config, token }
    }

    fn tools() -> Vec<Tool> {
        vec![info::tool(), find::tool(), upload::tool()]
    }

    fn instructions(&self) -> String {
        format!(
            "Tools for the symbol files on this computer, which BugSplat's hosted MCP server \
             can't see. symbols-info reads the debug id, module name, kind and architecture of \
             files, symbols-find searches folders for symbol files by debug id or name, and \
             symbols-upload uploads symbol files to BugSplat for an application version. Crash \
             data does not come from this server: crash reports, crash groups, versions and the \
             symbols BugSplat already has come from the hosted BugSplat server at {hosted}, a \
             separate connection with tools such as get-report, list-versions and list-symbols. \
             To fix a crash that is missing symbols, read the missing modules and their debug \
             ids from get-report there, locate the files here with symbols-find, and send them \
             with symbols-upload for the application and version the report names. Pass absolute \
             paths. symbols-upload writes to BugSplat, so run it with dryRun first and confirm \
             with the user; it uses the session from `bugsplat login`, which the user runs in a \
             terminal.",
            hosted = self.config.mcp_url()
        )
    }

    /// `None` when there is no tool called `name`.
    async fn call(
        &self,
        name: &str,
        arguments: Option<JsonObject>,
        cancelled: impl Future<Output = ()> + Send,
    ) -> Option<CallToolResult> {
        let outcome = match name {
            info::NAME => match tools::arguments(arguments) {
                Ok(args) => info::run(args).await,
                Err(err) => Err(err),
            },
            find::NAME => match tools::arguments(arguments) {
                Ok(args) => find::run(args, cancelled).await,
                Err(err) => Err(err),
            },
            upload::NAME => match tools::arguments(arguments) {
                Ok(args) => {
                    let access = upload::Access {
                        config: &self.config,
                        token: &self.token,
                        quiet: self.config.quiet,
                    };
                    upload::run(args, access, cancelled).await
                }
                Err(err) => Err(err),
            },
            _ => return None,
        };
        Some(result(outcome))
    }
}

/// The JSON goes out twice, as structured content and as text, because clients differ in
/// which of the two they hand to the model.
fn result(outcome: ToolResult) -> CallToolResult {
    match outcome {
        Ok(value) => CallToolResult::structured(value),
        Err(ToolError(message)) => CallToolResult::error(vec![ContentBlock::text(message)]),
    }
}

impl ServerHandler for LocalServer {
    fn get_info(&self) -> ServerConfig {
        let mut server = Implementation::new("bugsplat-cli", env!("CARGO_PKG_VERSION"));
        server.title = Some("BugSplat (local symbol files)".to_string());
        server.website_url = Some("https://docs.bugsplat.com".to_string());
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(server)
            .with_instructions(self.instructions())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(Self::tools()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        Self::tools().into_iter().find(|tool| tool.name == name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        let cancelled = context.ct.cancelled_owned();
        match self.call(&request.name, request.arguments, cancelled).await {
            Some(result) => Ok(result.into()),
            None => Err(ErrorData::invalid_params(
                format!("Unknown tool: {}", request.name),
                None,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;

    use serde_json::{Value, json};

    use super::*;
    use crate::tools::testing::{fixture, object, text};

    /// Never reaches a server: the tests here stop before any request.
    fn server() -> LocalServer {
        LocalServer {
            config: Config::new(Some("http://127.0.0.1:9")).unwrap(),
            token: Arc::new(|| Box::pin(async { Err(Error::NotSignedIn) })),
        }
    }

    async fn call(name: &str, arguments: Value) -> CallToolResult {
        server()
            .call(name, object(arguments), pending())
            .await
            .unwrap()
    }

    fn only_text(result: &CallToolResult) -> &str {
        assert_eq!(result.content.len(), 1);
        &result.content[0].as_text().unwrap().text
    }

    #[test]
    fn the_tools_are_named_titled_described_and_annotated_like_the_hosted_servers() {
        let tools = LocalServer::tools();
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
        assert_eq!(names, ["symbols-info", "symbols-find", "symbols-upload"]);

        for tool in &tools {
            let name = &tool.name;
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "{name} is not kebab-case"
            );
            let title = tool.title.as_deref().unwrap();
            assert!(title.ends_with("Symbol Files"), "{name}: {title}");
            assert!(tool.description.as_deref().unwrap().len() > 100, "{name}");
            assert!(tool.output_schema.is_none(), "{name}");

            let hints = tool.annotations.as_ref().unwrap();
            assert_eq!(hints.title.as_deref(), Some(title), "{name}");
            assert_eq!(
                hints.read_only_hint,
                Some(name != "symbols-upload"),
                "{name}"
            );
            assert_eq!(hints.destructive_hint, Some(false), "{name}");
            assert_eq!(hints.open_world_hint, Some(false), "{name}");
        }
    }

    #[test]
    fn every_property_is_described_and_required_ones_exist() {
        for tool in LocalServer::tools() {
            let name = &tool.name;
            let schema = &tool.input_schema;
            assert_eq!(schema["type"], "object", "{name}");
            let properties = schema["properties"].as_object().unwrap();
            for (property, definition) in properties {
                assert!(definition["type"].is_string(), "{name}.{property}");
                let description = definition["description"].as_str().unwrap_or_default();
                assert!(description.len() > 10, "{name}.{property}");
                assert!(
                    !property.contains('_') && !property.contains('-'),
                    "{name}.{property} is not camelCase"
                );
            }
            for required in schema["required"].as_array().unwrap() {
                assert!(
                    properties.contains_key(required.as_str().unwrap()),
                    "{name} requires {required}, which it does not describe"
                );
            }
        }
    }

    /// The schemas are written by hand, so this is what keeps them and the argument structs
    /// from drifting apart: a call that sets every described property must get past argument
    /// parsing.
    #[tokio::test]
    async fn every_described_property_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        for tool in LocalServer::tools() {
            let mut arguments = JsonObject::new();
            for (property, definition) in tool.input_schema["properties"].as_object().unwrap() {
                let value = match definition["type"].as_str().unwrap() {
                    "array" => json!([text(dir.path())]),
                    "integer" => json!(1),
                    "boolean" => json!(true),
                    _ => json!(text(dir.path())),
                };
                arguments.insert(property.clone(), value);
            }

            let result = server()
                .call(&tool.name, Some(arguments), pending())
                .await
                .unwrap();

            let text = result
                .content
                .first()
                .and_then(|content| content.as_text())
                .map(|content| content.text.clone())
                .unwrap_or_default();
            assert!(
                !text.starts_with("Invalid arguments"),
                "{}: {text}",
                tool.name
            );
        }
    }

    #[test]
    fn the_instructions_say_what_the_tools_are_for_and_where_crash_data_is() {
        let info = server().get_info();

        assert!(info.capabilities.tools.is_some());
        assert_eq!(info.server_info.name, "bugsplat-cli");
        assert_eq!(info.server_info.version, env!("CARGO_PKG_VERSION"));
        let instructions = info.instructions.unwrap();
        for tool in LocalServer::tools() {
            assert!(instructions.contains(tool.name.as_ref()), "{}", tool.name);
        }
        assert!(instructions.contains("Crash data does not come from this server"));
        assert!(instructions.contains("http://127.0.0.1:9/mcp"));
        assert!(instructions.contains("`bugsplat login`"));
    }

    #[tokio::test]
    async fn a_result_is_json_as_text_and_as_structured_content() {
        let pdb = fixture("pdb-guid/bugsplat.pdb");

        let result = call("symbols-info", json!({ "paths": [text(&pdb)] })).await;

        assert_eq!(result.is_error, Some(false));
        let structured = result.structured_content.clone().unwrap();
        assert_eq!(
            structured["objects"][0]["debugId"],
            "E546B55B6D214E86871B40AC35CD0D461"
        );
        let from_text: Value = serde_json::from_str(only_text(&result)).unwrap();
        assert_eq!(from_text, structured);
    }

    #[tokio::test]
    async fn a_failure_is_an_error_result_the_model_can_read() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");

        let result = call("symbols-find", json!({ "paths": [text(&missing)] })).await;

        assert_eq!(result.is_error, Some(true));
        assert!(result.structured_content.is_none());
        assert_eq!(
            only_text(&result),
            format!("{}: does not exist", missing.display())
        );
    }

    #[tokio::test]
    async fn bad_arguments_are_an_error_result_not_a_protocol_error() {
        let result = call("symbols-find", json!({ "path": "/tmp" })).await;

        assert_eq!(result.is_error, Some(true));
        let message = only_text(&result);
        assert!(message.starts_with("Invalid arguments:"), "{message}");

        let none = server()
            .call("symbols-info", None, pending())
            .await
            .unwrap();
        assert_eq!(none.is_error, Some(true));
        assert!(only_text(&none).contains("paths"));
    }

    #[tokio::test]
    async fn an_upload_without_a_session_points_at_bugsplat_login() {
        let dir = tempfile::tempdir().unwrap();

        let result = call(
            "symbols-upload",
            json!({
                "application": "MyApp",
                "version": "1.0.0",
                "directory": text(dir.path()),
                "files": "*.pdb",
                "database": "fred",
            }),
        )
        .await;

        assert_eq!(result.is_error, Some(true));
        assert!(only_text(&result).contains("`bugsplat login` in a terminal"));
    }

    #[tokio::test]
    async fn an_unknown_tool_is_not_one_of_ours() {
        assert!(
            server()
                .call("symbols-delete", None, pending())
                .await
                .is_none()
        );
    }
}

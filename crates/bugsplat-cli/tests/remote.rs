//! The remote verbs, run as the real binary against a mock MCP server.

use assert_cmd::Command;
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockBuilder, MockServer, Request, ResponseTemplate};

const TOKEN: &str = "remote-test-token";
const SESSION: &str = "remote-test-session";

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn rpc(name: &str) -> MockBuilder {
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(body_partial_json(json!({"method": name})))
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

fn text_result(request: &Request, text: &str) -> ResponseTemplate {
    result(
        request,
        json!({"content": [{"type": "text", "text": text}], "isError": false}),
    )
}

fn tools() -> Value {
    let filter = json!({"oneOf": [
        {"type": "string"},
        {"type": "array", "items": {"type": "string"}, "minItems": 1},
        {"type": "object", "minProperties": 1},
    ]});
    json!([
        {
            "name": "list-databases",
            "description": "List the databases this user can access.",
            "inputSchema": {"type": "object", "properties": {}, "required": []},
        },
        {
            "name": "list-reports",
            "description": "List individual crash reports.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "database": {"type": "string", "description": "Database to read"},
                    "application": filter,
                    "version": filter,
                    "startDate": {"type": "string"},
                    "pageSize": {"type": "integer", "minimum": 1, "maximum": 100},
                    "page": {"type": "integer", "minimum": 1},
                },
                "required": [],
            },
        },
        {
            "name": "get-report",
            "description": "Get one crash report in full.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "integer", "description": "Crash report id"},
                    "database": {"type": "string", "description": "Database to read"},
                },
                "required": ["id"],
            },
        },
    ])
}

/// A mock MCP server whose `tools/call` answers with `call`.
async fn server_with<F>(call: F) -> MockServer
where
    F: Fn(&Request) -> ResponseTemplate + Send + Sync + 'static,
{
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
            .insert_header("Mcp-Session-Id", SESSION)
        })
        .mount(&server)
        .await;
    rpc("notifications/initialized")
        .respond_with(ResponseTemplate::new(202))
        .mount(&server)
        .await;
    rpc("tools/list")
        .respond_with(|request: &Request| result(request, json!({"tools": tools()})))
        .mount(&server)
        .await;
    rpc("tools/call").respond_with(call).mount(&server).await;
    Mock::given(method("DELETE"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    server
}

async fn server() -> MockServer {
    server_with(|request| text_result(request, "ok")).await
}

async fn bugsplat_with(api_url: String, env: &[(&str, &str)], args: &[&str]) -> Run {
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let mut command = Command::cargo_bin("bugsplat").unwrap();
        for name in [
            "BUGSPLAT_ACCESS_TOKEN",
            "BUGSPLAT_API_URL",
            "BUGSPLAT_HOST",
            "BUGSPLAT_DATABASE",
            "BUGSPLAT_CLIENT_ID",
            "BUGSPLAT_CLIENT_SECRET",
            "SYMBOL_UPLOAD_CLIENT_ID",
            "SYMBOL_UPLOAD_CLIENT_SECRET",
        ] {
            command.env_remove(name);
        }
        command.env("BUGSPLAT_TOKEN_STORE", "file");
        command.env(
            "BUGSPLAT_CONFIG_DIR",
            std::env::temp_dir().join("bugsplat-cli-tests"),
        );
        command.env("BUGSPLAT_API_URL", api_url);
        command.envs(env);
        let output = command.args(args).output().unwrap();
        Run {
            code: output.status.code().unwrap(),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
        }
    })
    .await
    .unwrap()
}

async fn bugsplat(server: &MockServer, args: &[&str]) -> Run {
    bugsplat_with(server.uri(), &[("BUGSPLAT_ACCESS_TOKEN", TOKEN)], args).await
}

async fn requests(server: &MockServer, name: &str) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| {
            serde_json::from_slice::<Value>(&request.body).is_ok_and(|body| body["method"] == name)
        })
        .collect()
}

/// Runs the command against a fresh server and returns the `params` of its one `tools/call`.
async fn call_params(args: &[&str]) -> Value {
    let server = server().await;
    let run = bugsplat(&server, args).await;
    assert_eq!(run.code, 0, "{args:?} failed: {}", run.stderr);
    let calls = requests(&server, "tools/call").await;
    assert_eq!(calls.len(), 1, "{args:?} made {} tool calls", calls.len());
    body(&calls[0])["params"].clone()
}

async fn assert_call(args: &[&str], tool: &str, arguments: Value) {
    assert_eq!(
        call_params(args).await,
        json!({"name": tool, "arguments": arguments}),
        "{args:?}"
    );
}

#[tokio::test]
async fn databases_list_maps_its_flags() {
    assert_call(&["databases", "list"], "list-databases", json!({})).await;
    assert_call(
        &[
            "-b",
            "Fred",
            "databases",
            "list",
            "--name",
            "fr",
            "--company",
            "Public",
        ],
        "list-databases",
        json!({"name": "fr", "company": "Public"}),
    )
    .await;
}

#[tokio::test]
async fn reports_list_maps_its_flags() {
    assert_call(&["reports", "list"], "list-reports", json!({})).await;
    assert_call(
        &[
            "reports",
            "list",
            "--database",
            "Fred",
            "--application",
            "MyGame",
            "--version",
            r#"{"ne":"1.0"}"#,
            "--stack-group",
            "*memcpy*",
            "--stack-key-id",
            "-1",
            "--user",
            r#"["fred","barney"]"#,
            "--email",
            "*@bedrock.com",
            "--description",
            "froze",
            "--exception-code",
            "c0000005",
            "--exception-message",
            "Access violation",
            "--attribute",
            r#"GPU={"startsWith":"GeForce"}"#,
            "--attribute",
            "platform=steam",
            "--start-date",
            "2026-11-01",
            "--end-date",
            "2026-11-16T12:00:00Z",
            "--page-size",
            "50",
            "--page",
            "2",
            "--sort-by",
            "id",
            "--sort-order",
            "asc",
        ],
        "list-reports",
        json!({
            "database": "Fred",
            "application": "MyGame",
            "version": {"ne": "1.0"},
            "stackGroup": "*memcpy*",
            "stackKeyId": -1,
            "user": ["fred", "barney"],
            "email": "*@bedrock.com",
            "description": "froze",
            "exceptionCode": "c0000005",
            "exceptionMessage": "Access violation",
            "attributes": {"GPU": {"startsWith": "GeForce"}, "platform": "steam"},
            "startDate": "2026-11-01",
            "endDate": "2026-11-16T12:00:00Z",
            "pageSize": 50,
            "page": 2,
            "sortBy": "id",
            "sortOrder": "asc",
        }),
    )
    .await;
    assert_call(
        &["reports", "list", "--stack-key-id", r#"{"in":[7,8]}"#],
        "list-reports",
        json!({"stackKeyId": {"in": [7, 8]}}),
    )
    .await;
}

#[tokio::test]
async fn reports_get_maps_its_arguments() {
    assert_call(&["reports", "get", "42"], "get-report", json!({"id": 42})).await;
    assert_call(
        &["reports", "get", "42", "-b", "Fred"],
        "get-report",
        json!({"id": 42, "database": "Fred"}),
    )
    .await;
}

#[tokio::test]
async fn groups_list_maps_its_flags() {
    assert_call(&["groups", "list"], "list-groups", json!({})).await;
    assert_call(
        &[
            "groups",
            "list",
            "-b",
            "Fred",
            "--application",
            "MyGame",
            "--version",
            "2.1.*",
            "--status",
            r#"["open","regression"]"#,
            "--stack-key",
            "memcpy",
            "--crashes",
            r#"{"gte":10,"lt":100}"#,
            "--users",
            "5",
            "--start-date",
            "2026-11-01",
            "--end-date",
            "2026-11-16",
            "--linked-defect",
            "false",
            "--page-size",
            "25",
            "--page",
            "3",
            "--sort-by",
            "users",
            "--sort-order",
            "desc",
        ],
        "list-groups",
        json!({
            "database": "Fred",
            "application": "MyGame",
            "version": "2.1.*",
            "status": ["open", "regression"],
            "stackKey": "memcpy",
            "crashes": {"gte": 10, "lt": 100},
            "users": 5,
            "startDate": "2026-11-01",
            "endDate": "2026-11-16",
            "linkedDefect": false,
            "pageSize": 25,
            "page": 3,
            "sortBy": "users",
            "sortOrder": "desc",
        }),
    )
    .await;
    assert_call(
        &[
            "groups",
            "list",
            "--status",
            "open",
            "--linked-defect",
            "true",
        ],
        "list-groups",
        json!({"status": "open", "linkedDefect": true}),
    )
    .await;
}

#[tokio::test]
async fn groups_get_and_set_status_map_their_arguments() {
    assert_call(
        &["groups", "get", "7"],
        "get-group",
        json!({"stackKeyId": 7}),
    )
    .await;
    assert_call(
        &["groups", "get", "7", "--page-size", "3", "-b", "Fred"],
        "get-group",
        json!({"stackKeyId": 7, "pageSize": 3, "database": "Fred"}),
    )
    .await;
    for status in ["open", "fixed", "regression", "ignored"] {
        assert_call(
            &["groups", "set-status", "7", status, "-b", "Fred"],
            "set-group-status",
            json!({"stackKeyId": 7, "status": status, "database": "Fred"}),
        )
        .await;
    }
}

#[tokio::test]
async fn versions_list_maps_its_flags() {
    assert_call(&["versions", "list"], "list-versions", json!({})).await;
    assert_call(
        &[
            "versions",
            "list",
            "-b",
            "Fred",
            "--application",
            r#"{"in":["MyGame","MyTool"]}"#,
            "--version",
            "1.*",
            "--include-retired",
            "--start-date",
            "2026-01-01",
            "--end-date",
            "2026-06-30",
            "--page-size",
            "5",
            "--page",
            "2",
            "--sort-by",
            "symbolSize",
            "--sort-order",
            "asc",
        ],
        "list-versions",
        json!({
            "database": "Fred",
            "application": {"in": ["MyGame", "MyTool"]},
            "version": "1.*",
            "includeRetired": true,
            "startDate": "2026-01-01",
            "endDate": "2026-06-30",
            "pageSize": 5,
            "page": 2,
            "sortBy": "symbolSize",
            "sortOrder": "asc",
        }),
    )
    .await;
}

#[tokio::test]
async fn symbols_list_maps_its_flags() {
    assert_call(&["symbols", "list"], "list-symbols", json!({})).await;
    assert_call(
        &[
            "symbols",
            "list",
            "-b",
            "Fred",
            "--application",
            "MyGame",
            "--version",
            "1.0",
            "--name",
            "myapp.pdb",
            "--debug-id",
            "E546B55B6D214E86871B40AC35CD0D461",
            "--page-size",
            "10",
            "--page",
            "4",
            "--sort-by",
            "size",
            "--sort-order",
            "asc",
        ],
        "list-symbols",
        json!({
            "database": "Fred",
            "application": "MyGame",
            "version": "1.0",
            "name": "myapp.pdb",
            "debugId": "E546B55B6D214E86871B40AC35CD0D461",
            "pageSize": 10,
            "page": 4,
            "sortBy": "size",
            "sortOrder": "asc",
        }),
    )
    .await;
}

#[tokio::test]
async fn defects_map_their_arguments() {
    assert_call(
        &["defects", "create", "7", "--yes"],
        "create-defect",
        json!({"stackKeyId": 7}),
    )
    .await;
    assert_call(
        &[
            "defects",
            "create",
            "7",
            "--notes",
            "Seen in QA",
            "-y",
            "-b",
            "Fred",
        ],
        "create-defect",
        json!({"stackKeyId": 7, "notes": "Seen in QA", "database": "Fred"}),
    )
    .await;
    assert_call(
        &[
            "defects",
            "link",
            "7",
            "PROJ-123",
            "--notes",
            "Same crash",
            "--yes",
            "-b",
            "Fred",
        ],
        "add-defect-link",
        json!({"stackKeyId": 7, "defectId": "PROJ-123", "notes": "Same crash", "database": "Fred"}),
    )
    .await;
    assert_call(
        &["defects", "link", "7", "45", "-y"],
        "add-defect-link",
        json!({"stackKeyId": 7, "defectId": "45"}),
    )
    .await;
    assert_call(
        &["defects", "unlink", "7", "-b", "Fred"],
        "remove-defect-link",
        json!({"stackKeyId": 7, "database": "Fred"}),
    )
    .await;
}

#[tokio::test]
async fn the_database_can_come_from_the_environment() {
    let server = server().await;
    let env = [
        ("BUGSPLAT_ACCESS_TOKEN", TOKEN),
        ("BUGSPLAT_DATABASE", "Fred"),
    ];
    let run = bugsplat_with(server.uri(), &env, &["versions", "list"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let call = &requests(&server, "tools/call").await[0];
    assert_eq!(
        body(call)["params"]["arguments"],
        json!({"database": "Fred"})
    );
}

#[tokio::test]
async fn defects_create_and_link_need_yes_without_a_terminal() {
    for args in [
        &["defects", "create", "7", "-b", "Fred"][..],
        &["defects", "link", "7", "PROJ-123"][..],
    ] {
        let server = server().await;
        let run = bugsplat(&server, args).await;
        assert_eq!(run.code, 2, "{args:?}: {}", run.stderr);
        assert!(run.stderr.contains("crash group 7"), "{}", run.stderr);
        assert!(run.stderr.contains("--yes"), "{}", run.stderr);
        assert!(run.stdout.is_empty());
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn tool_call_types_pairs_by_the_tools_schema() {
    assert_call(
        &[
            "-b",
            "Fred",
            "tool",
            "call",
            "list-reports",
            "--args",
            r#"{"pageSize":5,"startDate":"2026-01-01"}"#,
            "version=1.0",
            "page=2",
            "application=[\"MyGame\",\"MyTool\"]",
            "startDate=2026-11-01",
        ],
        "list-reports",
        json!({
            "database": "Fred",
            "pageSize": 5,
            "version": "1.0",
            "page": 2,
            "application": ["MyGame", "MyTool"],
            "startDate": "2026-11-01",
        }),
    )
    .await;
}

#[tokio::test]
async fn tool_call_sends_the_database_only_to_tools_that_take_one() {
    assert_call(
        &["-b", "Fred", "tool", "call", "list-databases"],
        "list-databases",
        json!({}),
    )
    .await;
    assert_call(
        &[
            "-b",
            "Fred",
            "tool",
            "call",
            "get-report",
            "id=42",
            "database=Other",
        ],
        "get-report",
        json!({"id": 42, "database": "Other"}),
    )
    .await;
}

#[tokio::test]
async fn tool_call_reaches_a_tool_the_cli_does_not_know() {
    assert_call(
        &[
            "-b",
            "Fred",
            "tool",
            "call",
            "brand-new-tool",
            "count=3",
            "label=new",
        ],
        "brand-new-tool",
        json!({"count": 3, "label": "new", "database": "Fred"}),
    )
    .await;
}

#[tokio::test]
async fn tool_list_prints_the_tools() {
    let server = server().await;
    let run = bugsplat(&server, &["tool", "list"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(
        run.stdout
            .contains("get-report\n  Get one crash report in full.\n")
    );
    assert!(
        run.stdout
            .contains("    id (integer, required): Crash report id")
    );
    assert!(
        run.stdout
            .contains("    application (string | array | object)")
    );

    let run = bugsplat(&server, &["tool", "list", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let printed: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(printed, tools());
}

#[tokio::test]
async fn text_output_is_the_tools_text() {
    let server = server_with(|request| {
        text_result(
            request,
            "Group #7 in database Fred\n\nShowing results 1-1 (page 1).",
        )
    })
    .await;
    let run = bugsplat(&server, &["groups", "list"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        run.stdout,
        "Group #7 in database Fred\n\nShowing results 1-1 (page 1).\n"
    );
    assert!(run.stderr.is_empty(), "{}", run.stderr);
}

#[tokio::test]
async fn json_output_wraps_text_when_there_is_no_structured_content() {
    let server = server_with(|request| text_result(request, "Group #7\nCrashes: 3")).await;
    let run = bugsplat(&server, &["groups", "list", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let printed: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(printed, json!({"text": "Group #7\nCrashes: 3"}));
}

#[tokio::test]
async fn json_output_is_the_structured_content_when_the_server_sends_it() {
    let server = server_with(|request| {
        result(
            request,
            json!({
                "content": [{"type": "text", "text": "Group #7"}],
                "structuredContent": {"groups": [{"id": 7, "crashes": 3}]},
            }),
        )
    })
    .await;
    let run = bugsplat(&server, &["--json", "groups", "list"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let printed: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(printed, json!({"groups": [{"id": 7, "crashes": 3}]}));

    let run = bugsplat(&server, &["groups", "list"]).await;
    assert_eq!(run.stdout, "Group #7\n");
}

#[tokio::test]
async fn an_sse_response_is_read() {
    let server = server_with(|request| {
        let response = json!({
            "jsonrpc": "2.0",
            "id": body(request)["id"],
            "result": {"content": [{"type": "text", "text": "streamed"}]},
        });
        let progress = json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {}});
        ResponseTemplate::new(200).set_body_raw(
            format!("event: message\ndata: {progress}\n\nevent: message\ndata: {response}\n\n"),
            "text/event-stream",
        )
    })
    .await;
    let run = bugsplat(&server, &["reports", "get", "42"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout, "streamed\n");
}

#[tokio::test]
async fn a_tool_error_fails_with_the_servers_message() {
    let server = server_with(|request| {
        result(
            request,
            json!({
                "content": [{"type": "text", "text": "Crash #42 was not found in database Fred"}],
                "isError": true,
            }),
        )
    })
    .await;
    for args in [
        &["reports", "get", "42"][..],
        &["reports", "get", "42", "--json"][..],
    ] {
        let run = bugsplat(&server, args).await;
        assert_eq!(run.code, 1);
        assert_eq!(
            run.stderr,
            "error: Crash #42 was not found in database Fred\n"
        );
        assert!(run.stdout.is_empty());
    }
}

#[tokio::test]
async fn a_json_rpc_error_fails_with_the_servers_message() {
    let server = server_with(|request| {
        ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": body(request)["id"],
            "error": {"code": -32602, "message": "Tool not found: \"set-group-status\"."},
        }))
    })
    .await;
    let run = bugsplat(&server, &["groups", "set-status", "7", "fixed"]).await;
    assert_eq!(run.code, 1);
    assert_eq!(run.stderr, "error: Tool not found: \"set-group-status\".\n");
}

#[tokio::test]
async fn a_rejected_token_is_not_signed_in() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"message": "No token"})))
        .mount(&server)
        .await;
    let run = bugsplat(&server, &["databases", "list"]).await;
    assert_eq!(run.code, 3);
    assert!(run.stderr.contains("Not signed in"), "{}", run.stderr);
    assert!(run.stdout.is_empty());
}

#[tokio::test]
async fn no_token_is_not_signed_in_without_calling_the_server() {
    let server = server().await;
    let run = bugsplat_with(server.uri(), &[], &["databases", "list"]).await;
    assert_eq!(run.code, 3);
    assert!(run.stderr.contains("Not signed in"), "{}", run.stderr);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_command_sends_the_token_and_session_and_then_ends_the_session() {
    let server = server().await;
    let run = bugsplat(&server, &["databases", "list"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);

    let received = server.received_requests().await.unwrap();
    let header = |request: &Request, name: &str| {
        request
            .headers
            .get(name)
            .map(|value| value.to_str().unwrap().to_string())
    };
    let methods: Vec<String> = received
        .iter()
        .map(|request| match request.method.as_str() {
            "POST" => body(request)["method"].as_str().unwrap().to_string(),
            other => other.to_string(),
        })
        .collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "notifications/initialized",
            "tools/call",
            "DELETE"
        ]
    );
    for request in &received {
        assert_eq!(
            header(request, "authorization"),
            Some(format!("Bearer {TOKEN}"))
        );
    }
    for request in &received[1..] {
        assert_eq!(header(request, "mcp-session-id").as_deref(), Some(SESSION));
    }
    for request in &received[1..3] {
        assert_eq!(
            header(request, "mcp-protocol-version").as_deref(),
            Some("2025-06-18")
        );
        assert_eq!(
            header(request, "accept").as_deref(),
            Some("application/json, text/event-stream")
        );
    }
}

#[tokio::test]
async fn verbose_shows_the_call_but_never_the_token() {
    let server = server().await;
    let run = bugsplat(
        &server,
        &["--verbose", "reports", "get", "42", "-b", "Fred"],
    )
    .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.stderr.contains("Calling get-report"), "{}", run.stderr);
    assert!(run.stderr.contains(r#""id":42"#), "{}", run.stderr);
    assert!(!run.stderr.contains(TOKEN));
    assert!(!run.stdout.contains(TOKEN));
}

#[tokio::test]
async fn bad_arguments_are_usage_errors_that_call_nothing() {
    let cases: [&[&str]; 8] = [
        &["groups", "list", "--version", r#"{"in":["1.0",}"#],
        &["groups", "list", "--users", "many"],
        &["groups", "list", "--linked-defect", "maybe"],
        &["groups", "list", "--sort-order", "sideways"],
        &["groups", "set-status", "7", "closed"],
        &["reports", "list", "--attribute", "GPU"],
        &["tool", "call", "list-reports", "pageSize"],
        &["tool", "call", "list-reports", "--args", "[1,2]"],
    ];
    for args in cases {
        let server = server().await;
        let run = bugsplat(&server, args).await;
        assert_eq!(run.code, 2, "{args:?}: {}", run.stderr);
        assert!(run.stdout.is_empty(), "{args:?}");
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "{args:?} reached the server"
        );
    }
}

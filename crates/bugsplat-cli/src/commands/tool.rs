//! `tool`, plus what every remote verb shares: connecting to the MCP server, building a
//! tool's arguments from flags, and printing its result.
//! https://github.com/BugSplat-Git/bugsplat-cli/issues/4

use std::io::{ErrorKind, Write};

use bugsplat_core::Error;
use bugsplat_mcp_client::{McpClient, Tool, ToolResult};
use clap::{Args, Subcommand};
use serde_json::{Map, Value, json};

use crate::context::Context;
use crate::output::print_json;

pub(crate) const FILTER_HELP: &str = "\
Filters:
  A filter takes plain text, a JSON array of values, or a JSON object of operators:
    --version '1.2.*'                    pattern: exact, unless * starts or ends the value
    --version '[\"1.0\",\"1.1\"]'            any of the values
    --version '{\"ne\":\"1.0\"}'             every operator must hold
  Text operators: eq, ne, in, notIn, contains, notContains, startsWith, endsWith, exists.
  Integer operators: eq, ne, in, notIn, gt, gte, lt, lte.
  https://docs.bugsplat.com/api-reference/mcp-server/tools#filters";

#[derive(Debug, Subcommand)]
pub enum ToolCommand {
    /// List the MCP server's tools and their arguments
    List(ListArgs),
    /// Call a tool by name
    Call(CallArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {}

#[derive(Debug, Args)]
pub struct CallArgs {
    /// Tool name, for example list-groups
    pub name: String,
    /// The tool's arguments as a JSON object
    #[arg(long, value_name = "JSON")]
    pub args: Option<String>,
    /// Arguments as key=value pairs, added to --args. A value is sent as the JSON type the
    /// tool accepts for that argument: version=1.0 stays text, pageSize=5 is a number,
    /// status='["open","fixed"]' is an array
    #[arg(value_name = "KEY=VALUE")]
    pub pairs: Vec<String>,
}

pub async fn run(command: ToolCommand, ctx: &Context) -> anyhow::Result<()> {
    match command {
        ToolCommand::List(_) => list(ctx).await,
        ToolCommand::Call(args) => call_by_name(args, ctx).await,
    }
}

async fn list(ctx: &Context) -> anyhow::Result<()> {
    let mut client = connect(ctx).await?;
    let tools = client.list_tools().await;
    client.close().await;
    let tools = tools?;
    if ctx.config.is_json() {
        return print_json(&tools);
    }
    let text = tools.iter().map(describe).collect::<Vec<_>>().join("\n\n");
    print_text(&text)
}

fn describe(tool: &Tool) -> String {
    let mut lines = vec![tool.name.clone()];
    if let Some(description) = &tool.description {
        lines.push(format!("  {description}"));
    }
    let required: Vec<&str> = tool
        .input_schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let properties = tool
        .input_schema
        .get("properties")
        .and_then(Value::as_object);
    for (name, schema) in properties.into_iter().flatten() {
        let mut kind = schema_types(schema).join(" | ");
        if required.contains(&name.as_str()) {
            kind.push_str(", required");
        }
        lines.push(match schema.get("description").and_then(Value::as_str) {
            Some(description) => format!("    {name} ({kind}): {description}"),
            None => format!("    {name} ({kind})"),
        });
    }
    lines.join("\n")
}

async fn call_by_name(args: CallArgs, ctx: &Context) -> anyhow::Result<()> {
    let mut arguments = match &args.args {
        Some(raw) => parse_object(raw)?,
        None => Map::new(),
    };
    let pairs = args
        .pairs
        .iter()
        .map(|pair| split_pair(pair))
        .collect::<bugsplat_core::Result<Vec<_>>>()?;

    let mut client = connect(ctx).await?;
    // The schema decides how key=value text is typed and whether the tool takes a database.
    // A tool the server doesn't list is still called, so the server gets to say so.
    let schema = match client.list_tools().await {
        Ok(tools) => tools
            .into_iter()
            .find(|tool| tool.name == args.name)
            .map(|tool| tool.input_schema),
        Err(error) => {
            client.close().await;
            return Err(error.into());
        }
    };
    let property = |name: &str| {
        schema
            .as_ref()
            .and_then(|schema| schema.pointer(&format!("/properties/{name}")))
    };
    for (name, raw) in pairs {
        let value = pair_value(raw, property(name));
        arguments.insert(name.to_string(), value);
    }
    if let Some(database) = &ctx.config.database
        && !arguments.contains_key("database")
        && (schema.is_none() || property("database").is_some())
    {
        arguments.insert("database".to_string(), json!(database));
    }
    if ctx.config.is_json() && schema.as_ref().is_some_and(takes_json_format) {
        arguments
            .entry("format".to_string())
            .or_insert_with(|| json!("json"));
    }
    call_with(ctx, client, &args.name, arguments).await
}

fn parse_object(raw: &str) -> bugsplat_core::Result<Map<String, Value>> {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(Error::Usage(
            "--args must be a JSON object, for example '{\"pageSize\":5}'.".to_string(),
        )),
        Err(error) => Err(Error::Usage(format!("--args is not valid JSON: {error}"))),
    }
}

fn split_pair(pair: &str) -> bugsplat_core::Result<(&str, &str)> {
    match pair.split_once('=') {
        Some((name, value)) if !name.is_empty() => Ok((name, value)),
        _ => Err(Error::Usage(format!(
            "`{pair}` is not a key=value pair, for example pageSize=5."
        ))),
    }
}

/// `raw` as JSON when it parses to a type `schema` accepts, otherwise as text. Without a
/// schema anything that parses is sent as JSON.
fn pair_value(raw: &str, schema: Option<&Value>) -> Value {
    let Ok(parsed) = serde_json::from_str::<Value>(raw) else {
        return Value::String(raw.to_string());
    };
    let accepted = schema.map(schema_types).unwrap_or_default();
    if accepted.is_empty() || accepted.iter().any(|kind| is_type(&parsed, kind)) {
        parsed
    } else {
        Value::String(raw.to_string())
    }
}

/// The JSON types a property's schema accepts, through `type`, `oneOf` and `anyOf`.
fn schema_types(schema: &Value) -> Vec<String> {
    let mut types: Vec<String> = match schema.get("type") {
        Some(Value::String(kind)) => vec![kind.clone()],
        Some(Value::Array(kinds)) => kinds
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };
    let branches = ["oneOf", "anyOf"]
        .into_iter()
        .filter_map(|key| schema.get(key).and_then(Value::as_array))
        .flatten();
    for kind in branches.flat_map(schema_types) {
        if !types.contains(&kind) {
            types.push(kind);
        }
    }
    types
}

fn is_type(value: &Value, kind: &str) -> bool {
    match kind {
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => false,
    }
}

/// A tool's argument object, built from a command's flags.
#[derive(Debug, Default)]
pub(crate) struct ToolArguments(Map<String, Value>);

impl ToolArguments {
    /// Starts with the global `--database`, when one was given; without it the server
    /// uses the signed-in user's default database.
    pub(crate) fn for_database(ctx: &Context) -> Self {
        Self::default().opt("database", ctx.config.database.clone())
    }

    pub(crate) fn set(mut self, name: &str, value: impl Into<Value>) -> Self {
        self.0.insert(name.to_string(), value.into());
        self
    }

    pub(crate) fn opt<T: Into<Value>>(self, name: &str, value: Option<T>) -> Self {
        match value {
            Some(value) => self.set(name, value),
            None => self,
        }
    }

    /// A flag that is only sent when given, so the server's default applies otherwise.
    pub(crate) fn flag(self, name: &str, given: bool) -> Self {
        self.opt(name, given.then_some(true))
    }

    pub(crate) fn text_filter(
        self,
        name: &str,
        raw: Option<&String>,
    ) -> bugsplat_core::Result<Self> {
        let value = raw.map(|raw| text_filter(name, raw)).transpose()?;
        Ok(self.opt(name, value))
    }

    pub(crate) fn integer_filter(
        self,
        name: &str,
        raw: Option<&String>,
    ) -> bugsplat_core::Result<Self> {
        let value = raw.map(|raw| integer_filter(name, raw)).transpose()?;
        Ok(self.opt(name, value))
    }

    /// `NAME=FILTER` pairs as the `attributes` object of `list-reports`.
    pub(crate) fn attributes(self, pairs: &[String]) -> bugsplat_core::Result<Self> {
        if pairs.is_empty() {
            return Ok(self);
        }
        let mut attributes = Map::new();
        for pair in pairs {
            let (name, raw) = pair
                .split_once('=')
                .filter(|(name, _)| !name.is_empty())
                .ok_or_else(|| {
                    Error::Usage(format!(
                        "--attribute takes NAME=FILTER, for example GPU='GeForce*'; got `{pair}`."
                    ))
                })?;
            attributes.insert(name.to_string(), text_filter("attribute", raw)?);
        }
        Ok(self.set("attributes", attributes))
    }

    pub(crate) fn into_map(self) -> Map<String, Value> {
        self.0
    }
}

/// Plain text stays text; a value that starts like a JSON array or object must be one.
/// Falling back to text there would turn a typo in an operator object into a filter
/// that silently matches nothing.
fn text_filter(name: &str, raw: &str) -> bugsplat_core::Result<Value> {
    if !looks_like_json(raw) {
        return Ok(Value::String(raw.to_string()));
    }
    serde_json::from_str(raw).map_err(|error| {
        Error::Usage(format!(
            "--{}: `{raw}` is not valid JSON ({error}). To match text that starts with `{}`, use {{\"eq\":\"...\"}}.",
            flag_name(name),
            raw.trim_start().chars().next().unwrap_or('{'),
        ))
    })
}

fn integer_filter(name: &str, raw: &str) -> bugsplat_core::Result<Value> {
    if let Ok(number) = raw.trim().parse::<i64>() {
        return Ok(json!(number));
    }
    if looks_like_json(raw)
        && let Ok(value) = serde_json::from_str(raw)
    {
        return Ok(value);
    }
    Err(Error::Usage(format!(
        "--{} takes a whole number or a JSON filter such as '{{\"gte\":10}}'; got `{raw}`.",
        flag_name(name)
    )))
}

fn looks_like_json(raw: &str) -> bool {
    raw.trim_start().starts_with(['{', '['])
}

/// `stackKeyId` is the tool's argument; `--stack-key-id` is what the user typed.
fn flag_name(argument: &str) -> String {
    let mut flag = String::new();
    for character in argument.chars() {
        if character.is_ascii_uppercase() {
            flag.push('-');
        }
        flag.push(character.to_ascii_lowercase());
    }
    flag
}

pub(crate) async fn connect(ctx: &Context) -> anyhow::Result<McpClient> {
    let token = bugsplat_auth::access_token(&ctx.config).await?;
    Ok(McpClient::connect(ctx.config.mcp_url(), &token.secret).await?)
}

/// Calls `tool` and prints its result: the tool's text, or with `--json` the JSON the tool
/// returns when asked for `format: "json"`. A tool without that argument gets its
/// structured content printed, or `{"text": ...}` when the server sends none.
pub(crate) async fn call(
    ctx: &Context,
    tool: &str,
    arguments: ToolArguments,
) -> anyhow::Result<()> {
    let mut client = connect(ctx).await?;
    let mut arguments = arguments.into_map();
    if ctx.config.is_json() {
        let tools = match client.list_tools().await {
            Ok(tools) => tools,
            Err(error) => {
                client.close().await;
                return Err(error.into());
            }
        };
        if tools
            .iter()
            .any(|listed| listed.name == tool && takes_json_format(&listed.input_schema))
        {
            arguments.insert("format".to_string(), json!("json"));
        }
    }
    call_with(ctx, client, tool, arguments).await
}

/// BugSplat's tools describe `format` without an enum, so a bad value is the tool's own
/// error; an enum that leaves out `json` still rules it out.
fn takes_json_format(schema: &Value) -> bool {
    let Some(format) = schema.pointer("/properties/format") else {
        return false;
    };
    match format.get("enum").and_then(Value::as_array) {
        Some(formats) => formats.iter().any(|format| format == "json"),
        None => true,
    }
}

async fn call_with(
    ctx: &Context,
    mut client: McpClient,
    tool: &str,
    arguments: Map<String, Value>,
) -> anyhow::Result<()> {
    let asked_for_json = arguments
        .get("format")
        .is_some_and(|format| format == "json");
    let arguments = Value::Object(arguments);
    if ctx.config.verbose {
        eprintln!(
            "Calling {tool} at {} with {arguments}",
            ctx.config.mcp_url()
        );
    }
    let result = client.call_tool(tool, arguments).await;
    client.close().await;
    print_result(ctx, result?, asked_for_json)
}

fn print_result(ctx: &Context, result: ToolResult, asked_for_json: bool) -> anyhow::Result<()> {
    if result.is_error {
        let message = if result.text.is_empty() {
            "The tool failed without a message.".to_string()
        } else {
            result.text
        };
        return Err(Error::api(None, message).into());
    }
    if ctx.config.is_json() {
        return print_json(&json_output(result, asked_for_json));
    }
    print_text(&result.text)
}

fn json_output(result: ToolResult, asked_for_json: bool) -> Value {
    if let Some(structured) = result.structured {
        return structured;
    }
    if asked_for_json && let Ok(document) = serde_json::from_str::<Value>(&result.text) {
        return document;
    }
    json!({"text": result.text})
}

fn print_text(text: &str) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    let newline = if text.ends_with('\n') { "" } else { "\n" };
    match write!(stdout, "{text}{newline}").and_then(|()| stdout.flush()) {
        // `bugsplat reports list | head` closes the pipe early; that is not a failure.
        Err(error) if error.kind() == ErrorKind::BrokenPipe => Ok(()),
        other => Ok(other?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_text_filter_stays_text() {
        assert_eq!(text_filter("version", "1.2.*").unwrap(), json!("1.2.*"));
        assert_eq!(text_filter("version", "10").unwrap(), json!("10"));
        assert_eq!(text_filter("version", "true").unwrap(), json!("true"));
    }

    #[test]
    fn a_json_text_filter_is_sent_as_json() {
        assert_eq!(
            text_filter("version", r#"{"in":["1.0","1.1"]}"#).unwrap(),
            json!({"in": ["1.0", "1.1"]})
        );
        assert_eq!(
            text_filter("status", r#" ["open","regression"]"#).unwrap(),
            json!(["open", "regression"])
        );
    }

    #[test]
    fn a_broken_json_filter_is_a_usage_error_naming_the_flag() {
        let error = text_filter("stackGroup", r#"{"in":["1.0",}"#).unwrap_err();
        assert_eq!(error.exit_code(), 2);
        assert!(error.to_string().starts_with("--stack-group:"), "{error}");
    }

    #[test]
    fn an_integer_filter_is_a_number_or_json() {
        assert_eq!(integer_filter("users", "5").unwrap(), json!(5));
        assert_eq!(integer_filter("stackKeyId", "-1").unwrap(), json!(-1));
        assert_eq!(
            integer_filter("users", r#"{"gte":5,"lt":100}"#).unwrap(),
            json!({"gte": 5, "lt": 100})
        );
        assert_eq!(integer_filter("users", "[1,2]").unwrap(), json!([1, 2]));
    }

    #[test]
    fn an_integer_filter_rejects_text() {
        for raw in ["many", "1.5", r#"{"gte":"#] {
            let error = integer_filter("stackKeyId", raw).unwrap_err();
            assert_eq!(error.exit_code(), 2);
            assert!(error.to_string().starts_with("--stack-key-id "), "{error}");
        }
    }

    #[test]
    fn attributes_become_an_object_of_filters() {
        let arguments = ToolArguments::default()
            .attributes(&[
                "GPU={\"startsWith\":\"GeForce\"}".to_string(),
                "build type=a=b".to_string(),
            ])
            .unwrap()
            .into_map();
        assert_eq!(
            Value::Object(arguments),
            json!({"attributes": {"GPU": {"startsWith": "GeForce"}, "build type": "a=b"}})
        );
    }

    #[test]
    fn an_attribute_without_a_name_is_a_usage_error() {
        for pair in ["GPU", "=GeForce"] {
            let error = ToolArguments::default()
                .attributes(&[pair.to_string()])
                .unwrap_err();
            assert_eq!(error.exit_code(), 2);
        }
    }

    #[test]
    fn no_attributes_sends_no_argument() {
        let arguments = ToolArguments::default().attributes(&[]).unwrap();
        assert!(arguments.into_map().is_empty());
    }

    #[test]
    fn optional_arguments_are_left_out() {
        let arguments = ToolArguments::default()
            .opt("page", None::<u32>)
            .opt("pageSize", Some(5))
            .flag("includeRetired", false)
            .flag("linkedDefect", true)
            .into_map();
        assert_eq!(
            Value::Object(arguments),
            json!({"pageSize": 5, "linkedDefect": true})
        );
    }

    #[test]
    fn pair_values_follow_the_schema() {
        let text = json!({"type": "string"});
        let integer = json!({"type": "integer"});
        let filter = json!({"oneOf": [
            {"type": "string"},
            {"type": "array", "items": {"type": "string"}},
            {"type": "object"},
        ]});
        assert_eq!(pair_value("1.0", Some(&text)), json!("1.0"));
        assert_eq!(pair_value("true", Some(&text)), json!("true"));
        assert_eq!(pair_value("\"quoted\"", Some(&text)), json!("quoted"));
        assert_eq!(pair_value("5", Some(&integer)), json!(5));
        assert_eq!(pair_value("five", Some(&integer)), json!("five"));
        assert_eq!(pair_value("1.0", Some(&filter)), json!("1.0"));
        assert_eq!(
            pair_value(r#"{"ne":"1.0"}"#, Some(&filter)),
            json!({"ne": "1.0"})
        );
        assert_eq!(pair_value(r#"["a"]"#, Some(&filter)), json!(["a"]));
    }

    #[test]
    fn pair_values_without_a_schema_are_json_when_they_parse() {
        assert_eq!(pair_value("5", None), json!(5));
        assert_eq!(pair_value("true", None), json!(true));
        assert_eq!(pair_value("MyGame", None), json!("MyGame"));
        assert_eq!(pair_value("", None), json!(""));
    }

    #[test]
    fn pairs_split_on_the_first_equals_sign() {
        assert_eq!(split_pair("a=b=c").unwrap(), ("a", "b=c"));
        assert_eq!(split_pair("a=").unwrap(), ("a", ""));
        assert_eq!(split_pair("a").unwrap_err().exit_code(), 2);
        assert_eq!(split_pair("=b").unwrap_err().exit_code(), 2);
    }

    #[test]
    fn args_must_be_a_json_object() {
        assert_eq!(parse_object(r#"{"a":1}"#).unwrap()["a"], json!(1));
        assert_eq!(parse_object("[1]").unwrap_err().exit_code(), 2);
        assert_eq!(parse_object("{").unwrap_err().exit_code(), 2);
    }

    #[test]
    fn schema_types_reads_unions() {
        assert_eq!(schema_types(&json!({"type": "string"})), ["string"]);
        assert_eq!(
            schema_types(&json!({"type": ["string", "null"]})),
            ["string", "null"]
        );
        assert_eq!(
            schema_types(
                &json!({"anyOf": [{"type": "integer"}, {"type": "integer"}, {"type": "object"}]})
            ),
            ["integer", "object"]
        );
        assert!(schema_types(&json!({})).is_empty());
    }

    #[test]
    fn json_output_prefers_structured_content() {
        let text_only = ToolResult {
            text: "2 groups".into(),
            structured: None,
            is_error: false,
            raw: Value::Null,
        };
        assert_eq!(json_output(text_only, false), json!({"text": "2 groups"}));
        let structured = ToolResult {
            text: "2 groups".into(),
            structured: Some(json!({"groups": []})),
            is_error: false,
            raw: Value::Null,
        };
        assert_eq!(json_output(structured, true), json!({"groups": []}));
    }

    #[test]
    fn json_output_is_the_tools_json_document_when_it_was_asked_for() {
        let result = |text: &str| ToolResult {
            text: text.into(),
            structured: None,
            is_error: false,
            raw: Value::Null,
        };
        assert_eq!(
            json_output(result(r#"{"groups": [{"stackKeyId": 7}]}"#), true),
            json!({"groups": [{"stackKeyId": 7}]})
        );
        assert_eq!(
            json_output(result(r#"{"groups": []}"#), false),
            json!({"text": r#"{"groups": []}"#})
        );
        assert_eq!(
            json_output(result("Group #7"), true),
            json!({"text": "Group #7"})
        );
    }

    #[test]
    fn a_tool_takes_the_json_format_when_its_schema_lists_it() {
        assert!(takes_json_format(&json!({
            "properties": {"format": {"type": "string", "enum": ["text", "json"]}}
        })));
        assert!(!takes_json_format(&json!({
            "properties": {"format": {"type": "string", "enum": ["text"]}}
        })));
        assert!(takes_json_format(&json!({
            "properties": {"format": {"type": "string", "description": "text (default) or json."}}
        })));
        assert!(!takes_json_format(&json!({"properties": {"database": {}}})));
    }

    #[test]
    fn describe_lists_a_tools_arguments() {
        let tool = Tool {
            name: "get-report".into(),
            title: None,
            description: Some("Get one crash report".into()),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {"type": "integer", "description": "Crash report id"},
                    "database": {"type": "string"},
                },
                "required": ["id"],
            }),
        };
        let text = describe(&tool);
        assert_eq!(
            text,
            "get-report\n  Get one crash report\n    database (string)\n    id (integer, required): Crash report id"
        );
    }

    #[test]
    fn flag_names_are_kebab_case() {
        assert_eq!(flag_name("stackKeyId"), "stack-key-id");
        assert_eq!(flag_name("version"), "version");
    }
}

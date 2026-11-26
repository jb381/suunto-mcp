mod http;
mod sessions;

pub use http::router;

use crate::{App, cloud, local, stores};
use anyhow::{Result, anyhow, bail, ensure};
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
};
use tokio::io::{self, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_TOOL_TEXT_BYTES: usize = 8 * 1024 * 1024;
const PROTOCOLS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

fn embedded(source: &str) -> Value {
    serde_json::from_str(source).expect("bundled MCP metadata is valid JSON")
}

static TOOLS: LazyLock<Value> = LazyLock::new(|| embedded(include_str!("../protocol/tools.json")));
static RESOURCES: LazyLock<Value> =
    LazyLock::new(|| embedded(include_str!("../protocol/resources.json")));
static CATALOG: LazyLock<Value> =
    LazyLock::new(|| embedded(include_str!("../protocol/api-catalog.json")));
static COVERAGE: LazyLock<Value> =
    LazyLock::new(|| embedded(include_str!("../protocol/coverage.json")));
const INSTRUCTIONS: &str = include_str!("../protocol/instructions.md");

#[derive(Clone, Copy)]
enum Handler {
    Local,
    Store,
    Cloud,
}

static HANDLERS: LazyLock<HashMap<&'static str, Handler>> = LazyLock::new(|| {
    CATALOG
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(group, names)| {
            let handler = match group.as_str() {
                "imports" => Handler::Local,
                "oauth" | "webhooks" | "safety" => Handler::Store,
                "workouts" | "activity_247" | "routes" | "uploads" | "guides" => Handler::Cloud,
                _ => return None,
            };
            Some(
                names
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(move |name| (name.as_str().unwrap(), handler)),
            )
        })
        .flatten()
        .collect()
});

fn rpc_error(id: &Value, code: i32, message: impl Into<String>) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message.into()}})
}

fn rpc_result(id: &Value, result: Value) -> Value {
    let mut response = json!({"jsonrpc":"2.0","id":id});
    response["result"] = result;
    response
}

fn valid_type(schema: &Value, value: &Value) -> bool {
    if let Some(choices) = schema["anyOf"].as_array() {
        return choices.iter().any(|choice| valid_type(choice, value));
    }
    if let Some(choices) = schema["enum"].as_array()
        && !choices.contains(value)
    {
        return false;
    }
    match schema["type"].as_str() {
        Some("object") => value.as_object().is_some_and(|fields| {
            fields
                .values()
                .all(|field| valid_type(&schema["additionalProperties"], field))
        }),
        Some("array") => value
            .as_array()
            .is_some_and(|items| items.iter().all(|item| valid_type(&schema["items"], item))),
        Some("number") => value.is_number(),
        Some("integer") => value.as_i64().is_some(),
        Some("string") => value.is_string(),
        Some("null") => value.is_null(),
        Some("boolean") => value.is_boolean(),
        _ => true,
    }
}

fn validate_arguments(schema: &Value, supplied: &Value) -> Result<Value> {
    let mut supplied = match supplied {
        Value::Null => Map::new(),
        Value::Object(map) => map.clone(),
        _ => bail!("Tool arguments must be an object"),
    };
    if let Some(required) = schema["required"].as_array() {
        for name in required {
            let name = name.as_str().unwrap();
            ensure!(
                supplied.contains_key(name),
                "Missing required argument {name}"
            );
        }
    }
    for (name, value) in &supplied {
        let definition = &schema["properties"][name];
        ensure!(!definition.is_null(), "Unknown argument {name}");
        ensure!(
            valid_type(definition, value),
            "Invalid value for argument {name}"
        );
    }
    if let Some(properties) = schema["properties"].as_object() {
        for (name, definition) in properties {
            if let Some(default) = definition.get("default") {
                supplied
                    .entry(name.clone())
                    .or_insert_with(|| default.clone());
            }
        }
    }
    Ok(Value::Object(supplied))
}

fn tool_error(error: impl std::fmt::Display) -> Value {
    json!({"content":[{"type":"text","text":error.to_string()}],"isError":true})
}

struct BoundedWriter(Vec<u8>);

impl std::io::Write for BoundedWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(data.len()) > MAX_TOOL_TEXT_BYTES {
            return Err(std::io::Error::other(
                "Tool response exceeds the inline response limit",
            ));
        }
        self.0.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn tool_result(value: Value) -> Result<Value> {
    let mut writer = BoundedWriter(Vec::new());
    serde_json::to_writer(&mut writer, &value)?;
    let text = String::from_utf8(writer.0)?;
    let content = if value.is_null() {
        Vec::new()
    } else {
        let mut item = json!({"type":"text"});
        item["text"] = Value::String(text);
        vec![item]
    };
    let mut response = json!({"isError":false});
    response["content"] = Value::Array(content);
    if value.is_object() {
        response["structuredContent"] = value;
    } else {
        response["_meta"] = json!({"fastmcp":{"wrap_result":true}});
        response["structuredContent"] = Value::Object(Map::from_iter([("result".into(), value)]));
    }
    Ok(response)
}

async fn run_tool(app: &Arc<App>, params: &Value) -> Result<Value> {
    let name = params["name"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing tool name"))?;
    let (registered, handler) = HANDLERS
        .get_key_value(name)
        .ok_or_else(|| anyhow!("Unknown tool: {name}"))?;
    let schema = TOOLS
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == name)
        .ok_or_else(|| anyhow!("Missing schema for {name}"))?;
    let args = validate_arguments(&schema["inputSchema"], &params["arguments"])?;
    let name = *registered;
    let result = match handler {
        Handler::Local => {
            let app = Arc::clone(app);
            tokio::task::spawn_blocking(move || local::call(&app, name, &args)).await??
        }
        Handler::Store => stores::call(app, name, &args).await?,
        Handler::Cloud => cloud::call(app, name, &args).await?,
    };
    tool_result(result)
}

async fn resource(app: &Arc<App>, uri: &str) -> Result<Option<Value>> {
    Ok(Some(match uri {
        "suunto://config" => app.config.redacted().clone(),
        "suunto://api-catalog" => CATALOG.clone(),
        "suunto://coverage" => COVERAGE.clone(),
        "suunto://quota" => {
            let app = Arc::clone(app);
            tokio::task::spawn_blocking(move || app.quota.status()).await??
        }
        _ => return Ok(None),
    }))
}

fn method(message: &Value) -> Option<&str> {
    if message["jsonrpc"] == "2.0" {
        message["method"].as_str()
    } else {
        None
    }
}

fn is_initialize(message: &Value) -> bool {
    method(message) == Some("initialize") && valid_id(&message["id"])
}

fn valid_id(id: &Value) -> bool {
    id.is_string() || id.as_i64().is_some() || id.as_u64().is_some()
}

fn negotiated_version(params: &Value) -> &'static str {
    PROTOCOLS
        .iter()
        .copied()
        .find(|version| params["protocolVersion"] == *version)
        .unwrap_or(PROTOCOLS[0])
}

async fn dispatch(app: &Arc<App>, message: Value) -> Option<Value> {
    let id = &message["id"];
    let Some(method) = method(&message) else {
        // Client responses can arrive while using HTTP/SSE. This server issues no requests.
        if message["jsonrpc"] == "2.0"
            && valid_id(id)
            && (message.get("result").is_some() || message.get("error").is_some())
        {
            return None;
        }
        return Some(rpc_error(&Value::Null, -32600, "Invalid Request"));
    };
    message.get("id")?;
    if !valid_id(id) {
        return Some(rpc_error(&Value::Null, -32600, "Invalid request id"));
    }
    let params = &message["params"];
    if !params.is_null() && !params.is_object() {
        return Some(rpc_error(id, -32602, "Expected params object"));
    }
    let result = match method {
        "initialize" => {
            if !params["protocolVersion"].is_string() {
                return Some(rpc_error(id, -32602, "Missing protocolVersion"));
            }
            json!({
                "protocolVersion":negotiated_version(params),
                "capabilities":{"tools":{},"resources":{}},
                "serverInfo":{"name":"Suunto MCP","version":env!("CARGO_PKG_VERSION")},
                "instructions":INSTRUCTIONS,
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools":*TOOLS}),
        "tools/call" => run_tool(app, params).await.unwrap_or_else(tool_error),
        "resources/list" => json!({"resources":*RESOURCES}),
        "resources/templates/list" => json!({"resourceTemplates":[]}),
        "prompts/list" => json!({"prompts":[]}),
        "resources/read" => {
            let Some(uri) = params["uri"].as_str() else {
                return Some(rpc_error(id, -32602, "Missing resource URI"));
            };
            match resource(app, uri).await {
                Ok(Some(value)) => {
                    json!({"contents":[{"uri":uri,"mimeType":"text/plain","text":value.to_string()}]})
                }
                Ok(None) => {
                    return Some(rpc_error(id, -32002, format!("Resource not found: {uri}")));
                }
                Err(error) => return Some(rpc_error(id, -32603, error.to_string())),
            }
        }
        _ => return Some(rpc_error(id, -32601, "Method not found")),
    };
    Some(rpc_result(id, result))
}

pub async fn stdio(app: Arc<App>) -> Result<()> {
    let mut stdin = BufReader::new(io::stdin());
    let mut stdout = io::stdout();
    let mut line = Vec::new();
    loop {
        line.clear();
        let size = (&mut stdin)
            .take(MAX_REQUEST_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
            .await?;
        if size == 0 {
            return Ok(());
        }
        ensure!(size <= MAX_REQUEST_BYTES, "MCP stdin message is too large");
        let reply = match serde_json::from_slice(&line) {
            Ok(message) => dispatch(&app, message).await,
            Err(_) => Some(rpc_error(&Value::Null, -32700, "Parse error")),
        };
        if let Some(reply) = reply {
            let mut bytes = serde_json::to_vec(&reply)?;
            bytes.push(b'\n');
            stdout.write_all(&bytes).await?;
            stdout.flush().await?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_published_tool_has_a_handler() {
        let tools = TOOLS.as_array().unwrap();
        assert_eq!(tools.len(), 60);
        assert_eq!(HANDLERS.len(), tools.len());
        for tool in tools {
            assert!(HANDLERS.contains_key(tool["name"].as_str().unwrap()));
        }
    }

    #[test]
    fn tool_schema_checks_types_and_applies_defaults() {
        let tools = TOOLS.as_array().unwrap();
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == "suunto_parse_gpx_file")
            .unwrap();
        let schema = &tool["inputSchema"];
        assert!(validate_arguments(schema, &json!({})).is_err());
        assert!(validate_arguments(schema, &json!({"path":false})).is_err());
        assert!(validate_arguments(schema, &json!({"path":"x","point_limit":1.2})).is_err());
        assert!(validate_arguments(schema, &json!({"path":"x","unknown":"x"})).is_err());
        let args = validate_arguments(schema, &json!({"path":"x"})).unwrap();
        assert_eq!(args["include_points"], true);
        assert_eq!(args["point_limit"], 5000);
    }
}

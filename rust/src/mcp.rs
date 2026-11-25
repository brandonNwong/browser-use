//! Stdio JSON-RPC 2.0 MCP server.
//!
//! Line-delimited JSON-RPC 2.0 over `tokio::io::stdin()` / `stdout()` handling
//! `initialize`, `notifications/initialized`, `ping`, `tools/list` and
//! `tools/call`. All diagnostics go strictly to stderr so stdout stays a pure
//! JSON-RPC channel.
//!
//! MCP mode never requires a live hub: `PeerClient` connects lazily on the
//! first tool call, so an offline app surfaces the actionable "Browser Use is
//! not running..." message inside the tool result instead of crashing.

use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

use crate::peer::PeerClient;
use crate::tools;
use crate::{SERVER_NAME, VERSION};

/// Default hub port, mirrored from `hub::DEFAULT_PORT`.
///
/// MCP mode resolves `ORANGE_PORT` locally instead of importing `hub` so the
/// module dependency DAG (mcp never imports hub) stays clean.
const DEFAULT_PORT: u16 = 19_455;

/// MCP protocol revisions the server can speak. When a client requests one of
/// these it is echoed back, otherwise the newest revision is announced (the
/// same negotiation the legacy TypeScript SDK performs).
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 5] = [
    "2025-11-25",
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
    "2024-10-07",
];

/// The exact initialize `instructions` string from the legacy `src/index.ts`.
pub const INSTRUCTIONS: &str =
    "Call browser_snapshot before interacting, then act with durable @eN refs \
using browser_click, browser_fill, or browser_press. Re-snapshot (or use delta=true) after actions \
that change the page. @eN refs reset after navigation; pass a CSS selector (for example \
selector=\"iframe\") to inspect a new page. Pass interactive: false to read page text, tables, or \
status values. browser_screenshot returns an annotated image plus its ref legend.";

/// Run the MCP stdio loop until stdin closes (or a signal arrives). Returns the
/// process exit code.
pub async fn run() -> i32 {
    let port = resolve_port();
    let secret = match crate::secret::load_secret().await {
        Ok(secret) => secret,
        Err(error) => {
            log(&format!("fatal: {error}"));
            return 1;
        }
    };

    let peer = PeerClient::new(port, secret);
    log(&format!("ready on stdio (peer mode at {})", peer.url()));

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            line = lines.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        handle_line(&line, &peer, &mut stdout).await;
                    }
                    Ok(None) => {
                        log("stdin ended; shutting down");
                        break;
                    }
                    Err(error) => {
                        log(&format!("stdin error: {error}; shutting down"));
                        break;
                    }
                }
            }
            _ = &mut shutdown => {
                log("signal received; shutting down");
                break;
            }
        }
    }

    peer.close().await;
    0
}

/// Handle one JSON-RPC line: dispatch a request, ignore notifications, and
/// write at most one response line.
async fn handle_line(line: &str, peer: &PeerClient, stdout: &mut tokio::io::Stdout) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }

    let message: Value = match serde_json::from_str(trimmed) {
        Ok(message) => message,
        Err(error) => {
            log(&format!("invalid JSON on stdin: {error}"));
            write_json_line(stdout, &parse_error()).await;
            return;
        }
    };

    let Some(object) = message.as_object() else {
        write_json_line(stdout, &invalid_request(Value::Null, "Invalid Request")).await;
        return;
    };

    let Some(method) = object.get("method").and_then(Value::as_str) else {
        // A response from the client (we never send requests) is ignored.
        return;
    };
    let id = object.get("id").cloned().unwrap_or(Value::Null);

    // Notifications (no id) are never answered. `notifications/initialized` is
    // the only one with a documented effect in the legacy server, and it has
    // none beyond accepting the handshake.
    if id.is_null() {
        return;
    }

    let params = object.get("params").cloned().unwrap_or(Value::Null);
    let response = match method {
        "initialize" => ok_response(id, initialize_result(&params)),
        "ping" => ok_response(id, json!({})),
        "tools/list" => ok_response(id, json!({ "tools": tools::tool_definitions() })),
        "tools/call" => ok_response(id, call_tool_result(peer, &params).await),
        other => {
            log(&format!("unknown method: {other}"));
            error_response(id, -32601, "Method not found")
        }
    };
    write_json_line(stdout, &response).await;
}

/// Build the `initialize` result (protocol negotiation + exact instructions).
fn initialize_result(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("");
    let protocol_version = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
        requested.to_string()
    } else {
        SUPPORTED_PROTOCOL_VERSIONS[0].to_string()
    };

    json!({
        "protocolVersion": protocol_version,
        "capabilities": { "tools": { "listChanged": true } },
        "serverInfo": { "name": SERVER_NAME, "version": VERSION },
        "instructions": INSTRUCTIONS,
    })
}

/// Execute a `tools/call` and build the MCP `CallToolResult` JSON.
async fn call_tool_result(peer: &PeerClient, params: &Value) -> Value {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // The legacy SDK answers unknown tools with an isError result rather than a
    // JSON-RPC error (`MCP error -32602: Tool <name> not found`).
    if !tools::is_known_tool(&name) {
        log(&format!("unknown tool: {name}"));
        return tool_error_result(&format!("MCP error -32602: Tool {name} not found"));
    }

    match tools::call_tool(peer, &name, arguments).await {
        Ok(result) => json!({ "content": result.content }),
        Err(error) => {
            log(&format!("tool {name} failed: {}", error.0));
            let message = if error.0.starts_with("Input validation error:") {
                // SDK-level schema validation is reported verbatim.
                error.0
            } else {
                // Handler failures mirror the legacy `wrap()`: `Error: <message>`.
                format!("Error: {}", error.0)
            };
            tool_error_result(&message)
        }
    }
}

fn tool_error_result(message: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true,
    })
}

// ── JSON-RPC envelopes ───────────────────────────────────────────────────

fn ok_response(id: Value, result: Value) -> Value {
    let mut object = Map::new();
    object.insert("jsonrpc".to_string(), json!("2.0"));
    object.insert("id".to_string(), id);
    object.insert("result".to_string(), result);
    Value::Object(object)
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

fn parse_error() -> Value {
    error_response(Value::Null, -32700, "Parse error")
}

fn invalid_request(id: Value, message: &str) -> Value {
    error_response(id, -32600, message)
}

async fn write_json_line(stdout: &mut tokio::io::Stdout, value: &Value) {
    let mut line = match serde_json::to_string(value) {
        Ok(line) => line,
        Err(error) => {
            log(&format!("unable to encode JSON-RPC response: {error}"));
            return;
        }
    };
    line.push('\n');
    if let Err(error) = stdout.write_all(line.as_bytes()).await {
        log(&format!("stdout write failed: {error}"));
        return;
    }
    if let Err(error) = stdout.flush().await {
        log(&format!("stdout flush failed: {error}"));
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// `[browser-use] <line>` on stderr, matching the legacy log prefix.
fn log(line: &str) {
    eprintln!("[{SERVER_NAME}] {line}");
}

/// Resolve the hub port from `ORANGE_PORT` (mirrors `hub::resolve_port`).
fn resolve_port() -> u16 {
    let Ok(raw) = std::env::var("ORANGE_PORT") else {
        return DEFAULT_PORT;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return DEFAULT_PORT;
    }
    match trimmed.parse::<u32>() {
        Ok(port) if (1..=65_535).contains(&port) => port as u16,
        _ => {
            log(&format!(
                "ORANGE_PORT must be an integer between 1 and 65535 (got \"{raw}\"); using {DEFAULT_PORT}"
            ));
            DEFAULT_PORT
        }
    }
}

/// Resolve on SIGINT/SIGTERM; never resolves on platforms without them.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut interrupt = match signal(SignalKind::interrupt()) {
            Ok(stream) => stream,
            Err(_) => {
                std::future::pending::<()>().await;
                return;
            }
        };
        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(stream) => stream,
            Err(_) => {
                std::future::pending::<()>().await;
                return;
            }
        };
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(windows)]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    #[cfg(not(any(unix, windows)))]
    {
        std::future::pending::<()>().await;
    }
}

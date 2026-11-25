//! Stdio MCP binary tests (`browser-use mcp`).
//!
//! Every test spawns the real binary (via `CARGO_BIN_EXE_browser-use`) with
//! `ORANGE_PORT` / `ORANGE_SECRET` pointing at an in-process `Hub` +
//! `FakeExtension` from `rust/tests/common/mod.rs`, speaks line-delimited
//! JSON-RPC over its stdin/stdout, and asserts both the wire output and the
//! exact commands the extension receives. The machine's legacy hub on 19455 is
//! never touched: all ports are ephemeral.
//!
//! The child process I/O is driven with `std::process` + reader threads rather
//! than `tokio::process` (the crate does not enable the `process` feature), so
//! the tests use a multi-threaded Tokio runtime and block the test thread while
//! the hub tasks make progress on other workers.

mod common;

use std::collections::VecDeque;
use std::io::{BufRead as _, BufReader, Write as _};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use common::{free_port, start_hub, FakeReply, TestServer, TEST_SECRET};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(25);

/// The exact `initialize.instructions` string from the legacy `src/index.ts`.
const LEGACY_INSTRUCTIONS: &str =
    "Call browser_snapshot before interacting, then act with durable @eN refs \
using browser_click, browser_fill, or browser_press. Re-snapshot (or use delta=true) after actions \
that change the page. @eN refs reset after navigation; pass a CSS selector (for example \
selector=\"iframe\") to inspect a new page. Pass interactive: false to read page text, tables, or \
status values. browser_screenshot returns an annotated image plus its ref legend.";

/// The 12 tools in legacy registration order.
const EXPECTED_TOOLS: [&str; 12] = [
    "browser_snapshot",
    "browser_click",
    "browser_fill",
    "browser_press",
    "browser_select_or_check",
    "browser_locate",
    "browser_wait",
    "browser_navigate",
    "browser_tabs",
    "browser_read",
    "browser_screenshot",
    "browser_eval_and_network",
];

// ── MCP stdio client ─────────────────────────────────────────────────────

/// A spawned `browser-use mcp` child plus a blocking line-delimited JSON-RPC
/// client. Responses can arrive in any order; they are buffered by id.
struct McpClient {
    child: Child,
    stdin: Option<ChildStdin>,
    responses: Receiver<Value>,
    buffered: VecDeque<Value>,
    violations: Arc<Mutex<Vec<String>>>,
    stderr: Arc<Mutex<String>>,
    next_id: i64,
}

impl McpClient {
    /// Spawn `browser-use mcp` with `ORANGE_PORT`/`ORANGE_SECRET` in its env.
    fn spawn(port: u16, secret: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_browser-use"))
            .arg("mcp")
            .env("ORANGE_PORT", port.to_string())
            .env("ORANGE_SECRET", secret)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn browser-use mcp");

        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let stderr_pipe = child.stderr.take().expect("child stderr");

        let (tx, responses) = mpsc::channel();
        let violations = Arc::new(Mutex::new(Vec::new()));
        let stdout_violations = Arc::clone(&violations);
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(trimmed) {
                    Ok(value) => {
                        if tx.send(value).is_err() {
                            break;
                        }
                    }
                    Err(_) => stdout_violations
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(trimmed.to_string()),
                }
            }
        });

        let stderr = Arc::new(Mutex::new(String::new()));
        let stderr_buffer = Arc::clone(&stderr);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr_pipe);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => stderr_buffer
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push_str(&line),
                }
            }
        });

        Self {
            child,
            stdin: Some(stdin),
            responses,
            buffered: VecDeque::new(),
            violations,
            stderr,
            next_id: 1,
        }
    }

    fn write_line(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("stdin still open");
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .unwrap_or_else(|error| panic!("write to mcp stdin failed: {error}"));
    }

    /// Send a JSON-RPC request and wait for the response with that id.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        })
        .to_string();
        self.write_line(&line);
        self.await_id(id)
    }

    fn notify(&mut self, method: &str, params: Value) {
        let line = json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string();
        self.write_line(&line);
    }

    fn await_id(&mut self, id: i64) -> Value {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        loop {
            if let Some(index) = self
                .buffered
                .iter()
                .position(|value| value.get("id").and_then(Value::as_i64) == Some(id))
            {
                return self.buffered.remove(index).expect("buffered response");
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                panic!(
                    "timed out waiting for JSON-RPC id {id}\nstderr:\n{}",
                    self.stderr_snapshot()
                );
            }
            match self.responses.recv_timeout(remaining) {
                Ok(value) => self.buffered.push_back(value),
                Err(error) => panic!(
                    "no response for JSON-RPC id {id} ({error})\nstderr:\n{}",
                    self.stderr_snapshot()
                ),
            }
        }
    }

    fn initialize(&mut self) -> Value {
        let response = self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "browser-use-test", "version": "1.0.0" },
            }),
        );
        self.notify("notifications/initialized", json!({}));
        response
            .get("result")
            .cloned()
            .unwrap_or_else(|| panic!("initialize returned no result: {response}"))
    }

    /// Call one tool and return the raw MCP `CallToolResult` object.
    fn call_tool(&mut self, name: &str, arguments: Value) -> Value {
        let response = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        response
            .get("result")
            .cloned()
            .unwrap_or_else(|| panic!("tools/call {name} returned no result: {response}"))
    }

    fn tool_text(&mut self, name: &str, arguments: Value) -> String {
        text_of(&self.call_tool(name, arguments))
    }

    /// Call one tool, assert it is an `isError` result, and return its text.
    fn tool_error(&mut self, name: &str, arguments: Value) -> String {
        let result = self.call_tool(name, arguments);
        assert_eq!(
            result.get("isError"),
            Some(&Value::Bool(true)),
            "expected {name} to fail, got: {result}"
        );
        text_of(&result)
    }

    fn stdout_violations(&self) -> Vec<String> {
        self.violations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn stderr_snapshot(&self) -> String {
        self.stderr
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn wait_for_exit(&mut self) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return status.code().unwrap_or(-1),
                Ok(None) => {
                    assert!(
                        Instant::now() < deadline,
                        "mcp child did not exit\nstderr:\n{}",
                        self.stderr_snapshot()
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("unable to wait for mcp child: {error}"),
            }
        }
    }

    /// Close stdin (clean EOF shutdown) and return `(exit code, stderr)`.
    fn finish(mut self) -> (i32, String) {
        drop(self.stdin.take());
        let code = self.wait_for_exit();
        // The stderr reader thread may still be draining the pipe.
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.stderr_snapshot().contains("shutting down") && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        (code, self.stderr_snapshot())
    }
}

fn text_of(result: &Value) -> String {
    result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks
                .iter()
                .find(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        })
        .and_then(|block| block.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn image_of(result: &Value) -> Value {
    result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks
                .iter()
                .find(|block| block.get("type").and_then(Value::as_str) == Some("image"))
        })
        .cloned()
        .unwrap_or_else(|| panic!("no image content block in: {result}"))
}

// ── Fixture extension ────────────────────────────────────────────────────

/// Start a hub + fake extension answering like the legacy `mcp-tools.test.ts`
/// fixture, and return the recorded `cmd` frames.
async fn start_fixture() -> (TestServer, Arc<Mutex<Vec<Value>>>) {
    let server = TestServer::start().await;
    let recorded: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&recorded);
    server.extension.set_handler(move |message| {
        sink.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(message.clone());
        fixture_reply(&message.get("cmd").cloned().unwrap_or(Value::Null))
    });
    (server, recorded)
}

fn fixture_reply(cmd: &Value) -> FakeReply {
    let action = cmd.get("action").and_then(Value::as_str).unwrap_or("");
    match action {
        // Unscoped snapshots are large so the 12k default cap is exercised;
        // the scoped snapshot stays tiny.
        "snapshot" => {
            let tree = if cmd.get("selector").is_none() {
                format!(
                    "{}- button \"Run\" [ref=e1]",
                    "- StaticText \"row\"\n".repeat(900)
                )
            } else {
                "- button \"Run\" [ref=e1]".to_string()
            };
            FakeReply::Ok(json!({
                "ok": true,
                "kind": "full",
                "title": "MCP Fixture",
                "url": "https://mcp.example",
                "tabId": 9,
                "revision": 6,
                "tree": tree,
            }))
        }
        "click" | "dblclick" | "contextmenu" | "fill" | "type" | "press" | "check" | "uncheck"
        | "select" | "hover" | "scroll" | "focus" => {
            let selector = cmd.get("selector").cloned().unwrap_or(Value::Null);
            let selector_text = selector.as_str().unwrap_or("");
            let reference = if selector_text == "body" {
                Value::Null
            } else if is_ref(selector_text) {
                json!(selector_text.trim_start_matches('@'))
            } else {
                json!("e1")
            };
            let fillish = action == "fill" || action == "type";
            let pressed_enter = action == "press" && cmd.get("key") == Some(&json!("Enter"));
            FakeReply::Ok(json!({
                "ok": true,
                "ref": reference,
                "role": if fillish { "textbox" } else { "button" },
                "name": if fillish { "Email" } else { "Run" },
                "checked": if action == "check" { json!(true) } else { Value::Null },
                "value": if action == "select" { json!("b") } else { Value::Null },
                "scrollY": if action == "scroll" { json!(120) } else { Value::Null },
                "navigated": if pressed_enter { json!(true) } else { Value::Null },
                "navigationStatus": if pressed_enter { json!("complete") } else { Value::Null },
                "tabId": 9,
                "status": "complete",
            }))
        }
        "locate" => FakeReply::Ok(json!({
            "ok": true, "ref": "e7", "role": "button", "name": "Save", "matches": 1,
        })),
        "wait" => FakeReply::Ok(json!({ "ok": true, "state": "visible", "waited": 32 })),
        "navigate" | "back" | "forward" | "reload" => FakeReply::Ok(json!({
            "ok": true,
            "tabId": 9,
            "url": cmd.get("url").cloned().unwrap_or(json!("https://mcp.example")),
            "title": "MCP Fixture",
            "status": "complete",
        })),
        "tab_list" => FakeReply::Ok(json!({
            "ok": true,
            "tabs": [{ "id": 9, "active": true, "title": "MCP Fixture", "url": "https://mcp.example" }],
        })),
        "tab_new" | "tab_switch" => FakeReply::Ok(json!({
            "ok": true, "id": 9, "boundTabId": 9, "tabId": 9, "active": true,
            "title": "MCP Fixture", "url": "https://mcp.example",
        })),
        "tab_bind" => FakeReply::Ok(json!({
            "ok": true, "id": 9, "boundTabId": cmd.get("tabId").cloned().unwrap_or(Value::Null),
            "tabId": 9, "active": true, "title": "MCP Fixture", "url": "https://mcp.example",
        })),
        "tab_close" => FakeReply::Ok(json!({ "ok": true, "closed": 9 })),
        "url" => FakeReply::Ok(json!({ "ok": true, "url": "https://mcp.example", "tabId": 9 })),
        "read" => {
            let url = cmd.get("url").and_then(Value::as_str).unwrap_or("");
            let outline = cmd.get("outline") == Some(&Value::Bool(true));
            let markdown = if url.contains("huge") {
                "x".repeat(95_000)
            } else if url.contains("empty") {
                String::new()
            } else if url.contains("truncated") {
                "partial text".to_string()
            } else {
                "# MCP Fixture\n\nHello from the reader.".to_string()
            };
            let mut result = json!({
                "ok": true,
                "source": "tab",
                "url": if url.is_empty() { "https://mcp.example" } else { url },
                "title": "MCP Fixture",
                "markdown": markdown,
                "truncated": url.contains("truncated"),
            });
            if outline {
                result["outline"] = json!("# Heading\n## Subheading");
            }
            FakeReply::Ok(result)
        }
        "screenshot" => {
            let selector = cmd.get("selector").and_then(Value::as_str);
            let format = cmd.get("format").and_then(Value::as_str).unwrap_or("jpeg");
            let mut result = json!({
                "ok": true,
                "tabId": 9,
                "base64": "ZmFrZS1pbWFnZQ==",
                "format": format,
                "annotate": true,
                "annotations": [],
                "notes": [],
            });
            match selector {
                Some("none") => {}
                Some("many") => {
                    result["annotations"] = Value::Array(
                        (1..=205)
                            .map(|index| {
                                json!({
                                    "label": index.to_string(),
                                    "ref": format!("e{index}"),
                                    "role": "button",
                                    "name": format!("Item {index}"),
                                })
                            })
                            .collect(),
                    );
                    result["notes"] = json!(["pinned annotations are approximate"]);
                }
                _ => {
                    result["annotations"] = json!([
                        { "label": "1", "ref": "e1", "role": "button", "name": "Run" }
                    ]);
                    result["notes"] = json!([]);
                }
            }
            FakeReply::Ok(result)
        }
        "evaluate" => {
            let expression = cmd.get("expression").and_then(Value::as_str).unwrap_or("");
            if expression.contains("isolated") {
                FakeReply::Ok(json!({
                    "ok": true, "result": "blocked", "world": "isolated",
                }))
            } else if expression.contains("string") {
                FakeReply::Ok(json!({ "ok": true, "result": "hello from the page" }))
            } else {
                FakeReply::Ok(json!({ "ok": true, "result": { "answer": 42 } }))
            }
        }
        "requests" => FakeReply::Ok(json!({
            "ok": true,
            "count": 1,
            "requests": [{
                "method": "GET",
                "status": 200,
                "timeMs": 12,
                "url": "https://mcp.example/api",
            }],
        })),
        "route" => FakeReply::Ok(json!({
            "ok": true,
            "route": { "urlPattern": cmd.get("urlPattern").cloned().unwrap_or(Value::Null) },
            "routes": [{ "urlPattern": cmd.get("urlPattern").cloned().unwrap_or(Value::Null) }],
        })),
        "unroute" => FakeReply::Ok(json!({
            "ok": true, "removed": 1, "routes": [{ "urlPattern": "/api/**" }],
        })),
        "diff_snapshot" => FakeReply::Ok(json!({
            "ok": true,
            "additions": 3,
            "removals": 1,
            "diff": "+ button \"New\" [ref=e9]",
        })),
        _ => FakeReply::Ok(json!({ "ok": true })),
    }
}

fn is_ref(text: &str) -> bool {
    let trimmed = text.trim_start_matches('@');
    trimmed.len() > 1
        && trimmed.starts_with('e')
        && trimmed[1..]
            .chars()
            .all(|character| character.is_ascii_digit())
}

/// The most recent recorded `cmd` object for `action`.
fn last_command(recorded: &Arc<Mutex<Vec<Value>>>, action: &str) -> Value {
    let messages = recorded
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    messages
        .iter()
        .rev()
        .find(|message| {
            message
                .get("cmd")
                .and_then(|cmd| cmd.get("action"))
                .and_then(Value::as_str)
                == Some(action)
        })
        .and_then(|message| message.get("cmd").cloned())
        .unwrap_or_else(|| panic!("no {action} command recorded"))
}

/// The most recent recorded full command frame (for `tabId`/`timeoutMs`).
fn last_frame(recorded: &Arc<Mutex<Vec<Value>>>, action: &str) -> Value {
    let messages = recorded
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    messages
        .iter()
        .rev()
        .find(|message| {
            message
                .get("cmd")
                .and_then(|cmd| cmd.get("action"))
                .and_then(Value::as_str)
                == Some(action)
        })
        .cloned()
        .unwrap_or_else(|| panic!("no {action} command frame recorded"))
}

/// Count recorded commands with `action` and a `selector` matching `wanted`.
fn recorded_selectors(recorded: &Arc<Mutex<Vec<Value>>>, action: &str) -> Vec<String> {
    let messages = recorded
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    messages
        .iter()
        .filter(|message| {
            message
                .get("cmd")
                .and_then(|cmd| cmd.get("action"))
                .and_then(Value::as_str)
                == Some(action)
        })
        .filter_map(|message| {
            message
                .get("cmd")
                .and_then(|cmd| cmd.get("selector"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

// ── Handshake / surface ──────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn initialize_ping_and_tools_list_match_legacy() {
    let (server, port) = start_hub().await;
    let mut client = McpClient::spawn(port, TEST_SECRET);

    let initialize = client.initialize();
    assert_eq!(initialize["protocolVersion"], "2025-06-18");
    assert_eq!(initialize["serverInfo"]["name"], "browser-use");
    assert_eq!(initialize["serverInfo"]["version"], "0.1.0");
    assert_eq!(initialize["capabilities"]["tools"]["listChanged"], true);
    assert_eq!(
        initialize["instructions"].as_str().unwrap_or_default(),
        LEGACY_INSTRUCTIONS
    );

    let ping = client.request("ping", json!({}));
    assert_eq!(ping["result"], json!({}));

    let listed = client.request("tools/list", json!({}));
    let tools = listed["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list returned no tools: {listed}"));
    assert_eq!(tools.len(), 12, "expected all 12 tools: {listed}");
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect();
    assert_eq!(names, EXPECTED_TOOLS);

    // Schema/default parity checks ported from test/mcp-tools.test.ts.
    let snapshot = tools
        .iter()
        .find(|tool| tool["name"] == "browser_snapshot")
        .expect("browser_snapshot definition");
    assert_eq!(snapshot["title"], "Browser snapshot");
    let snapshot_description = snapshot["description"].as_str().unwrap_or_default();
    assert!(snapshot_description.contains("interactive: false"));
    let selector_schema = &snapshot["inputSchema"]["properties"]["selector"];
    assert!(selector_schema["description"]
        .as_str()
        .unwrap_or_default()
        .contains("refs reset after navigation"));
    let click = tools
        .iter()
        .find(|tool| tool["name"] == "browser_click")
        .expect("browser_click definition");
    assert_eq!(click["inputSchema"]["required"], json!(["target"]));
    let fill = tools
        .iter()
        .find(|tool| tool["name"] == "browser_fill")
        .expect("browser_fill definition");
    assert_eq!(fill["inputSchema"]["required"], json!(["target", "value"]));
    let fill_delay = &fill["inputSchema"]["properties"]["delayMs"];
    assert_eq!(fill_delay["minimum"], json!(0));
    assert_eq!(fill_delay["maximum"], json!(10000));
    let wait = tools
        .iter()
        .find(|tool| tool["name"] == "browser_wait")
        .expect("browser_wait definition");
    assert_eq!(
        wait["inputSchema"]["properties"]["state"]["enum"],
        json!(["visible", "hidden", "attached", "detached"])
    );
    assert_eq!(
        wait["inputSchema"]["properties"]["timeoutMs"]["maximum"],
        json!(120000)
    );

    let violations = client.stdout_violations();
    let (code, stderr) = client.finish();
    assert!(violations.is_empty(), "non-JSON-RPC stdout: {violations:?}");
    assert_eq!(code, 0);
    assert!(stderr.contains("[browser-use]"), "stderr: {stderr}");
    assert!(
        stderr.contains("stdin ended; shutting down"),
        "stderr: {stderr}"
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_tool_and_method_errors_match_legacy() {
    let (server, port) = start_hub().await;
    let mut client = McpClient::spawn(port, TEST_SECRET);
    client.initialize();

    // Unknown tool: legacy SDK answers with an isError CallToolResult.
    let text = client.tool_error("nope", json!({}));
    assert_eq!(text, "MCP error -32602: Tool nope not found");

    // Unknown method: JSON-RPC -32601 Method not found.
    let response = client.request("bogus/method", json!({}));
    assert_eq!(response["error"]["code"], json!(-32601));
    assert_eq!(response["error"]["message"], "Method not found");

    // Notifications never get a response: the next response is for `ping`.
    client.notify("some/notification", json!({ "x": 1 }));
    let ping = client.request("ping", json!({}));
    assert_eq!(ping["result"], json!({}));

    let violations = client.stdout_violations();
    let (code, _stderr) = client.finish();
    assert!(violations.is_empty(), "non-JSON-RPC stdout: {violations:?}");
    assert_eq!(code, 0);
    server.stop().await;
}

// ── Offline / disconnected paths ─────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_hub_returns_actionable_error_without_crashing() {
    let port = free_port();
    let mut client = McpClient::spawn(port, TEST_SECRET);

    // MCP mode must work with no hub at all (lazy peer connect).
    client.initialize();
    let text = client.tool_error("browser_snapshot", json!({}));
    assert_eq!(
        text,
        format!(
            "Error: Browser Use is not running at ws://127.0.0.1:{port}. Launch {} first.",
            common::APP_LAUNCH_NAME
        )
    );

    // The server stays alive after a failed tool call.
    let listed = client.request("tools/list", json!({}));
    assert_eq!(listed["result"]["tools"].as_array().map(Vec::len), Some(12));

    let violations = client.stdout_violations();
    let (code, stderr) = client.finish();
    assert!(violations.is_empty(), "non-JSON-RPC stdout: {violations:?}");
    assert_eq!(code, 0);
    assert!(
        stderr.contains("Browser Use is not running"),
        "stderr: {stderr}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_without_extension_reports_extension_not_connected() {
    let (hub, port) = start_hub().await;
    let mut client = McpClient::spawn(port, TEST_SECRET);
    client.initialize();

    let text = client.tool_error("browser_snapshot", json!({}));
    assert!(text.starts_with("Error: "), "text: {text}");
    assert!(
        text.contains("Gecko Orange extension is not connected to ws://127.0.0.1:"),
        "text: {text}"
    );
    assert!(text.contains("browser-use secret"), "text: {text}");

    let (code, _stderr) = client.finish();
    assert_eq!(code, 0);
    hub.stop().await;
}

// ── The 12 tools over a live fixture ─────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tools_map_to_extension_commands_and_render_text() {
    let (server, recorded) = start_fixture().await;
    let mut client = McpClient::spawn(server.port, TEST_SECRET);
    client.initialize();

    // browser_snapshot: interactive defaults flip on `selector`.
    client.call_tool("browser_snapshot", json!({ "tabId": 9 }));
    assert_eq!(
        last_command(&recorded, "snapshot")["interactive"],
        json!(true)
    );
    client.call_tool(
        "browser_snapshot",
        json!({ "selector": "main", "tabId": 9 }),
    );
    assert_eq!(
        last_command(&recorded, "snapshot")["interactive"],
        json!(false)
    );

    let snapshot = client.tool_text(
        "browser_snapshot",
        json!({ "interactive": false, "compact": false, "urls": true, "selector": "main", "tabId": 9 }),
    );
    assert!(
        snapshot.contains("Title: MCP Fixture | URL: https://mcp.example | Tab: 9 | Revision: 6"),
        "snapshot: {snapshot}"
    );
    assert!(snapshot.contains("button \"Run\""));
    assert!(
        !snapshot.contains("truncated"),
        "scoped snapshot clipped: {snapshot}"
    );
    let snapshot_cmd = last_command(&recorded, "snapshot");
    assert_eq!(snapshot_cmd["interactive"], json!(false));
    assert_eq!(snapshot_cmd["compact"], json!(false));
    assert_eq!(snapshot_cmd["urls"], json!(true));
    assert_eq!(snapshot_cmd["selector"], json!("main"));
    assert_eq!(last_frame(&recorded, "snapshot")["tabId"], json!(9));

    // Unscoped snapshots get the 12k cap plus the selector hint.
    let truncated = client.tool_text(
        "browser_snapshot",
        json!({ "interactive": true, "compact": true, "tabId": 9 }),
    );
    assert!(
        truncated.contains("truncated to 12,000 chars"),
        "text: {truncated}"
    );
    assert!(
        truncated.contains("pass selector=\"...\" or interactive=true"),
        "text: {truncated}"
    );
    assert!(truncated.len() < 13_000, "text len: {}", truncated.len());
    let budget = last_command(&recorded, "snapshot")["maxChars"]
        .as_i64()
        .expect("maxChars");
    assert!(budget > 2_000 && budget <= 12_000, "maxChars: {budget}");

    // browser_click: double + force map to dblclick.
    let click = client.tool_text(
        "browser_click",
        json!({ "target": "@e1", "double": true, "force": true, "tabId": 9 }),
    );
    assert_eq!(click, "Double-clicked @e1 button \"Run\".");
    let click_cmd = last_command(&recorded, "dblclick");
    assert_eq!(click_cmd["selector"], json!("@e1"));
    assert_eq!(click_cmd["force"], json!(true));

    // browser_fill: slow typing + delay + Enter navigation reporting.
    let fill = client.tool_text(
        "browser_fill",
        json!({
            "target": "@e1", "value": "hi", "slowType": true, "delayMs": 5,
            "pressEnter": true, "tabId": 9,
        }),
    );
    assert_eq!(
        fill,
        "Typed @e1 textbox \"Email\" with \"hi\". Pressed Enter on @e1 button \"Run\". \
         Waited for navigation (status=complete)."
    );
    assert_eq!(last_command(&recorded, "type")["delay"], json!(5));
    assert_eq!(last_command(&recorded, "press")["key"], json!("Enter"));

    // browser_press: default target is body.
    let press = client.tool_text("browser_press", json!({ "key": "Control+a", "tabId": 9 }));
    assert_eq!(press, "Pressed \"Control+a\" on the page.");
    assert_eq!(last_command(&recorded, "press")["selector"], json!("body"));
    assert_eq!(last_frame(&recorded, "press")["tabId"], json!(9));

    // browser_select_or_check: single + batched check.
    let checked = client.tool_text(
        "browser_select_or_check",
        json!({ "action": "check", "target": "@e1", "tabId": 9 }),
    );
    assert_eq!(checked, "Checked @e1 button \"Run\" (checked=true).");

    let batch = client.tool_text(
        "browser_select_or_check",
        json!({ "action": "check", "target": ["@e190", "@e191", "@e192"], "tabId": 9 }),
    );
    assert_eq!(batch, "Checked 3 elements: @e190, @e191, @e192.");
    let batch_selectors: Vec<String> = recorded_selectors(&recorded, "check")
        .into_iter()
        .filter(|selector| selector.starts_with("@e19"))
        .collect();
    assert_eq!(batch_selectors, vec!["@e190", "@e191", "@e192"]);

    let selected = client.tool_text(
        "browser_select_or_check",
        json!({ "action": "select", "target": "@e1", "value": "b", "tabId": 9 }),
    );
    assert_eq!(
        selected,
        "Selected \"b\" in @e1 button \"Run\" (current value \"b\")."
    );

    let scrolled = client.tool_text(
        "browser_select_or_check",
        json!({ "action": "scroll", "tabId": 9 }),
    );
    assert_eq!(scrolled, "Scrolled to y=120.");
    assert_eq!(last_command(&recorded, "scroll")["y"], json!(0));

    // browser_locate.
    let located = client.tool_text(
        "browser_locate",
        json!({ "by": "role", "role": "button", "query": "Save", "tabId": 9 }),
    );
    assert_eq!(located, "Located @e7 button \"Save\" (1 match).");
    let locate_cmd = last_command(&recorded, "locate");
    assert_eq!(locate_cmd["role"], json!("button"));
    assert_eq!(locate_cmd["name"], json!("Save"));
    assert_eq!(locate_cmd["exact"], json!(false));
    assert_eq!(last_frame(&recorded, "locate")["tabId"], json!(9));

    // browser_wait: condition omits/sets state and uses `timeoutMs`.
    let waited = client.tool_text(
        "browser_wait",
        json!({ "selector": "#done", "timeoutMs": 50, "tabId": 9 }),
    );
    assert_eq!(
        waited,
        "Waited 32ms for selector \"#done\" (state=visible)."
    );
    let wait_frame = last_frame(&recorded, "wait");
    assert_eq!(wait_frame["cmd"]["timeoutMs"], json!(50));
    assert_eq!(wait_frame["cmd"]["state"], json!("visible"));

    // ADR: omitting selector/text pauses and omits `state` entirely.
    let settled = client.call_tool("browser_wait", json!({ "timeoutMs": 250, "tabId": 9 }));
    assert_eq!(settled.get("isError"), None);
    assert_eq!(text_of(&settled), "Waited 32ms.");
    let settled_cmd = last_command(&recorded, "wait");
    assert_eq!(settled_cmd["timeoutMs"], json!(250));
    assert!(settled_cmd.get("selector").is_none(), "cmd: {settled_cmd}");
    assert!(settled_cmd.get("state").is_none(), "cmd: {settled_cmd}");

    // browser_navigate: goto maps to `navigate` with a 40s command timeout.
    let navigate = client.tool_text(
        "browser_navigate",
        json!({ "action": "goto", "url": "https://mcp.example/next", "tabId": 9 }),
    );
    assert_eq!(
        navigate,
        "Navigated to https://mcp.example/next (status=complete, tab 9)."
    );
    assert_eq!(
        last_command(&recorded, "navigate")["url"],
        json!("https://mcp.example/next")
    );

    // browser_tabs: list / new / switch / bind / close.
    let listed = client.tool_text("browser_tabs", json!({ "action": "list" }));
    assert_eq!(listed, "Tabs (1):\n* [9] MCP Fixture — https://mcp.example");

    let new_tab = client.tool_text(
        "browser_tabs",
        json!({ "action": "new", "url": "https://mcp.example/new" }),
    );
    assert_eq!(new_tab, "Opened [9] MCP Fixture — https://mcp.example.");
    assert_eq!(
        last_command(&recorded, "tab_new")["url"],
        json!("https://mcp.example/new")
    );

    let switched = client.tool_text("browser_tabs", json!({ "action": "switch", "tabId": 9 }));
    assert_eq!(
        switched,
        "Switched to [9] MCP Fixture — https://mcp.example."
    );

    let bound = client.tool_text("browser_tabs", json!({ "action": "bind", "tabId": 9 }));
    assert_eq!(bound, "Bound to [9] MCP Fixture — https://mcp.example.");
    assert_eq!(last_command(&recorded, "tab_bind")["tabId"], json!(9));

    let unbound = client.tool_text("browser_tabs", json!({ "action": "bind" }));
    assert_eq!(unbound, "Unbound the agent tab (using the active tab).");
    assert_eq!(last_command(&recorded, "tab_bind")["tabId"], Value::Null);

    let closed = client.tool_text("browser_tabs", json!({ "action": "close", "tabId": 9 }));
    assert_eq!(closed, "Closed tab 9.");

    // ADR: close without tabId looks up the active tab first.
    let closed_active = client.tool_text("browser_tabs", json!({ "action": "close" }));
    assert_eq!(closed_active, "Closed tab 9.");
    assert_eq!(last_command(&recorded, "tab_close")["tabId"], json!(9));

    // browser_read: markdown, llms URL discovery, outline.
    let read = client.tool_text(
        "browser_read",
        json!({ "url": "https://mcp.example", "tabId": 9 }),
    );
    assert!(read.contains("Hello from the reader."), "read: {read}");

    let read_llms = client.tool_text("browser_read", json!({ "llms": "index", "tabId": 9 }));
    assert!(
        read_llms.contains("Hello from the reader."),
        "read: {read_llms}"
    );
    assert_eq!(last_command(&recorded, "url")["action"], json!("url"));
    assert_eq!(
        last_command(&recorded, "read")["url"],
        json!("https://mcp.example")
    );

    let outline = client.tool_text(
        "browser_read",
        json!({ "url": "https://mcp.example", "outline": true }),
    );
    assert_eq!(outline, "# Heading\n## Subheading");

    // browser_screenshot: [image, text] blocks with the ref legend.
    let screenshot = client.call_tool(
        "browser_screenshot",
        json!({ "annotate": true, "format": "jpeg", "tabId": 9 }),
    );
    let image = image_of(&screenshot);
    assert_eq!(image["mimeType"], "image/jpeg");
    assert_eq!(image["data"], "ZmFrZS1pbWFnZQ==");
    assert!(
        text_of(&screenshot).contains("[1] @e1 button \"Run\""),
        "text: {screenshot}"
    );

    let png = client.call_tool("browser_screenshot", json!({ "format": "png" }));
    assert_eq!(image_of(&png)["mimeType"], "image/png");

    // browser_eval_and_network: evaluate / requests.
    let evaluated = client.tool_text(
        "browser_eval_and_network",
        json!({ "action": "evaluate", "expression": "1 + 41", "tabId": 9 }),
    );
    assert!(evaluated.contains("\"answer\": 42"), "eval: {evaluated}");

    let string_eval = client.tool_text(
        "browser_eval_and_network",
        json!({ "action": "evaluate", "expression": "string()" }),
    );
    assert_eq!(string_eval, "Result:\nhello from the page");

    let isolated = client.tool_text(
        "browser_eval_and_network",
        json!({ "action": "evaluate", "expression": "isolated()" }),
    );
    assert!(
        isolated.contains("isolated content-script world; MAIN world eval was blocked by CSP"),
        "eval: {isolated}"
    );

    let requests = client.tool_text(
        "browser_eval_and_network",
        json!({ "action": "requests", "filter": "/api", "limit": 10, "clear": true }),
    );
    assert_eq!(
        requests,
        "Requests (1):\n200 GET 12ms https://mcp.example/api"
    );
    let requests_cmd = last_command(&recorded, "requests");
    assert_eq!(requests_cmd["filter"], json!("/api"));
    assert_eq!(requests_cmd["limit"], json!(10));
    assert_eq!(requests_cmd["clear"], json!(true));

    // route / unroute / diff_snapshot.
    let route = client.tool_text(
        "browser_eval_and_network",
        json!({ "action": "route", "urlPattern": "/api/**", "status": 201, "abort": false }),
    );
    assert_eq!(route, "Route \"/api/**\" configured (1 active).");
    let route_cmd = last_command(&recorded, "route");
    assert_eq!(route_cmd["urlPattern"], json!("/api/**"));
    assert_eq!(route_cmd["status"], json!(201));
    assert_eq!(route_cmd["abort"], json!(false));

    let unroute = client.tool_text("browser_eval_and_network", json!({ "action": "unroute" }));
    assert_eq!(unroute, "Removed 1 route(s); 1 remain.");

    let diff = client.tool_text(
        "browser_eval_and_network",
        json!({ "action": "diff_snapshot" }),
    );
    assert!(
        diff.starts_with("Snapshot diff (3+ / 1-):\n\n+ button \"New\" [ref=e9]"),
        "diff: {diff}"
    );

    let violations = client.stdout_violations();
    let (code, stderr) = client.finish();
    assert!(violations.is_empty(), "non-JSON-RPC stdout: {violations:?}");
    assert_eq!(code, 0);
    assert!(stderr.contains("[browser-use]"), "stderr: {stderr}");
    server.stop().await;
}

// ── Tool-specific edge cases ─────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn screenshot_annotations_notes_and_caps() {
    let (server, _recorded) = start_fixture().await;
    let mut client = McpClient::spawn(server.port, TEST_SECRET);
    client.initialize();

    // No annotations + annotate default true -> explicit note.
    let none = client.call_tool("browser_screenshot", json!({ "selector": "none" }));
    assert!(text_of(&none).contains("No visible annotated elements."));
    assert_eq!(image_of(&none)["mimeType"], "image/jpeg");

    // >200 annotations are capped with an omitted-count line, plus notes.
    let many = client.call_tool("browser_screenshot", json!({ "selector": "many" }));
    let text = text_of(&many);
    assert!(text.contains("Annotations:"), "text: {text}");
    assert!(text.contains("[1] @e1 button \"Item 1\""), "text: {text}");
    assert!(
        text.contains("[200] @e200 button \"Item 200\""),
        "text: {text}"
    );
    assert!(
        text.contains("... 5 more annotations omitted"),
        "text: {text}"
    );
    assert!(
        text.contains("Note: pinned annotations are approximate"),
        "text: {text}"
    );

    let (code, _stderr) = client.finish();
    assert_eq!(code, 0);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_empty_truncated_outline_and_80k_clip() {
    let (server, recorded) = start_fixture().await;
    let mut client = McpClient::spawn(server.port, TEST_SECRET);
    client.initialize();

    let empty = client.tool_text(
        "browser_read",
        json!({ "url": "https://mcp.example/empty" }),
    );
    assert!(
        empty.starts_with(
            "(empty document: the reader extracted no text; the page may still be loading or may not \
             expose readable content)"
        ),
        "empty: {empty}"
    );

    let truncated = client.tool_text(
        "browser_read",
        json!({ "url": "https://mcp.example/truncated" }),
    );
    assert!(
        truncated.ends_with("[content truncated by reader]"),
        "truncated: {truncated}"
    );

    // 95k reader payload is clipped at 80k with the standard note.
    let huge = client.tool_text("browser_read", json!({ "url": "https://mcp.example/huge" }));
    assert!(
        huge.ends_with("[truncated at 80000 characters for context efficiency]"),
        "huge tail: {}",
        &huge[huge.len().saturating_sub(120)..]
    );
    assert!(huge.len() < 81_000, "huge len: {}", huge.len());

    // outline=true keeps `outline` and sends the flag.
    let outline = client.tool_text(
        "browser_read",
        json!({ "url": "https://mcp.example", "outline": true }),
    );
    assert_eq!(outline, "# Heading\n## Subheading");
    assert_eq!(last_command(&recorded, "read")["outline"], json!(true));

    let (code, _stderr) = client.finish();
    assert_eq!(code, 0);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eval_diff_empty_renders_no_changes() {
    let (server, reported) = start_fixture().await;
    server.extension.set_handler(move |message| {
        reported
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(message.clone());
        FakeReply::Ok(json!({ "ok": true, "diff": "" }))
    });
    let mut client = McpClient::spawn(server.port, TEST_SECRET);
    client.initialize();

    let diff = client.tool_text(
        "browser_eval_and_network",
        json!({ "action": "diff_snapshot" }),
    );
    assert_eq!(diff, "No snapshot changes.");

    let (code, _stderr) = client.finish();
    assert_eq!(code, 0);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tool_argument_validation_and_action_errors() {
    let (server, _recorded) = start_fixture().await;
    let mut client = McpClient::spawn(server.port, TEST_SECRET);
    client.initialize();

    // SDK-level schema validation is reported verbatim (no `Error: ` prefix).
    let missing = client.tool_error("browser_click", json!({}));
    assert!(
        missing.starts_with("Input validation error: Invalid arguments for tool browser_click:"),
        "missing: {missing}"
    );
    assert!(missing.contains("target"), "missing: {missing}");

    let wrong_type = client.tool_error("browser_click", json!({ "target": 5 }));
    assert!(
        wrong_type.starts_with("Input validation error: Invalid arguments for tool browser_click:"),
        "wrong type: {wrong_type}"
    );

    // Handler-level validation errors are prefixed with `Error: ` (legacy wrap()).
    let navigate = client.tool_error("browser_navigate", json!({ "action": "goto" }));
    assert_eq!(navigate, "Error: browser_navigate(goto) requires url");

    let select = client.tool_error(
        "browser_select_or_check",
        json!({ "action": "select", "target": "@e1" }),
    );
    assert_eq!(
        select,
        "Error: browser_select_or_check(select) requires value"
    );

    let hover = client.tool_error("browser_select_or_check", json!({ "action": "hover" }));
    assert_eq!(
        hover,
        "Error: browser_select_or_check(hover) requires target"
    );

    let switch = client.tool_error("browser_tabs", json!({ "action": "switch" }));
    assert_eq!(switch, "Error: browser_tabs(switch) requires tabId");

    let evaluate = client.tool_error("browser_eval_and_network", json!({ "action": "evaluate" }));
    assert_eq!(evaluate, "Error: evaluate requires expression or code");

    let route = client.tool_error("browser_eval_and_network", json!({ "action": "route" }));
    assert_eq!(route, "Error: route requires urlPattern");

    // The server survives every rejected call.
    let ping = client.request("ping", json!({}));
    assert_eq!(ping["result"], json!({}));

    let violations = client.stdout_violations();
    let (code, _stderr) = client.finish();
    assert!(violations.is_empty(), "non-JSON-RPC stdout: {violations:?}");
    assert_eq!(code, 0);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn batch_check_partial_failure_reports_counts_and_failures() {
    let (server, _recorded) = start_fixture().await;
    // Fail the middle target so the batch reports "2 of 3 ... failed".
    server.extension.set_handler(|message| {
        let cmd = message.get("cmd").cloned().unwrap_or(Value::Null);
        if cmd.get("selector") == Some(&json!("@e191")) {
            return FakeReply::Err("element no longer exists".to_string());
        }
        FakeReply::Ok(
            json!({ "ok": true, "ref": cmd.get("selector").cloned().unwrap_or(Value::Null) }),
        )
    });
    let mut client = McpClient::spawn(server.port, TEST_SECRET);
    client.initialize();

    let text = client.tool_text(
        "browser_select_or_check",
        json!({ "action": "check", "target": ["@e190", "@e191", "@e192"] }),
    );
    assert_eq!(
        text,
        "Checked 2 of 3 elements: @e190, @e192; failed: @e191 (element no longer exists)."
    );

    let (code, _stderr) = client.finish();
    assert_eq!(code, 0);
    server.stop().await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sigint_shuts_down_cleanly_with_exit_code_zero() {
    let port = free_port();
    let mut client = McpClient::spawn(port, TEST_SECRET);
    client.initialize();

    let pid = client.child.id();
    let status = Command::new("kill")
        .args(["-INT", &pid.to_string()])
        .status()
        .expect("run kill -INT");
    assert!(status.success(), "kill -INT failed: {status}");

    let code = client.wait_for_exit();
    assert_eq!(code, 0);
    let deadline = Instant::now() + Duration::from_secs(2);
    while !client.stderr_snapshot().contains("signal received") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        client.stderr_snapshot().contains("signal received"),
        "stderr: {}",
        client.stderr_snapshot()
    );
}

//! Terminal CLI: `secret`, `status`, `snap`, `click`, `fill`, `read`, `--help`,
//! `--version`.
//!
//! This is intentionally a thin layer over the same hub/peer transport the MCP
//! server uses, so `browser-use snap` and a concurrently running MCP server
//! share one browser connection. Ported 1:1 from the legacy `src/cli.ts`;
//! `serve` and every launchd service command are deleted: the menu bar app is
//! the single hub owner.
//!
//! Module dependency rule: this module may import
//! `secret`, `peer`, `format`, `hub` port helpers/constants plus `lib.rs`
//! constants only.

use std::io::Write as _;

use serde_json::{json, Map, Value};

use crate::format;
use crate::hub::{self, HUB_HOST};
use crate::peer::{PeerClient, PeerError, PeerErrorKind};
use crate::secret;
use crate::{SERVER_NAME, VERSION};

/// Everything in the legacy help text except the launchd service commands
/// (`serve`, `install-service`, `uninstall-service`, `restart-service`,
/// `service-status`) and their `--wait` / `--json` / `--port` / `--node` flags.
const HELP_BODY: &str = r#"
Usage:
  browser-use [mcp]                 stdio MCP server (default)
  browser-use secret                print the shared secret + Zen setup line
  browser-use status                show hub, extension, and tab status
  browser-use snap [-i] [-c]        print an accessibility snapshot
  browser-use click <@ref|selector> click an element
  browser-use fill <@ref|selector> <text>
  browser-use read [url]            print page Markdown

Options:
  --tab <id>       target a specific tab
  --selector <s>   scope snap to a selector
  --urls           include link URLs in snap
  --delta          return a snapshot delta
  --double         double-click
  --right          right-click
  --force          bypass actionability checks
  --slow           type character by character (fill)
  --delay <ms>     delay per character (fill --slow)
  --enter          press Enter after filling
  --outline        return only headings (read)
  --llms index|full  read llms.txt / llms-full.txt
  --filter <text>  filter read sections
  -i, --interactive  interactive-only snapshot
  -c, --compact      compact snapshot
  -h, --help         show this help
  -v, --version      show version

Environment:
  ORANGE_PORT     hub port (default 19455)
  ORANGE_SECRET   shared secret (overrides ~/.orange-mcp/secret)
"#;

fn help_text() -> String {
    format!("browser-use {VERSION} — Gecko Orange bridge\n{HELP_BODY}")
}

/// Run the CLI with the process arguments after the binary name. Returns the
/// process exit code: `0` success, `1` runtime/offline/unauthorized failure,
/// `2` unknown command.
pub async fn run(argv: Vec<String>) -> i32 {
    let mut args = argv.into_iter();
    let command = args.next().unwrap_or_else(|| "help".to_string());
    let rest: Vec<String> = args.collect();

    let outcome = match command.as_str() {
        "-h" | "--help" | "help" => {
            write_stdout(&help_text());
            Ok(0)
        }
        "-v" | "--version" | "version" => {
            write_stdout(&format!("{SERVER_NAME} {VERSION}\n"));
            Ok(0)
        }
        "secret" => secret_command(&rest).await,
        "status" => status_command(&rest).await,
        "snap" => snap_command(&rest).await,
        "click" => click_command(&rest).await,
        "fill" => fill_command(&rest).await,
        "read" => read_command(&rest).await,
        // main.rs intercepts `mcp` before the CLI, exactly like the legacy
        // entrypoint; reaching this arm is a bug.
        "mcp" => Err("'browser-use mcp' is handled by the entrypoint; this is a bug".to_string()),
        other => {
            write_stderr(&format!("Unknown command: {other}\n\n{}", help_text()));
            return 2;
        }
    };

    match outcome {
        Ok(code) => code,
        Err(message) => {
            write_stderr(&format!("Error: {message}\n"));
            1
        }
    }
}

// ── Commands ─────────────────────────────────────────────────────────────

async fn secret_command(argv: &[String]) -> Result<i32, String> {
    if argv.iter().any(|arg| arg == "-h" || arg == "--help") {
        write_stdout(&help_text());
        return Ok(0);
    }
    if !argv.is_empty() {
        return Err("secret takes no arguments".to_string());
    }

    let secret = secret::load_secret().await.map_err(|error| error.0)?;
    let source = if secret::secret_from_environment() {
        "ORANGE_SECRET".to_string()
    } else {
        secret::secret_path().display().to_string()
    };
    let quoted = serde_json::to_string(&secret)
        .map_err(|error| format!("Unable to encode secret: {error}"))?;

    let output = [
        format!("Secret: {secret}"),
        format!("Source: {source}"),
        String::new(),
        "Paste this once into Zen (about:debugging -> Gecko Orange -> Inspect -> Console):"
            .to_string(),
        format!("await OrangeBridge.setSecret({quoted})"),
        String::new(),
    ]
    .join("\n");

    write_stdout(&output);
    Ok(0)
}

async fn status_command(argv: &[String]) -> Result<i32, String> {
    let parsed = parse_options(argv)?;
    if parsed.flag("help") {
        write_stdout(&help_text());
        return Ok(0);
    }
    assert_no_positionals(&parsed, "status")?;

    let port = resolve_port()?;
    let secret = secret::load_secret().await.map_err(|error| error.0)?;

    // The legacy CLI connected with a 1.5 s timeout so `status` fails fast when
    // the app is closed; the default lazily-connecting peer timeout is 5 s.
    // On Windows, loopback `connect()` retries SYN packets for ~2 s before
    // returning WSAECONNREFUSED, so allow 3.5 s there to avoid false timeouts.
    #[cfg(windows)]
    const STATUS_CONNECT_TIMEOUT_MS: u64 = 3_500;
    #[cfg(not(windows))]
    const STATUS_CONNECT_TIMEOUT_MS: u64 = 1_500;

    let peer = PeerClient::with_connect_timeout(port, secret, STATUS_CONNECT_TIMEOUT_MS);
    let outcome = peer.status().await;
    peer.close().await;

    match outcome {
        Ok(status) => {
            write_stdout(&format_status(port, &status));
            Ok(0)
        }
        Err(error) => {
            // Exactly like the legacy CLI, the actionable offline /
            // unauthorized report goes to stdout and exits 1.
            write_stdout(&format_status_error(port, &error));
            Ok(1)
        }
    }
}

async fn snap_command(argv: &[String]) -> Result<i32, String> {
    let parsed = parse_options(argv)?;
    if parsed.flag("help") {
        write_stdout(&help_text());
        return Ok(0);
    }
    assert_no_positionals(&parsed, "snap")?;

    let tab_id = parsed.tab_id()?;
    let mut command = Map::new();
    command.insert("action".to_string(), json!("snapshot"));
    command.insert("interactive".to_string(), json!(parsed.flag("interactive")));
    command.insert("compact".to_string(), json!(parsed.flag("compact")));
    command.insert("delta".to_string(), json!(parsed.flag("delta")));
    command.insert("urls".to_string(), json!(parsed.flag("urls")));
    if let Some(selector) = parsed.string_flag("selector") {
        command.insert("selector".to_string(), Value::String(selector.to_string()));
    }

    let result = peer_command(Value::Object(command), tab_id).await?;
    let snapshot = into_object(result, "snapshot")?;
    write_stdout(&format!("{}\n", format::format_snapshot(&snapshot)));
    Ok(0)
}

async fn click_command(argv: &[String]) -> Result<i32, String> {
    let parsed = parse_options(argv)?;
    if parsed.flag("help") {
        write_stdout(&help_text());
        return Ok(0);
    }

    let Some(target) = parsed.positionals.first().cloned() else {
        return Err("Usage: browser-use click <@ref|selector>".to_string());
    };
    if parsed.positionals.len() > 1 {
        return Err("click accepts exactly one target".to_string());
    }

    let action = if parsed.flag("right") {
        "contextmenu"
    } else if parsed.flag("double") {
        "dblclick"
    } else {
        "click"
    };
    let command = json!({
        "action": action,
        "selector": target,
        "force": parsed.flag("force"),
    });
    let result = peer_command(command, parsed.tab_id()?).await?;
    let element = into_object(result, "click")?;

    let verb = match action {
        "contextmenu" => "Right-clicked",
        "dblclick" => "Double-clicked",
        _ => "Clicked",
    };
    write_stdout(&format!(
        "{verb} {}.\n",
        format::describe_element(&element, Some(&target))
    ));
    Ok(0)
}

async fn fill_command(argv: &[String]) -> Result<i32, String> {
    let parsed = parse_options(argv)?;
    if parsed.flag("help") {
        write_stdout(&help_text());
        return Ok(0);
    }

    let Some(target) = parsed.positionals.first().cloned() else {
        return Err("Usage: browser-use fill <@ref|selector> <text>".to_string());
    };
    if parsed.positionals.len() < 2 {
        return Err("fill requires text".to_string());
    }

    let value = parsed.positionals[1..].join(" ");
    let slow = parsed.flag("slow");
    let delay = match parsed.string_flag("delay") {
        // Legacy only validates `--delay` when `--slow` is set.
        Some(raw) if slow => Some(number_from_flag(raw, "delay")?),
        _ => None,
    };
    let press_enter = parsed.flag("enter");
    let tab_id = parsed.tab_id()?;

    let peer = open_peer().await?;
    let outcome: Result<Value, String> =
        fill_with_peer(&peer, &target, &value, slow, delay, press_enter, tab_id).await;
    peer.close().await;

    let filled = into_object(outcome?, "fill")?;
    let quoted =
        serde_json::to_string(&value).map_err(|error| format!("Unable to encode text: {error}"))?;
    let verb = if slow { "Typed" } else { "Filled" };
    let mut text = format!(
        "{verb} {} with {quoted}.",
        format::describe_element(&filled, Some(&target))
    );
    if press_enter {
        text.push_str(" Pressed Enter.");
    }
    write_stdout(&format!("{text}\n"));
    Ok(0)
}

async fn read_command(argv: &[String]) -> Result<i32, String> {
    let parsed = parse_options(argv)?;
    if parsed.flag("help") {
        write_stdout(&help_text());
        return Ok(0);
    }
    if parsed.positionals.len() > 1 {
        return Err("read accepts at most one URL".to_string());
    }

    let requested_url = parsed.positionals.first().cloned();
    let outline = parsed.flag("outline");
    let llms = parsed.string_flag("llms").map(str::to_string);
    if let Some(llms) = &llms {
        if llms != "index" && llms != "full" {
            return Err("--llms must be 'index' or 'full'".to_string());
        }
    }
    let filter = parsed.string_flag("filter").map(str::to_string);
    let tab_id = parsed.tab_id()?;

    let peer = open_peer().await?;
    let outcome = read_with_peer(&peer, requested_url, outline, llms, filter, tab_id).await;
    peer.close().await;

    let result = into_object(outcome?, "read")?;

    let mut body = String::new();
    let mut body_set = false;
    if outline {
        if let Some(text) = result.get("outline").and_then(Value::as_str) {
            body = text.to_string();
            body_set = true;
        }
    }
    if !body_set {
        if let Some(text) = result.get("markdown").and_then(Value::as_str) {
            body = text.to_string();
        } else if let Some(text) = result.get("content").and_then(Value::as_str) {
            body = text.to_string();
        }
    }

    if body.trim().is_empty() {
        body = "(empty document: the reader extracted no text; the page may still be loading \
                or may not expose readable content)"
            .to_string();
    }
    if matches!(result.get("truncated"), Some(Value::Bool(true))) {
        body.push_str("\n\n[content truncated by reader]");
    }
    if !body.ends_with('\n') {
        body.push('\n');
    }
    write_stdout(&body);
    Ok(0)
}

// ── Command plumbing ─────────────────────────────────────────────────────

/// Resolve the hub port from `ORANGE_PORT` (default 19455). Invalid values are
/// a hard `Error: ORANGE_PORT must be …` failure, exactly like the legacy CLI.
fn resolve_port() -> Result<u16, String> {
    let raw = std::env::var("ORANGE_PORT").ok();
    hub::try_resolve_port(raw.as_deref())
}

async fn open_peer() -> Result<PeerClient, String> {
    let port = resolve_port()?;
    let secret = secret::load_secret().await.map_err(|error| error.0)?;
    Ok(PeerClient::new(port, secret))
}

/// Connect, run exactly one command, and close again (the legacy `withBridge`).
async fn peer_command(command: Value, tab_id: Option<i64>) -> Result<Value, String> {
    let peer = open_peer().await?;
    let outcome = peer.command(command, tab_id, 0).await;
    peer.close().await;
    outcome.map_err(|error| error.message)
}

async fn fill_with_peer(
    peer: &PeerClient,
    target: &str,
    value: &str,
    slow: bool,
    delay: Option<i64>,
    press_enter: bool,
    tab_id: Option<i64>,
) -> Result<Value, String> {
    let mut command = Map::new();
    command.insert(
        "action".to_string(),
        Value::String(if slow { "type" } else { "fill" }.to_string()),
    );
    command.insert("selector".to_string(), Value::String(target.to_string()));
    command.insert("value".to_string(), Value::String(value.to_string()));
    if let Some(delay) = delay {
        command.insert("delay".to_string(), json!(delay));
    }

    let filled = peer
        .command(Value::Object(command), tab_id, 0)
        .await
        .map_err(|error| error.message)?;

    if press_enter {
        peer.command(
            json!({ "action": "press", "selector": target, "key": "Enter" }),
            tab_id,
            0,
        )
        .await
        .map_err(|error| error.message)?;
    }

    Ok(filled)
}

async fn read_with_peer(
    peer: &PeerClient,
    requested_url: Option<String>,
    outline: bool,
    llms: Option<String>,
    filter: Option<String>,
    tab_id: Option<i64>,
) -> Result<Value, String> {
    let mut url = requested_url;

    // Legacy `if (!url && llms)`: an empty URL is falsy just like a missing one.
    let has_url = url.as_deref().is_some_and(|value| !value.is_empty());
    if !has_url && llms.is_some() {
        let active = peer
            .command(json!({ "action": "url" }), tab_id, 0)
            .await
            .map_err(|error| error.message)?;
        let active = into_object(active, "url")?;
        url = active
            .get("url")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if url.is_none() {
            return Err(
                "Could not determine the active tab URL; pass a URL explicitly".to_string(),
            );
        }
    }

    let mut command = Map::new();
    command.insert("action".to_string(), json!("read"));
    if let Some(url) = url {
        command.insert("url".to_string(), Value::String(url));
    }
    command.insert("outline".to_string(), json!(outline));
    if let Some(llms) = llms {
        command.insert("llms".to_string(), Value::String(llms));
    }
    if let Some(filter) = filter {
        command.insert("filter".to_string(), Value::String(filter));
    }

    peer.command(Value::Object(command), tab_id, 0)
        .await
        .map_err(|error| error.message)
}

fn into_object(value: Value, label: &str) -> Result<Value, String> {
    if value.is_object() {
        Ok(value)
    } else {
        Err(format!("Extension returned an invalid {label} response"))
    }
}

// ── Support formatters ───────────────────────────────────────────────────

fn format_status(port: u16, status: &Value) -> String {
    let pid = json_text(status.get("pid"));
    let uptime = format_status_duration(status.get("uptimeMs").and_then(Value::as_f64));
    let peers = json_text(status.get("peers"));

    let extension_connected = status
        .get("extensionConnected")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let tabs_error = status
        .get("tabsError")
        .and_then(Value::as_str)
        .unwrap_or("");

    let mut lines = vec![
        format!("Hub: running at ws://{HUB_HOST}:{port} (pid {pid}, up {uptime})"),
        format!("Extension: {}", format_extension(status)),
        format!("Peers: {peers}"),
    ];

    if !tabs_error.is_empty() {
        lines.push(format!("Tabs: unavailable ({tabs_error})"));
    } else if extension_connected && status.get("tabs").map(Value::is_array).unwrap_or(false) {
        lines.push(String::new());
        lines.push(format::format_tabs(status));
    } else {
        lines.push("Tabs: unavailable until the extension connects".to_string());
    }

    append_newline(lines.join("\n"))
}

fn format_status_error(port: u16, error: &PeerError) -> String {
    let lines = match error.kind {
        PeerErrorKind::Offline => vec![
            format!("Hub: not running at ws://{HUB_HOST}:{port}"),
            "Extension: unknown".to_string(),
            String::new(),
            // The peer's actionable message already names the URL and the fix
            // ("Browser Use is not running at ws://… Launch Browser Use.app first.").
            error.message.clone(),
        ],
        PeerErrorKind::Unauthorized => vec![
            format!("Hub: running at ws://{HUB_HOST}:{port}, but it rejected the shared secret"),
            "Extension: unknown".to_string(),
            String::new(),
            "Run 'browser-use secret' and paste the OrangeBridge.setSecret(...) line into Zen \
             again."
                .to_string(),
        ],
        _ => vec![
            format!("Hub: port {port} is not a usable browser-use hub"),
            format!("Reason: {}", error.message),
            String::new(),
            "Stop the other service or set ORANGE_PORT.".to_string(),
        ],
    };

    append_newline(lines.join("\n"))
}

fn format_extension(status: &Value) -> String {
    let connected = status
        .get("extensionConnected")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !connected {
        return "not connected".to_string();
    }

    // Legacy: `connected (${extension.client} ${extension.version})`. The hub
    // crate may report `client` or, additively, `browser`; tolerate both.
    let Some(extension) = status.get("extension").and_then(Value::as_object) else {
        return "connected".to_string();
    };
    let client = extension
        .get("client")
        .or_else(|| extension.get("browser"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let version = extension
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("");

    match (client.is_empty(), version.is_empty()) {
        (true, true) => "connected".to_string(),
        (false, true) => format!("connected ({client})"),
        (true, false) => format!("connected ({version})"),
        (false, false) => format!("connected ({client} {version})"),
    }
}

/// Legacy `formatDuration`: `0s`, `12s`, `3m 4s`, `2h 5m`.
fn format_status_duration(ms: Option<f64>) -> String {
    let Some(ms) = ms else {
        return "0s".to_string();
    };
    if !ms.is_finite() || ms < 0.0 {
        return "0s".to_string();
    }
    if ms < 1_000.0 {
        return "0s".to_string();
    }

    let seconds = (ms / 1_000.0).floor();
    if seconds < 60.0 {
        return format!("{}s", seconds as u64);
    }
    let minutes = (seconds / 60.0).floor();
    if minutes < 60.0 {
        return format!("{}m {}s", minutes as u64, (seconds % 60.0) as u64);
    }
    let hours = (minutes / 60.0).floor();
    format!("{}h {}m", hours as u64, (minutes % 60.0) as u64)
}

/// `value ?? "?"` rendering, matching the legacy CLI's template strings.
fn json_text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "?".to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

fn append_newline(mut text: String) -> String {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn write_stdout(text: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

fn write_stderr(text: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(text.as_bytes());
    let _ = stderr.flush();
}

// ── Legacy flag parser ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum FlagValue {
    Bool(bool),
    Text(String),
}

#[derive(Debug, Default)]
struct ParsedArgs {
    positionals: Vec<String>,
    flags: std::collections::HashMap<String, FlagValue>,
}

impl ParsedArgs {
    fn flag(&self, name: &str) -> bool {
        matches!(self.flags.get(name), Some(FlagValue::Bool(true)))
    }

    fn string_flag(&self, name: &str) -> Option<&str> {
        match self.flags.get(name) {
            Some(FlagValue::Text(value)) => Some(value),
            _ => None,
        }
    }

    fn tab_id(&self) -> Result<Option<i64>, String> {
        match self.string_flag("tab") {
            None => Ok(None),
            Some(raw) => Ok(Some(number_from_flag(raw, "tab")?)),
        }
    }
}

/// Legacy `BOOLEAN_FLAGS` map (aliases included), minus `--wait` (serve) and
/// `--json` (service-status).
fn boolean_flag_name(raw: &str) -> Option<&'static str> {
    Some(match raw {
        "i" | "interactive" => "interactive",
        "c" | "compact" => "compact",
        "urls" => "urls",
        "delta" => "delta",
        "double" => "double",
        "right" | "right-click" | "rightClick" => "right",
        "force" => "force",
        "slow" | "slow-type" | "slowType" => "slow",
        "enter" | "press-enter" | "pressEnter" => "enter",
        "outline" => "outline",
        "h" | "help" => "help",
        _ => return None,
    })
}

/// Legacy `VALUE_FLAGS` map (aliases included), minus `--port` / `--node`
/// (service-only).
fn value_flag_name(raw: &str) -> Option<&'static str> {
    Some(match raw {
        "tab" | "tab-id" | "tabId" => "tab",
        "selector" => "selector",
        "delay" | "delay-ms" | "delayMs" => "delay",
        "llms" => "llms",
        "filter" => "filter",
        _ => return None,
    })
}

/// Port of the legacy `parseOptions` loop: `--flag`, `--flag=value`,
/// `--opt value`, `-h`, `--` terminator, and the exact legacy error strings.
fn parse_options(argv: &[String]) -> Result<ParsedArgs, String> {
    let mut parsed = ParsedArgs::default();
    let mut index = 0;

    while index < argv.len() {
        let arg = &argv[index];

        if arg == "--" {
            parsed.positionals.extend_from_slice(&argv[index + 1..]);
            break;
        }

        if arg == "-h" || arg == "--help" {
            parsed
                .flags
                .insert("help".to_string(), FlagValue::Bool(true));
            index += 1;
            continue;
        }

        if let Some(without_prefix) = arg.strip_prefix("--") {
            let (raw_name, inline_value) = match without_prefix.find('=') {
                Some(equals) => (
                    &without_prefix[..equals],
                    Some(without_prefix[equals + 1..].to_string()),
                ),
                None => (without_prefix, None),
            };

            if let Some(name) = boolean_flag_name(raw_name) {
                if inline_value.is_some() {
                    return Err(format!("--{raw_name} does not take a value"));
                }
                parsed.flags.insert(name.to_string(), FlagValue::Bool(true));
                index += 1;
                continue;
            }

            if let Some(name) = value_flag_name(raw_name) {
                let value = match &inline_value {
                    Some(value) => value.clone(),
                    None => match argv.get(index + 1) {
                        Some(next) if next != "--" => next.clone(),
                        _ => return Err(format!("--{raw_name} requires a value")),
                    },
                };
                parsed
                    .flags
                    .insert(name.to_string(), FlagValue::Text(value));
                index += if inline_value.is_none() { 2 } else { 1 };
                continue;
            }

            return Err(format!("Unknown option: --{raw_name}"));
        }

        if arg.starts_with('-') && arg != "-" {
            let name = &arg[1..];
            if let Some(name) = boolean_flag_name(name) {
                parsed.flags.insert(name.to_string(), FlagValue::Bool(true));
                index += 1;
                continue;
            }
            return Err(format!("Unknown option: {arg}"));
        }

        parsed.positionals.push(arg.clone());
        index += 1;
    }

    Ok(parsed)
}

fn assert_no_positionals(parsed: &ParsedArgs, command: &str) -> Result<(), String> {
    if parsed.positionals.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{command} does not take positional arguments: {}",
        parsed.positionals.join(" ")
    ))
}

/// Legacy `numberFromFlag`: finite, non-negative, floored.
fn number_from_flag(value: &str, name: &str) -> Result<i64, String> {
    let trimmed = value.trim();
    // JavaScript `Number("") === 0`.
    let number = if trimmed.is_empty() {
        0.0
    } else {
        trimmed
            .parse::<f64>()
            .map_err(|_| format!("--{name} must be a non-negative number"))?
    };

    if !number.is_finite() || number < 0.0 {
        return Err(format!("--{name} must be a non-negative number"));
    }
    Ok(number.floor() as i64)
}

//! CLI integration tests: spawn the real `browser-use` binary against a
//! real in-process hub plus a fake Gecko Orange extension, and assert the
//! legacy flags, wire payloads, output formatting and exit codes.
//!
//! Ported from the legacy `test/cli.test.ts`. Every hub binds an ephemeral
//! port; the machine-wide legacy hub on 19455 is never touched (the child
//! process gets `ORANGE_PORT` / `ORANGE_SECRET` explicitly).

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{
    start_hub, start_hub_with_secret, FakeReply, TestServer, APP_LAUNCH_NAME, TEST_SECRET,
};
use serde_json::{json, Value};

// ── Harness ──────────────────────────────────────────────────────────────

#[derive(Debug)]
struct CliResult {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

impl CliResult {
    fn status(&self) -> i32 {
        self.status.expect("browser-use was terminated by a signal")
    }

    fn assert_status(&self, expected: i32, label: &str) {
        assert_eq!(
            self.status(),
            expected,
            "{label}: unexpected exit code\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.stdout,
            self.stderr,
        );
    }

    fn assert_stdout_empty(&self) {
        assert!(
            self.stdout.is_empty(),
            "expected empty stdout, got {:?}",
            self.stdout
        );
    }

    fn assert_stderr_empty(&self) {
        assert!(
            self.stderr.is_empty(),
            "expected empty stderr, got {:?}",
            self.stderr
        );
    }
}

async fn run_cli_with_secret(port: u16, secret: &str, args: &[&str]) -> CliResult {
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    let secret = secret.to_string();
    let label = format!("{args:?}");

    let task = tokio::task::spawn_blocking(move || {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_browser-use"))
            .args(&args)
            .env("ORANGE_PORT", port.to_string())
            .env("ORANGE_SECRET", &secret)
            .output()
            .expect("failed to spawn the browser-use binary");
        CliResult {
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    });

    match tokio::time::timeout(Duration::from_secs(20), task).await {
        Ok(joined) => joined.expect("browser-use CLI task panicked"),
        Err(_) => panic!("browser-use CLI timed out for args {label}"),
    }
}

async fn run_cli(port: u16, args: &[&str]) -> CliResult {
    run_cli_with_secret(port, TEST_SECRET, args).await
}

// ── Command recording ────────────────────────────────────────────────────

type Seen = Arc<Mutex<Vec<Value>>>;

fn seen() -> Seen {
    Arc::new(Mutex::new(Vec::new()))
}

/// Wrap a fake-extension responder so every incoming `cmd` frame is recorded.
fn recording_handler(
    seen: Seen,
    responder: impl Fn(&Value) -> FakeReply + Send + Sync + 'static,
) -> impl Fn(&Value) -> FakeReply + Send + Sync + 'static {
    move |message| {
        seen.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(message.clone());
        responder(message)
    }
}

fn sent_commands(seen: &Seen) -> Vec<Value> {
    seen.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

fn find_command(seen: &Seen, action: &str) -> Value {
    sent_commands(seen)
        .into_iter()
        .find(|message| message["cmd"]["action"] == json!(action))
        .unwrap_or_else(|| panic!("extension never received a {action:?} command"))
}

fn clear_commands(seen: &Seen) {
    seen.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

/// Standard element result used by click/fill assertions.
fn element_result() -> Value {
    json!({ "ok": true, "ref": "e1", "role": "button", "name": "Go" })
}

// ── Static surface: help / version / unknown command ─────────────────────

#[tokio::test]
async fn help_version_and_unknown_command_contract() {
    let port = common::free_port();

    let help = run_cli(port, &["--help"]).await;
    help.assert_status(0, "--help");
    help.assert_stderr_empty();
    assert!(
        help.stdout
            .starts_with("browser-use 0.1.0 — Gecko Orange bridge\n\nUsage:\n"),
        "unexpected help header: {:?}",
        help.stdout
    );
    for expected in [
        "browser-use [mcp]",
        "browser-use secret",
        "browser-use status",
        "browser-use snap [-i] [-c]",
        "browser-use click <@ref|selector>",
        "browser-use fill <@ref|selector> <text>",
        "browser-use read [url]",
        "--selector <s>",
        "--tab <id>",
        "--llms index|full",
        "-h, --help",
        "-v, --version",
        "ORANGE_PORT",
        "ORANGE_SECRET",
    ] {
        assert!(
            help.stdout.contains(expected),
            "help text is missing {expected:?}:\n{}",
            help.stdout
        );
    }
    // Guardrail #2: the launchd service surface must stay deleted.
    for removed in [
        "browser-use serve",
        "install-service",
        "uninstall-service",
        "restart-service",
        "service-status",
        "--wait",
        "--json",
        "--node",
        "--port",
    ] {
        assert!(
            !help.stdout.contains(removed),
            "help text still exposes the removed service surface {removed:?}:\n{}",
            help.stdout
        );
    }

    for alias in ["help", "-h"] {
        let output = run_cli(port, &[alias]).await;
        output.assert_status(0, alias);
        assert_eq!(
            output.stdout, help.stdout,
            "{alias} must print the same help"
        );
    }

    let version = run_cli(port, &["--version"]).await;
    version.assert_status(0, "--version");
    version.assert_stderr_empty();
    assert_eq!(version.stdout, "browser-use 0.1.0\n");
    for alias in ["-v", "version"] {
        let output = run_cli(port, &[alias]).await;
        output.assert_status(0, alias);
        assert_eq!(output.stdout, "browser-use 0.1.0\n");
    }

    let unknown = run_cli(port, &["frobnicate"]).await;
    unknown.assert_status(2, "unknown command");
    unknown.assert_stdout_empty();
    assert!(
        unknown
            .stderr
            .starts_with("Unknown command: frobnicate\n\n"),
        "unexpected stderr: {:?}",
        unknown.stderr
    );
    assert!(unknown.stderr.contains("Usage:"));
}

#[tokio::test]
async fn secret_prints_the_legacy_block_and_rejects_arguments() {
    let port = common::free_port();

    let output = run_cli(port, &["secret"]).await;
    output.assert_status(0, "secret");
    output.assert_stderr_empty();
    let expected = format!(
        "Secret: {TEST_SECRET}\n\
         Source: ORANGE_SECRET\n\
         \n\
         Paste this once into Zen (about:debugging -> Gecko Orange -> Inspect -> Console):\n\
         await OrangeBridge.setSecret(\"{TEST_SECRET}\")\n"
    );
    assert_eq!(output.stdout, expected);

    let extra = run_cli(port, &["secret", "extra"]).await;
    extra.assert_status(1, "secret extra");
    extra.assert_stdout_empty();
    assert_eq!(extra.stderr, "Error: secret takes no arguments\n");
}

// ── status ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn status_offline_exits_one_with_the_peer_message() {
    let port = common::free_port();

    let output = run_cli(port, &["status"]).await;
    output.assert_status(1, "status offline");
    output.assert_stderr_empty();
    assert!(
        output
            .stdout
            .contains(&format!("Hub: not running at ws://127.0.0.1:{port}")),
        "missing offline hub line: {:?}",
        output.stdout
    );
    assert!(output.stdout.contains("Extension: unknown"));
    assert!(
        output.stdout.contains(&format!(
            "Browser Use is not running at ws://127.0.0.1:{port}. Launch {APP_LAUNCH_NAME} first."
        )),
        "missing actionable peer message: {:?}",
        output.stdout
    );

    // Commands that need the peer surface the same actionable message as an
    // `Error:` on stderr instead of a status report.
    let snap = run_cli(port, &["snap"]).await;
    snap.assert_status(1, "snap offline");
    snap.assert_stdout_empty();
    assert_eq!(
        snap.stderr,
        format!(
            "Error: Browser Use is not running at ws://127.0.0.1:{port}. Launch {APP_LAUNCH_NAME} first.\n"
        )
    );
}

#[tokio::test]
async fn status_unauthorized_reports_the_secret_mismatch() {
    // Hub expects a different secret than the CLI's ORANGE_SECRET.
    let (hub, port) = start_hub_with_secret(&format!("{TEST_SECRET}00")).await;

    let output = run_cli(port, &["status"]).await;
    hub.stop().await;

    output.assert_status(1, "status unauthorized");
    output.assert_stderr_empty();
    assert!(
        output.stdout.contains(&format!(
            "Hub: running at ws://127.0.0.1:{port}, but it rejected the shared secret"
        )),
        "missing unauthorized hub line: {:?}",
        output.stdout
    );
    assert!(output.stdout.contains(
        "Run 'browser-use secret' and paste the OrangeBridge.setSecret(...) line into Zen again."
    ));
}

#[tokio::test]
async fn status_online_reports_hub_extension_peers_and_tabs() {
    let server = TestServer::start().await;
    let port = server.port;

    let output = run_cli(port, &["status"]).await;
    server.stop().await;

    output.assert_status(0, "status online");
    output.assert_stderr_empty();
    assert!(
        output
            .stdout
            .contains(&format!("Hub: running at ws://127.0.0.1:{port} (pid ")),
        "missing hub line: {:?}",
        output.stdout
    );
    assert!(output.stdout.contains(", up "));
    assert!(
        output
            .stdout
            .contains("Extension: connected (gecko-orange 3.0.0)\n"),
        "missing extension line: {:?}",
        output.stdout
    );
    assert!(output.stdout.contains("Peers: 1\n"));
    assert!(
        !output.stdout.contains("Service:"),
        "the launchd Service row must stay deleted: {:?}",
        output.stdout
    );
    assert!(
        output.stdout.contains(
            "\nTabs (2):\n* [42] Alpha — https://alpha.example\n  [43] Beta — https://beta.example\n"
        ),
        "missing formatted tab list: {:?}",
        output.stdout
    );
}

#[tokio::test]
async fn status_without_extension_reports_unavailable_tabs() {
    let (hub, port) = start_hub().await;

    let output = run_cli(port, &["status"]).await;
    hub.stop().await;

    output.assert_status(0, "status without extension");
    output.assert_stderr_empty();
    assert!(output.stdout.contains("Extension: not connected\n"));
    assert!(output
        .stdout
        .contains("Tabs: unavailable until the extension connects\n"));
}

#[tokio::test]
async fn status_reports_a_tab_list_error_from_the_extension() {
    let server = TestServer::start().await;
    server.extension.set_handler(|message| {
        if message["cmd"]["action"] == json!("tab_list") {
            FakeReply::Err("tab_list boom".to_string())
        } else {
            FakeReply::Ok(json!({ "ok": true }))
        }
    });
    let port = server.port;

    let output = run_cli(port, &["status"]).await;
    server.stop().await;

    output.assert_status(0, "status tabs error");
    output.assert_stderr_empty();
    assert!(
        output.stdout.contains("Tabs: unavailable (tab_list boom)"),
        "missing tabs error line: {:?}",
        output.stdout
    );
}

// ── snap / click / fill ──────────────────────────────────────────────────

#[tokio::test]
async fn snap_sends_the_legacy_flags_and_formats_the_snapshot() {
    let server = TestServer::start().await;
    let seen = seen();
    server.extension.set_handler(recording_handler(
        Arc::clone(&seen),
        |message| match message["cmd"]["action"].as_str() {
            Some("snapshot") => FakeReply::Ok(json!({
                "ok": true,
                "kind": "full",
                "title": "CLI Fixture",
                "url": "https://cli.example",
                "tabId": 9,
                "revision": 4,
                "tree": "- button \"Go\" [ref=e1]",
            })),
            _ => FakeReply::Ok(json!({ "ok": true })),
        },
    ));
    let port = server.port;

    let output = run_cli(port, &["snap", "-i", "-c", "--tab", "9"]).await;
    output.assert_status(0, "snap");
    output.assert_stderr_empty();
    assert_eq!(
        output.stdout,
        "Title: CLI Fixture | URL: https://cli.example | Tab: 9 | Revision: 4\n\n\
         - button \"Go\" [ref=e1]\n"
    );

    let snapshot = find_command(&seen, "snapshot");
    assert_eq!(snapshot["tabId"], json!(9));
    assert_eq!(snapshot["cmd"]["interactive"], json!(true));
    assert_eq!(snapshot["cmd"]["compact"], json!(true));
    assert_eq!(snapshot["cmd"]["delta"], json!(false));
    assert_eq!(snapshot["cmd"]["urls"], json!(false));
    assert!(snapshot["cmd"].get("selector").is_none());

    clear_commands(&seen);
    let output = run_cli(port, &["snap", "--selector", "#main", "--delta", "--urls"]).await;
    server.stop().await;
    output.assert_status(0, "snap selector");
    assert_eq!(
        output.stdout,
        "Title: CLI Fixture | URL: https://cli.example | Tab: 9 | Revision: 4\n\n\
         - button \"Go\" [ref=e1]\n"
    );

    let snapshot = find_command(&seen, "snapshot");
    assert_eq!(snapshot["cmd"]["selector"], json!("#main"));
    assert_eq!(snapshot["cmd"]["interactive"], json!(false));
    assert_eq!(snapshot["cmd"]["delta"], json!(true));
    assert_eq!(snapshot["cmd"]["urls"], json!(true));
}

#[tokio::test]
async fn click_maps_variants_and_describes_the_element() {
    let server = TestServer::start().await;
    let seen = seen();
    server.extension.set_handler(recording_handler(
        Arc::clone(&seen),
        |message| match message["cmd"]["action"].as_str() {
            Some("click") | Some("dblclick") | Some("contextmenu") => {
                FakeReply::Ok(element_result())
            }
            _ => FakeReply::Ok(json!({ "ok": true })),
        },
    ));
    let port = server.port;

    let output = run_cli(port, &["click", "@e1"]).await;
    output.assert_status(0, "click");
    output.assert_stderr_empty();
    assert_eq!(output.stdout, "Clicked @e1 button \"Go\".\n");
    let click = find_command(&seen, "click");
    assert_eq!(click["cmd"]["selector"], json!("@e1"));
    assert_eq!(click["cmd"]["force"], json!(false));

    clear_commands(&seen);
    let output = run_cli(port, &["click", "#go", "--double", "--force"]).await;
    output.assert_status(0, "double click");
    assert_eq!(output.stdout, "Double-clicked @e1 button \"Go\".\n");
    let dblclick = find_command(&seen, "dblclick");
    assert_eq!(dblclick["cmd"]["selector"], json!("#go"));
    assert_eq!(dblclick["cmd"]["force"], json!(true));

    clear_commands(&seen);
    let output = run_cli(port, &["click", "#go", "--right"]).await;
    server.stop().await;
    output.assert_status(0, "right click");
    assert_eq!(output.stdout, "Right-clicked @e1 button \"Go\".\n");
    find_command(&seen, "contextmenu");
}

#[tokio::test]
async fn fill_sends_text_enter_and_slow_delay() {
    let server = TestServer::start().await;
    let seen = seen();
    server.extension.set_handler(recording_handler(
        Arc::clone(&seen),
        |message| match message["cmd"]["action"].as_str() {
            Some("fill") | Some("type") | Some("press") => FakeReply::Ok(json!({
                "ok": true,
                "ref": "e1",
                "role": "textbox",
                "name": "Email",
            })),
            _ => FakeReply::Ok(json!({ "ok": true })),
        },
    ));
    let port = server.port;

    let output = run_cli(
        port,
        &["fill", "@e1", "hello", "world", "--enter", "--tab", "3"],
    )
    .await;
    output.assert_status(0, "fill");
    output.assert_stderr_empty();
    assert_eq!(
        output.stdout,
        "Filled @e1 textbox \"Email\" with \"hello world\". Pressed Enter.\n"
    );

    let sent = sent_commands(&seen);
    assert_eq!(sent.len(), 2, "expected fill + press, got {sent:?}");
    assert_eq!(sent[0]["cmd"]["action"], json!("fill"));
    assert_eq!(sent[0]["cmd"]["selector"], json!("@e1"));
    assert_eq!(sent[0]["cmd"]["value"], json!("hello world"));
    assert!(sent[0]["cmd"].get("delay").is_none());
    assert_eq!(sent[0]["tabId"], json!(3));
    assert_eq!(sent[1]["cmd"]["action"], json!("press"));
    assert_eq!(sent[1]["cmd"]["key"], json!("Enter"));

    clear_commands(&seen);
    let output = run_cli(port, &["fill", "@e1", "abc", "--slow", "--delay", "25"]).await;
    server.stop().await;
    output.assert_status(0, "slow fill");
    assert_eq!(output.stdout, "Typed @e1 textbox \"Email\" with \"abc\".\n");
    let sent = sent_commands(&seen);
    assert_eq!(sent.len(), 1, "expected a single type, got {sent:?}");
    assert_eq!(sent[0]["cmd"]["action"], json!("type"));
    assert_eq!(sent[0]["cmd"]["delay"], json!(25));
}

// ── read ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn read_handles_markdown_outline_empty_truncated_and_llms() {
    let server = TestServer::start().await;
    let seen = seen();
    server
        .extension
        .set_handler(recording_handler(Arc::clone(&seen), |message| {
            let cmd = &message["cmd"];
            match cmd["action"].as_str() {
                Some("url") => {
                    FakeReply::Ok(json!({ "ok": true, "url": "https://active.example" }))
                }
                Some("read") if cmd["llms"] == json!("index") => FakeReply::Ok(json!({
                    "ok": true,
                    "markdown": "# LLMS Index\n\nentries",
                })),
                Some("read") if cmd["outline"] == json!(true) => FakeReply::Ok(json!({
                    "ok": true,
                    "outline": "# Only Headings",
                })),
                Some("read") if cmd["url"] == json!("https://empty.example") => {
                    FakeReply::Ok(json!({
                        "ok": true,
                        "markdown": "   ",
                    }))
                }
                Some("read") if cmd["url"] == json!("https://truncated.example") => {
                    FakeReply::Ok(json!({ "ok": true, "markdown": "body", "truncated": true }))
                }
                Some("read") => FakeReply::Ok(json!({
                    "ok": true,
                    "source": "tab",
                    "url": "https://cli.example",
                    "markdown": "# Read Me\n\nCLI reader output.",
                })),
                _ => FakeReply::Ok(json!({ "ok": true })),
            }
        }));
    let port = server.port;

    // Markdown body.
    let output = run_cli(port, &["read", "https://cli.example"]).await;
    output.assert_status(0, "read");
    output.assert_stderr_empty();
    assert_eq!(output.stdout, "# Read Me\n\nCLI reader output.\n");
    let read = find_command(&seen, "read");
    assert_eq!(read["cmd"]["url"], json!("https://cli.example"));
    assert_eq!(read["cmd"]["outline"], json!(false));
    assert!(read["cmd"].get("llms").is_none());
    assert!(read["cmd"].get("filter").is_none());

    // `--filter` is forwarded on the read command.
    clear_commands(&seen);
    let output = run_cli(port, &["read", "https://cli.example", "--filter", "Read"]).await;
    output.assert_status(0, "read filter");
    let read = find_command(&seen, "read");
    assert_eq!(read["cmd"]["filter"], json!("Read"));

    // Outline-only body (no URL requested -> omitted from the command).
    clear_commands(&seen);
    let output = run_cli(port, &["read", "--outline"]).await;
    output.assert_status(0, "read outline");
    assert_eq!(output.stdout, "# Only Headings\n");
    let read = find_command(&seen, "read");
    assert!(read["cmd"].get("url").is_none());
    assert_eq!(read["cmd"]["outline"], json!(true));

    // Empty document fallback.
    let output = run_cli(port, &["read", "https://empty.example"]).await;
    output.assert_status(0, "read empty");
    assert_eq!(
        output.stdout,
        "(empty document: the reader extracted no text; the page may still be loading \
         or may not expose readable content)\n"
    );

    // Truncation note.
    let output = run_cli(port, &["read", "https://truncated.example"]).await;
    output.assert_status(0, "read truncated");
    assert_eq!(output.stdout, "body\n\n[content truncated by reader]\n");

    // `--llms index` without a URL first resolves the active tab URL.
    clear_commands(&seen);
    let output = run_cli(port, &["read", "--llms", "index"]).await;
    output.assert_status(0, "read llms");
    assert_eq!(output.stdout, "# LLMS Index\n\nentries\n");
    let sent = sent_commands(&seen);
    assert_eq!(sent.len(), 2, "expected url + read, got {sent:?}");
    assert_eq!(sent[0]["cmd"]["action"], json!("url"));
    assert_eq!(sent[1]["cmd"]["action"], json!("read"));
    assert_eq!(sent[1]["cmd"]["url"], json!("https://active.example"));
    assert_eq!(sent[1]["cmd"]["llms"], json!("index"));

    // Invalid `--llms` value is rejected before any command is sent.
    clear_commands(&seen);
    let output = run_cli(port, &["read", "--llms", "bogus"]).await;
    output.assert_status(1, "read bad llms");
    output.assert_stdout_empty();
    assert_eq!(output.stderr, "Error: --llms must be 'index' or 'full'\n");
    assert!(sent_commands(&seen).is_empty());

    // Active URL lookup failure.
    server
        .extension
        .set_handler(|_message| FakeReply::Ok(json!({ "ok": true })));
    let output = run_cli(port, &["read", "--llms", "index"]).await;
    server.stop().await;
    output.assert_status(1, "read llms no active url");
    assert_eq!(
        output.stderr,
        "Error: Could not determine the active tab URL; pass a URL explicitly\n"
    );
}

// ── Parser + error surface ───────────────────────────────────────────────

#[tokio::test]
async fn flag_parser_and_usage_errors_exit_one() {
    let port = common::free_port();

    for (args, expected) in [
        (vec!["snap", "--bogus"], "Error: Unknown option: --bogus\n"),
        (
            vec!["snap", "--interactive=1"],
            "Error: --interactive does not take a value\n",
        ),
        (vec!["snap", "--tab"], "Error: --tab requires a value\n"),
        (
            vec!["snap", "--tab", "abc"],
            "Error: --tab must be a non-negative number\n",
        ),
        (vec!["snap", "-z"], "Error: Unknown option: -z\n"),
        (
            vec!["snap", "extra"],
            "Error: snap does not take positional arguments: extra\n",
        ),
        (
            vec!["status", "extra"],
            "Error: status does not take positional arguments: extra\n",
        ),
        (
            vec!["click"],
            "Error: Usage: browser-use click <@ref|selector>\n",
        ),
        (
            vec!["click", "@e1", "@e2"],
            "Error: click accepts exactly one target\n",
        ),
        (
            vec!["fill"],
            "Error: Usage: browser-use fill <@ref|selector> <text>\n",
        ),
        (vec!["fill", "@e1"], "Error: fill requires text\n"),
        (
            vec!["read", "a", "b"],
            "Error: read accepts at most one URL\n",
        ),
    ] {
        let output = run_cli(port, &args).await;
        output.assert_status(1, &format!("{args:?}"));
        output.assert_stdout_empty();
        assert_eq!(output.stderr, expected, "args {args:?}");
    }
}

#[tokio::test]
async fn extension_errors_and_invalid_responses_become_error_stderr() {
    let server = TestServer::start().await;
    server
        .extension
        .set_handler(|message| match message["cmd"]["action"].as_str() {
            Some("snapshot") => FakeReply::Err("No element matches @e9".to_string()),
            Some("click") => FakeReply::Ok(json!("not-an-object")),
            _ => FakeReply::Ok(json!({ "ok": true })),
        });
    let port = server.port;

    let output = run_cli(port, &["snap"]).await;
    output.assert_status(1, "snap extension error");
    output.assert_stdout_empty();
    assert_eq!(output.stderr, "Error: No element matches @e9\n");

    let output = run_cli(port, &["click", "@e1"]).await;
    server.stop().await;
    output.assert_status(1, "click invalid response");
    output.assert_stdout_empty();
    assert_eq!(
        output.stderr,
        "Error: Extension returned an invalid click response\n"
    );
}

//! Shared helpers for the integration tests.
//!
//! Every test binary (`secret_test`, `hub_test`, `mcp_test`, `cli_test`,
//! `ffi_test`) compiles this module via `mod common;`. Helpers are generous on
//! purpose so any test target can spin up a real hub on an ephemeral port, a
//! fake extension, raw HTTP checks and raw WebSocket upgrades.
//!
//! Never use the legacy hub's fixed port (19455): always bind port 0 / use
//! [`free_port`].

#![allow(dead_code)]

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use futures_util::stream::SplitSink;
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::error::Error as WsError;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::frame::CloseFrame;
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};
use tokio_tungstenite::{connect_async_with_config, MaybeTlsStream, WebSocketStream};

use browser_use::hub::{Hub, HubOptions};

/// Fixed secret used by the test suite (64 hex characters, like a generated
/// secret, so `secrets_match` length checks behave realistically).
pub const TEST_SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[cfg(windows)]
pub const APP_LAUNCH_NAME: &str = "Browser Use";
#[cfg(not(windows))]
pub const APP_LAUNCH_NAME: &str = "Browser Use.app";

/// The test secret as an owned `String`.
pub fn test_secret() -> String {
    TEST_SECRET.to_string()
}

/// Client-side WebSocket type used by every helper in this module.
pub type ClientSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

// ── Ports ────────────────────────────────────────────────────────────────

/// Reserve an ephemeral loopback port by binding and immediately releasing it.
pub fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral port");
    let port = listener
        .local_addr()
        .expect("ephemeral listener address")
        .port();
    drop(listener);
    port
}

/// `HubOptions` for a test hub: `port == 0` binds an ephemeral port.
pub fn hub_options(port: u16) -> HubOptions {
    HubOptions {
        port,
        secret: test_secret(),
        ..Default::default()
    }
}

/// Start a hub on an ephemeral port with the test secret.
pub async fn start_hub() -> (Hub, u16) {
    start_hub_with_secret(TEST_SECRET).await
}

/// Start a hub on an ephemeral port with an explicit secret.
pub async fn start_hub_with_secret(secret: &str) -> (Hub, u16) {
    let options = HubOptions {
        port: 0,
        secret: secret.to_string(),
        ..Default::default()
    };
    let hub = Hub::new(options);
    let port = hub.start().await.expect("test hub failed to start");
    (hub, port)
}

// ── Raw HTTP ─────────────────────────────────────────────────────────────

/// A parsed raw HTTP/1.1 response.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: String,
    pub body: String,
}

impl HttpResponse {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

/// Issue a minimal `GET` over loopback and read the whole response.
pub async fn http_get(port: u16, path: &str) -> HttpResponse {
    http_request(port, "GET", path, &[]).await
}

/// Convert `GET` body to JSON (returns `Value::Null` when the body is not JSON).
pub async fn http_get_json(port: u16, path: &str) -> (u16, Value) {
    let response = http_get(port, path).await;
    (response.status, response.json())
}

/// Issue an arbitrary HTTP/1.1 request over a fresh loopback connection.
pub async fn http_request(
    port: u16,
    method: &str,
    path: &str,
    extra_headers: &[(&str, &str)],
) -> HttpResponse {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap_or_else(|error| panic!("connect to test hub {port}: {error}"));

    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    for (name, value) in extra_headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");

    let exchange = async {
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write raw http request");
        let mut buffer = Vec::new();
        stream
            .read_to_end(&mut buffer)
            .await
            .expect("read raw http response");
        buffer
    };
    let buffer = tokio::time::timeout(Duration::from_secs(5), exchange)
        .await
        .expect("raw http request timed out");

    parse_raw_response(&buffer)
}

/// Parse a raw `HTTP/1.1 <status> ...\r\n<headers>\r\n\r\n<body>` response.
pub fn parse_raw_response(buffer: &[u8]) -> HttpResponse {
    let split = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or(buffer.len());
    let (head, body) = if split < buffer.len() {
        (&buffer[..split], &buffer[split + 4..])
    } else {
        (buffer, &[][..])
    };
    let head = String::from_utf8_lossy(head).to_string();
    let body = String::from_utf8_lossy(body).to_string();
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    HttpResponse {
        status,
        headers: head,
        body,
    }
}

// ── WebSocket clients ────────────────────────────────────────────────────

/// Why a raw client WebSocket connection failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsConnectError {
    /// The hub answered the upgrade request with a non-101 status.
    Http(u16),
    Other(String),
}

impl std::fmt::Display for WsConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WsConnectError::Http(status) => write!(f, "HTTP {status}"),
            WsConnectError::Other(message) => f.write_str(message),
        }
    }
}

/// Open a raw WebSocket connection to `path` with an optional `Origin` header.
pub async fn ws_connect(
    port: u16,
    path: &str,
    origin: Option<&str>,
) -> Result<ClientSocket, WsConnectError> {
    let url = format!("ws://127.0.0.1:{port}{path}");
    let mut request = url
        .into_client_request()
        .map_err(|error| WsConnectError::Other(error.to_string()))?;
    if let Some(origin) = origin {
        let value = HeaderValue::from_str(origin)
            .map_err(|error| WsConnectError::Other(error.to_string()))?;
        request.headers_mut().insert("origin", value);
    }

    match tokio::time::timeout(
        Duration::from_secs(3),
        connect_async_with_config(request, None, true),
    )
    .await
    {
        Err(_) => Err(WsConnectError::Other("timed out opening websocket".into())),
        Ok(Ok((socket, _response))) => Ok(socket),
        Ok(Err(WsError::Http(response))) => Err(WsConnectError::Http(response.status().as_u16())),
        Ok(Err(error)) => Err(WsConnectError::Other(error.to_string())),
    }
}

/// Read the next JSON text frame, or `None` on timeout / close / error.
pub async fn next_json(socket: &mut ClientSocket, timeout_ms: u64) -> Option<Value> {
    let deadline = Duration::from_millis(timeout_ms);
    loop {
        match tokio::time::timeout(deadline, socket.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                return serde_json::from_str::<Value>(&text).ok();
            }
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) | Err(_) => {
                return None;
            }
            Ok(Some(Ok(_))) => continue,
        }
    }
}

/// Wait for the socket to close and return `(code, reason)`.
pub async fn next_close(socket: &mut ClientSocket, timeout_ms: u64) -> Option<(u16, String)> {
    let wait = async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Close(frame))) => {
                    let code = frame
                        .as_ref()
                        .map(|frame| u16::from(frame.code))
                        .unwrap_or(1000);
                    let reason = frame
                        .as_ref()
                        .map(|frame| frame.reason.to_string())
                        .unwrap_or_default();
                    return (code, reason);
                }
                Some(Ok(_)) => continue,
                Some(Err(_)) | None => {
                    return (1006u16, String::new());
                }
            }
        }
    };
    tokio::time::timeout(Duration::from_millis(timeout_ms), wait)
        .await
        .ok()
}

/// Send a JSON text frame.
pub async fn send_json(socket: &mut ClientSocket, value: &Value) -> bool {
    socket.send(Message::text(value.to_string())).await.is_ok()
}

// ── Fake extension ───────────────────────────────────────────────────────

/// How a [`FakeExtension`] answers one `cmd` frame.
#[derive(Debug, Clone)]
pub enum FakeReply {
    /// Reply `{type:"res", ok:true, result}`.
    Ok(Value),
    /// Reply `{type:"res", ok:false, error}`.
    Err(String),
    /// Do not reply (tests then answer manually or expect a timeout).
    Ignore,
}

type FakeHandler = Arc<dyn Fn(&Value) -> FakeReply + Send + Sync>;
type FakeHandlerSlot = Arc<StdMutex<FakeHandler>>;

/// The default auto-responder: plausible `tab_list` data and an echo for
/// everything else. Mirrors the legacy `test/helpers.ts` responder.
pub fn default_reply(message: &Value) -> FakeReply {
    let cmd = message.get("cmd").cloned().unwrap_or(Value::Null);
    let action = cmd.get("action").and_then(Value::as_str).unwrap_or("");
    match action {
        "tab_list" => FakeReply::Ok(json!({
            "ok": true,
            "tabs": [
                { "id": 42, "active": true, "title": "Alpha", "url": "https://alpha.example" },
                { "id": 43, "active": false, "title": "Beta", "url": "https://beta.example" },
            ],
        })),
        _ => FakeReply::Ok(json!({
            "ok": true,
            "action": action,
            "tabId": message.get("tabId").cloned().unwrap_or(Value::Null),
        })),
    }
}

enum ExtensionControl {
    Send(Value),
    Close(u16, String),
    Abort,
}

/// A minimal Gecko Orange background page: connects to `ws://<port>/` with an
/// extension Origin, authenticates with `hello`, auto-answers `cmd` frames and
/// replies to JSON pings.
///
/// Tests can observe incoming commands with [`FakeExtension::next_command`],
/// replace the auto-responder with [`FakeExtension::set_handler`] (e.g. to hold
/// a command open), or answer manually with [`FakeExtension::send_json`].
pub struct FakeExtension {
    pub port: u16,
    hello_ok: Value,
    command_rx: mpsc::UnboundedReceiver<Value>,
    close_rx: mpsc::UnboundedReceiver<(u16, String)>,
    control_tx: mpsc::UnboundedSender<ExtensionControl>,
    handler: FakeHandlerSlot,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl FakeExtension {
    /// Connect with the shared [`TEST_SECRET`] and the default auto-responder.
    pub async fn connect(port: u16) -> Result<Self, String> {
        Self::connect_with_secret(port, TEST_SECRET).await
    }

    /// Connect with an explicit secret (e.g. a per-test random one).
    pub async fn connect_with_secret(port: u16, secret: &str) -> Result<Self, String> {
        Self::connect_with_origin(port, secret, "moz-extension://browser-use-test").await
    }

    /// Connect with an explicit secret and Origin header.
    pub async fn connect_with_origin(
        port: u16,
        secret: &str,
        origin: &str,
    ) -> Result<Self, String> {
        let mut socket = ws_connect(port, "/", Some(origin))
            .await
            .map_err(|error| format!("fake extension upgrade failed: {error}"))?;

        let hello = json!({
            "type": "hello",
            "secret": secret,
            "client": "gecko-orange",
            "version": "3.0.0",
            "userAgent": "browser-use-test",
        });
        socket
            .send(Message::text(hello.to_string()))
            .await
            .map_err(|error| format!("fake extension hello failed: {error}"))?;

        let mut buffered_commands = Vec::new();
        let hello_ok = loop {
            let incoming = tokio::time::timeout(Duration::from_secs(3), socket.next())
                .await
                .map_err(|_| "timed out waiting for hello_ok".to_string())?
                .ok_or_else(|| "connection closed before hello_ok".to_string())?
                .map_err(|error| format!("websocket error before hello_ok: {error}"))?;

            match incoming {
                Message::Text(text) => {
                    let value: Value = serde_json::from_str(&text)
                        .map_err(|error| format!("invalid JSON from hub: {error}"))?;
                    match value.get("type").and_then(Value::as_str) {
                        Some("hello_ok") => break value,
                        Some("cmd") => buffered_commands.push(value),
                        _ => {}
                    }
                }
                Message::Close(_) => return Err("closed before hello_ok".into()),
                _ => {}
            }
        };

        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (close_tx, close_rx) = mpsc::unbounded_channel();
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let handler: FakeHandlerSlot =
            Arc::new(StdMutex::new(Arc::new(default_reply) as FakeHandler));

        let task = tokio::spawn(extension_loop(
            socket,
            buffered_commands,
            command_tx,
            close_tx,
            control_rx,
            Arc::clone(&handler),
        ));

        Ok(Self {
            port,
            hello_ok,
            command_rx,
            close_rx,
            control_tx,
            handler,
            task: Some(task),
        })
    }

    /// The `hello_ok` frame the hub sent.
    pub fn hello_ok(&self) -> &Value {
        &self.hello_ok
    }

    /// Replace the auto-responder for all future `cmd` frames.
    pub fn set_handler(&self, handler: impl Fn(&Value) -> FakeReply + Send + Sync + 'static) {
        *self
            .handler
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(handler);
    }

    /// Reply to every future command with the same `result`.
    pub fn set_fixed_result(&self, result: Value) {
        self.set_handler(move |_| FakeReply::Ok(result.clone()));
    }

    /// Never auto-reply: tests answer manually with [`Self::send_json`].
    pub fn set_ignore_all(&self) {
        self.set_handler(|_| FakeReply::Ignore);
    }

    /// Wait for the next `cmd` frame the hub forwards (2s timeout).
    pub async fn next_command(&mut self) -> Value {
        self.next_command_timeout(2_000)
            .await
            .expect("timed out waiting for an extension command")
    }

    /// Wait up to `timeout_ms` for the next `cmd` frame.
    pub async fn next_command_timeout(&mut self, timeout_ms: u64) -> Option<Value> {
        tokio::time::timeout(Duration::from_millis(timeout_ms), self.command_rx.recv())
            .await
            .ok()
            .flatten()
    }

    /// Wait for the hub to close this extension socket (2s timeout).
    pub async fn next_close(&mut self) -> Option<(u16, String)> {
        self.next_close_timeout(2_000).await
    }

    /// Wait up to `timeout_ms` for a close frame.
    pub async fn next_close_timeout(&mut self, timeout_ms: u64) -> Option<(u16, String)> {
        tokio::time::timeout(Duration::from_millis(timeout_ms), self.close_rx.recv())
            .await
            .ok()
            .flatten()
    }

    /// Send a raw JSON frame to the hub (used to answer ignored commands).
    pub fn send_json(&self, value: Value) -> bool {
        self.control_tx.send(ExtensionControl::Send(value)).is_ok()
    }

    /// Answer a previously observed command id with a successful result.
    pub fn respond(&self, id: &Value, result: Value) -> bool {
        self.send_json(json!({ "type": "res", "id": id, "ok": true, "result": result }))
    }

    /// Answer a previously observed command id with an error.
    pub fn respond_error(&self, id: &Value, error: &str) -> bool {
        self.send_json(json!({ "type": "res", "id": id, "ok": false, "error": error }))
    }

    /// Gracefully close the extension socket.
    pub async fn close(mut self) {
        let _ = self
            .control_tx
            .send(ExtensionControl::Close(1000, "test done".to_string()));
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_millis(1_000), task).await;
        }
    }

    /// Drop the TCP connection without a close handshake (simulates a crash).
    pub fn abort(mut self) {
        let _ = self.control_tx.send(ExtensionControl::Abort);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Drop for FakeExtension {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn extension_loop(
    socket: ClientSocket,
    buffered_commands: Vec<Value>,
    command_tx: mpsc::UnboundedSender<Value>,
    close_tx: mpsc::UnboundedSender<(u16, String)>,
    mut control_rx: mpsc::UnboundedReceiver<ExtensionControl>,
    handler: FakeHandlerSlot,
) {
    let (mut sink, mut stream) = socket.split();

    for command in buffered_commands {
        answer_command(&command, &command_tx, &handler, &mut sink).await;
    }

    loop {
        tokio::select! {
            control = control_rx.recv() => {
                match control {
                    Some(ExtensionControl::Send(value)) => {
                        if sink.send(Message::text(value.to_string())).await.is_err() {
                            break;
                        }
                    }
                    Some(ExtensionControl::Close(code, reason)) => {
                        let _ = sink.send(close_message(code, reason.clone())).await;
                        let _ = close_tx.send((code, reason));
                        let _ = sink.close().await;
                        break;
                    }
                    Some(ExtensionControl::Abort) | None => break,
                }
            }
            incoming = stream.next() => {
                let message = match incoming {
                    Some(Ok(message)) => message,
                    Some(Err(_)) | None => break,
                };
                match message {
                    Message::Text(text) => {
                        let Ok(value) = serde_json::from_str::<Value>(&text) else {
                            continue;
                        };
                        match value.get("type").and_then(Value::as_str) {
                            Some("cmd") => {
                                answer_command(&value, &command_tx, &handler, &mut sink).await;
                            }
                            Some("ping") => {
                                let pong = json!({ "type": "pong", "ts": now_ms() });
                                let _ = sink.send(Message::text(pong.to_string())).await;
                            }
                            _ => {}
                        }
                    }
                    Message::Close(frame) => {
                        let code = frame
                            .as_ref()
                            .map(|frame| u16::from(frame.code))
                            .unwrap_or(1000);
                        let reason = frame
                            .as_ref()
                            .map(|frame| frame.reason.to_string())
                            .unwrap_or_default();
                        let _ = close_tx.send((code, reason));
                        let _ = sink.close().await;
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn answer_command(
    message: &Value,
    command_tx: &mpsc::UnboundedSender<Value>,
    handler: &FakeHandlerSlot,
    sink: &mut SplitSink<ClientSocket, Message>,
) {
    let _ = command_tx.send(message.clone());

    let reply = {
        let handler = Arc::clone(
            &*handler
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        handler(message)
    };
    let id = message.get("id").cloned().unwrap_or(Value::Null);

    let response = match reply {
        FakeReply::Ok(result) => {
            json!({ "type": "res", "id": id, "ok": true, "result": result })
        }
        FakeReply::Err(error) => {
            json!({ "type": "res", "id": id, "ok": false, "error": error })
        }
        FakeReply::Ignore => return,
    };
    let _ = sink.send(Message::text(response.to_string())).await;
}

fn close_message(code: u16, reason: String) -> Message {
    Message::Close(Some(CloseFrame {
        code: CloseCode::from(code),
        reason: Utf8Bytes::from(reason),
    }))
}

// ── Test server bundle ───────────────────────────────────────────────────

/// A hub + connected fake extension, torn down explicitly by [`TestServer::stop`].
pub struct TestServer {
    pub hub: Hub,
    pub port: u16,
    pub extension: FakeExtension,
}

impl TestServer {
    /// Hub on an ephemeral port + authenticated fake extension.
    pub async fn start() -> Self {
        Self::start_with_secret(TEST_SECRET, TEST_SECRET).await
    }

    /// Hub and fake extension sharing an explicit secret.
    pub async fn start_with_secret(hub_secret: &str, extension_secret: &str) -> Self {
        let options = HubOptions {
            port: 0,
            secret: hub_secret.to_string(),
            ..Default::default()
        };
        let hub = Hub::new(options);
        let port = hub.start().await.expect("test hub failed to start");
        let extension = FakeExtension::connect_with_secret(port, extension_secret)
            .await
            .expect("fake extension failed to connect");
        Self {
            hub,
            port,
            extension,
        }
    }

    /// Close the extension and stop the hub.
    pub async fn stop(self) {
        let TestServer { hub, extension, .. } = self;
        extension.close().await;
        hub.stop().await;
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

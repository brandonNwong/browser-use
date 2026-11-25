//! Pure `/peer` WebSocket client.
//!
//! Connects to the in-process hub that `Browser Use.app` owns, authenticates
//! with the shared secret, and lazily reconnects on the next call. There is no
//! leader election here: the Swift app is always the hub owner.
//!
//! Keep the public API stable; `tools`, `cli` and `mcp` build on it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::error::Error as WsError;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::frame::CloseFrame;
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};
use tokio_tungstenite::{connect_async_with_config, MaybeTlsStream, WebSocketStream};

use crate::hub::{DEFAULT_COMMAND_TIMEOUT_MS, DEFAULT_PORT, HUB_HOST, MAX_COMMAND_TIMEOUT_MS};

/// Timeout for the TCP + WebSocket handshake.
const CONNECT_TIMEOUT_MS: u64 = 5_000;
/// Timeout waiting for the hub's `hello_ok` after sending `hello`.
const HELLO_TIMEOUT_MS: u64 = 3_000;
/// Extra slack over the per-command timeout before the peer gives up.
const REQUEST_TIMEOUT_SLACK_MS: u64 = 5_000;
/// How long a reconnecting peer waits for a restarting hub.
const RETRY_WINDOW_MS: u64 = 2_500;
/// Poll interval inside the retry window.
const RETRY_INTERVAL_MS: u64 = 150;
/// Timeout for a `/peer` status round trip (`tabs: true`).
const STATUS_TIMEOUT_MS: u64 = 10_000;

type PeerSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type PendingMap = Arc<StdMutex<HashMap<String, oneshot::Sender<Result<Value, PeerError>>>>>;

/// Why a peer operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerErrorKind {
    /// Nothing is listening (first connect). `browser-use` prints an actionable
    /// "Browser Use is not running..." message.
    Offline,
    /// The hub rejected the shared secret (close code 4001).
    Unauthorized,
    /// The hub went away mid-session (close code other than 4001 / socket loss).
    Closed,
    /// A request exceeded its timeout.
    Timeout,
    /// The hub rejected the `/peer` upgrade with HTTP 403.
    Forbidden,
    /// Some other HTTP/WebSocket failure.
    Http,
    /// Malformed protocol message.
    Protocol,
}

/// Peer error with a human-readable message.
#[derive(Debug, Clone)]
pub struct PeerError {
    pub kind: PeerErrorKind,
    pub message: String,
}

impl PeerError {
    pub fn new(kind: PeerErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PeerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PeerError {}

#[cfg(windows)]
const APP_LAUNCH_NAME: &str = "Browser Use";
#[cfg(not(windows))]
const APP_LAUNCH_NAME: &str = "Browser Use.app";

impl PeerError {
    fn offline(port: u16) -> Self {
        Self::new(
            PeerErrorKind::Offline,
            format!(
                "Browser Use is not running at ws://{HUB_HOST}:{port}. Launch {APP_LAUNCH_NAME} first."
            ),
        )
    }

    fn unauthorized() -> Self {
        Self::new(
            PeerErrorKind::Unauthorized,
            "Hub rejected the shared secret (run 'browser-use secret' and update the extension)",
        )
    }

    fn closed(code: u16, reason: &str) -> Self {
        let suffix = if reason.is_empty() {
            String::new()
        } else {
            format!(": {reason}")
        };
        Self::new(
            PeerErrorKind::Closed,
            format!("Hub connection closed (code {code}{suffix})"),
        )
    }
}

/// One authenticated WebSocket connection to the hub's `/peer` endpoint.
struct ActivePeer {
    tx: mpsc::UnboundedSender<Message>,
    pending: PendingMap,
    join: StdMutex<Option<tokio::task::JoinHandle<()>>>,
}

/// Lazy, auto-reconnecting `/peer` client.
pub struct PeerClient {
    port: u16,
    secret: String,
    connect_timeout_ms: u64,
    active: Mutex<Option<Arc<ActivePeer>>>,
    connect_lock: Mutex<()>,
    has_connected_once: AtomicBool,
    request_seq: AtomicU64,
}

impl PeerClient {
    pub fn new(port: u16, secret: String) -> Self {
        Self::with_connect_timeout(port, secret, CONNECT_TIMEOUT_MS)
    }

    /// Additive constructor used by `browser-use status` (legacy used a
    /// 1_500 ms connect timeout). `connect_timeout_ms == 0` keeps the default.
    pub fn with_connect_timeout(port: u16, secret: String, connect_timeout_ms: u64) -> Self {
        let port = if port == 0 { DEFAULT_PORT } else { port };
        let connect_timeout_ms = if connect_timeout_ms == 0 {
            CONNECT_TIMEOUT_MS
        } else {
            connect_timeout_ms
        };
        Self {
            port,
            secret,
            connect_timeout_ms,
            active: Mutex::new(None),
            connect_lock: Mutex::new(()),
            has_connected_once: AtomicBool::new(false),
            request_seq: AtomicU64::new(0),
        }
    }

    /// `ws://127.0.0.1:<port>/peer`.
    pub fn url(&self) -> String {
        format!("ws://{HUB_HOST}:{}/peer", self.port)
    }

    /// The hub port this client targets.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Whether a live `/peer` socket is currently held (additive helper for
    /// `browser-use status` and tests; it never reconnects).
    pub async fn is_connected(&self) -> bool {
        self.live_peer().await.is_some()
    }

    /// Execute a command on the extension. `tab_id == None` sends JSON null.
    /// `timeout_ms == 0` uses the default 30s. Reconnects lazily when the hub
    /// restarted (retrying ECONNREFUSED/ECONNRESET for ~2.5s after the first
    /// successful connection).
    pub async fn command(
        &self,
        cmd: Value,
        tab_id: Option<i64>,
        timeout_ms: u64,
    ) -> Result<Value, PeerError> {
        let timeout = normalize_timeout(timeout_ms);
        let wait_ms = timeout + REQUEST_TIMEOUT_SLACK_MS;
        let message = self
            .request(wait_ms, |id| {
                json!({
                    "type": "cmd",
                    "id": id,
                    "tabId": tab_id,
                    "cmd": cmd,
                    "timeoutMs": timeout,
                })
            })
            .await?;

        Ok(message.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Fetch hub status (including tabs) over `/peer`.
    /// Returns the full `status_res` message object.
    pub async fn status(&self) -> Result<Value, PeerError> {
        self.request(
            STATUS_TIMEOUT_MS,
            |id| json!({ "type": "status", "id": id, "tabs": true }),
        )
        .await
    }

    /// Close the current socket (best effort).
    pub async fn close(&self) {
        let peer = self.active.lock().await.take();
        let Some(peer) = peer else {
            return;
        };

        let _ = peer.tx.send(Message::Close(Some(CloseFrame {
            code: CloseCode::Normal,
            reason: Utf8Bytes::from("browser-use client closing"),
        })));

        let handle = peer.join.lock().ok().and_then(|mut slot| slot.take());
        if let Some(mut handle) = handle {
            if tokio::time::timeout(Duration::from_millis(250), &mut handle)
                .await
                .is_err()
            {
                handle.abort();
            }
        }
    }

    // ── Connection management ────────────────────────────────────────────

    async fn live_peer(&self) -> Option<Arc<ActivePeer>> {
        let guard = self.active.lock().await;
        guard.as_ref().filter(|peer| !peer.tx.is_closed()).cloned()
    }

    async fn ensure_peer(&self) -> Result<Arc<ActivePeer>, PeerError> {
        if let Some(peer) = self.live_peer().await {
            return Ok(peer);
        }

        let _connect_guard = self.connect_lock.lock().await;
        if let Some(peer) = self.live_peer().await {
            return Ok(peer);
        }

        let peer = self.connect_with_retry().await?;
        *self.active.lock().await = Some(peer.clone());
        Ok(peer)
    }

    async fn invalidate(&self, peer: &Arc<ActivePeer>) {
        let mut guard = self.active.lock().await;
        if guard
            .as_ref()
            .map(|current| Arc::ptr_eq(current, peer))
            .unwrap_or(false)
        {
            *guard = None;
        }
    }

    /// Initial connection policy: a first-connect refusal is the actionable
    /// `Offline` error; once we have talked to a hub, a refusal means the app is
    /// probably mid-restart, so retry briefly.
    async fn connect_with_retry(&self) -> Result<Arc<ActivePeer>, PeerError> {
        match self.connect_once().await {
            Ok(peer) => return Ok(peer),
            Err(error) if error.kind == PeerErrorKind::Offline => {
                if !self.has_connected_once.load(Ordering::SeqCst) {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }

        let deadline = tokio::time::Instant::now() + Duration::from_millis(RETRY_WINDOW_MS);
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(PeerError::offline(self.port));
            }
            tokio::time::sleep(Duration::from_millis(RETRY_INTERVAL_MS)).await;
            match self.connect_once().await {
                Ok(peer) => return Ok(peer),
                Err(error) if error.kind == PeerErrorKind::Offline => continue,
                Err(error) => return Err(error),
            }
        }
    }

    async fn connect_once(&self) -> Result<Arc<ActivePeer>, PeerError> {
        let mut socket = self.connect_socket().await?;
        self.authenticate(&mut socket).await?;

        let pending: PendingMap = Arc::new(StdMutex::new(HashMap::new()));
        let (tx, rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(peer_task(socket, pending.clone(), rx));
        self.has_connected_once.store(true, Ordering::SeqCst);

        Ok(Arc::new(ActivePeer {
            tx,
            pending,
            join: StdMutex::new(Some(task)),
        }))
    }

    async fn connect_socket(&self) -> Result<PeerSocket, PeerError> {
        let request = self
            .url()
            .into_client_request()
            .map_err(|error| PeerError::new(PeerErrorKind::Http, error.to_string()))?;

        match tokio::time::timeout(
            Duration::from_millis(self.connect_timeout_ms),
            connect_async_with_config(request, None, true),
        )
        .await
        {
            Err(_) => Err(PeerError::new(
                PeerErrorKind::Timeout,
                format!("Timed out connecting to hub at {}", self.url()),
            )),
            Ok(Ok((socket, _response))) => Ok(socket),
            Ok(Err(error)) => Err(map_connect_error(error, self.port)),
        }
    }

    async fn authenticate(&self, socket: &mut PeerSocket) -> Result<(), PeerError> {
        let hello = json!({
            "type": "hello",
            "secret": self.secret,
            "client": crate::SERVER_NAME,
            "version": crate::VERSION,
        });
        socket
            .send(Message::text(hello.to_string()))
            .await
            .map_err(|error| {
                PeerError::new(
                    PeerErrorKind::Closed,
                    format!("Unable to send peer hello: {error}"),
                )
            })?;

        let deadline = tokio::time::Instant::now() + Duration::from_millis(HELLO_TIMEOUT_MS);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(PeerError::new(
                    PeerErrorKind::Timeout,
                    "Timed out waiting for hub hello_ok",
                ));
            }

            let incoming = match tokio::time::timeout(remaining, socket.next()).await {
                Err(_) => {
                    return Err(PeerError::new(
                        PeerErrorKind::Timeout,
                        "Timed out waiting for hub hello_ok",
                    ));
                }
                Ok(None) => {
                    return Err(PeerError::new(
                        PeerErrorKind::Closed,
                        "Hub connection closed during handshake",
                    ));
                }
                Ok(Some(Err(_))) => {
                    return Err(PeerError::new(
                        PeerErrorKind::Closed,
                        "Hub connection closed during handshake",
                    ));
                }
                Ok(Some(Ok(message))) => message,
            };

            match incoming {
                Message::Text(text) => {
                    let Ok(value) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    if value.get("type").and_then(Value::as_str) == Some("hello_ok") {
                        return Ok(());
                    }
                }
                Message::Close(frame) => {
                    let code: u16 = frame
                        .as_ref()
                        .map(|frame| u16::from(frame.code))
                        .unwrap_or(1000);
                    let reason = frame
                        .as_ref()
                        .map(|frame| frame.reason.to_string())
                        .unwrap_or_default();
                    return Err(if code == 4001 {
                        PeerError::unauthorized()
                    } else {
                        PeerError::closed(code, &reason)
                    });
                }
                // Ping/Pong/Binary before hello_ok are ignored, like legacy.
                _ => continue,
            }
        }
    }

    // ── Request/response ─────────────────────────────────────────────────

    async fn request<F>(&self, wait_ms: u64, build: F) -> Result<Value, PeerError>
    where
        F: Fn(&str) -> Value,
    {
        let mut attempts = 0u32;
        loop {
            attempts += 1;
            let peer = self.ensure_peer().await?;

            let id = format!("p{}", self.request_seq.fetch_add(1, Ordering::SeqCst) + 1);
            let payload = build(&id);
            let text = serde_json::to_string(&payload)
                .map_err(|error| PeerError::new(PeerErrorKind::Protocol, error.to_string()))?;

            let (tx, rx) = oneshot::channel();
            if let Ok(mut pending) = peer.pending.lock() {
                pending.insert(id.clone(), tx);
            }

            if peer.tx.send(Message::text(text)).is_err() {
                if let Ok(mut pending) = peer.pending.lock() {
                    pending.remove(&id);
                }
                self.invalidate(&peer).await;
                if attempts >= 2 {
                    return Err(PeerError::offline(self.port));
                }
                continue;
            }

            match tokio::time::timeout(Duration::from_millis(wait_ms), rx).await {
                Ok(Ok(Ok(value))) => return Ok(value),
                Ok(Ok(Err(error))) => return Err(error),
                Ok(Err(_)) => {
                    return Err(PeerError::new(
                        PeerErrorKind::Closed,
                        "Hub connection closed",
                    ));
                }
                Err(_) => {
                    if let Ok(mut pending) = peer.pending.lock() {
                        pending.remove(&id);
                    }
                    return Err(PeerError::new(
                        PeerErrorKind::Timeout,
                        format!("Hub request timed out after {wait_ms}ms"),
                    ));
                }
            }
        }
    }
}

fn normalize_timeout(timeout_ms: u64) -> u64 {
    if timeout_ms == 0 {
        DEFAULT_COMMAND_TIMEOUT_MS
    } else {
        timeout_ms.min(MAX_COMMAND_TIMEOUT_MS)
    }
}

fn map_connect_error(error: WsError, port: u16) -> PeerError {
    match error {
        WsError::Http(response) => {
            let status = response.status().as_u16();
            if status == 403 {
                PeerError::new(
                    PeerErrorKind::Forbidden,
                    "Hub rejected peer WebSocket (HTTP 403)",
                )
            } else {
                PeerError::new(
                    PeerErrorKind::Http,
                    format!("Hub rejected peer WebSocket (HTTP {status})"),
                )
            }
        }
        WsError::Io(ref io) if is_connection_refused(io) => PeerError::offline(port),
        WsError::Io(io) => PeerError::new(PeerErrorKind::Http, io.to_string()),
        other => PeerError::new(PeerErrorKind::Http, other.to_string()),
    }
}

fn is_connection_refused(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::NotConnected
    )
}

/// Owns the socket: forwards queued requests, answers pings, resolves pending
/// responses and drains every pending request when the socket dies.
async fn peer_task(
    socket: PeerSocket,
    pending: PendingMap,
    mut outgoing: mpsc::UnboundedReceiver<Message>,
) {
    let (mut sink, mut stream) = socket.split();
    let mut close_info: Option<(u16, String)> = None;

    loop {
        tokio::select! {
            queued = outgoing.recv() => {
                match queued {
                    Some(message) => {
                        let is_close = matches!(message, Message::Close(_));
                        if sink.send(message).await.is_err() {
                            break;
                        }
                        if is_close {
                            break;
                        }
                    }
                    None => {
                        let _ = sink.close().await;
                        break;
                    }
                }
            }
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        handle_incoming_text(&text, &pending, &mut sink).await;
                    }
                    Some(Ok(Message::Close(frame))) => {
                        close_info = frame.map(|frame| {
                            (u16::from(frame.code), frame.reason.to_string())
                        });
                        let _ = sink.close().await;
                        break;
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(_)) | None => break,
                }
            }
        }
    }

    let error = match close_info {
        Some((4001, _)) => PeerError::unauthorized(),
        Some((code, reason)) => PeerError::closed(code, &reason),
        None => PeerError::new(PeerErrorKind::Closed, "Hub connection closed"),
    };

    let waiters: Vec<oneshot::Sender<Result<Value, PeerError>>> = {
        let mut guard = pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.drain().map(|(_, sender)| sender).collect()
    };
    for waiter in waiters {
        let _ = waiter.send(Err(error.clone()));
    }
}

async fn handle_incoming_text(
    text: &str,
    pending: &PendingMap,
    sink: &mut futures_util::stream::SplitSink<PeerSocket, Message>,
) {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let Some(object) = value.as_object() else {
        return;
    };
    let message_type = object.get("type").and_then(Value::as_str).unwrap_or("");

    match message_type {
        "res" | "status_res" => {
            let id = object.get("id").map(id_string).unwrap_or_default();
            let waiter = {
                let mut guard = pending
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                guard.remove(&id)
            };
            let Some(waiter) = waiter else {
                return;
            };

            let result = if object.get("ok") == Some(&Value::Bool(false)) {
                Err(PeerError::new(
                    PeerErrorKind::Http,
                    string_value(object.get("error"), "Browser command failed"),
                ))
            } else if message_type == "status_res"
                && object.get("error").is_some()
                && object.get("hub") != Some(&Value::Bool(true))
            {
                Err(PeerError::new(
                    PeerErrorKind::Http,
                    string_value(object.get("error"), "Hub status failed"),
                ))
            } else {
                Ok(value)
            };
            let _ = waiter.send(result);
        }
        "ping" => {
            let pong = json!({ "type": "pong", "ts": now_ms() });
            let _ = sink.send(Message::text(pong.to_string())).await;
        }
        _ => {}
    }
}

fn id_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        other => other.to_string(),
    }
}

fn string_value(value: Option<&Value>, fallback: &str) -> String {
    match value {
        Some(Value::String(text)) if !text.is_empty() => text.clone(),
        _ => fallback.to_string(),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

//! Ported legacy `test/hub.test.ts` + the multi-peer/reload parts of
//! `test/multi-session.test.ts` (leader election replaced by a single
//! in-process hub). Every hub binds an ephemeral loopback port; the legacy
//! launchd hub on 19455 is never touched.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use browser_use::hub::{
    try_resolve_port, Hub, HubOptions, DEFAULT_COMMAND_TIMEOUT_MS, DEFAULT_PORT,
    HEARTBEAT_INTERVAL_MS, MAX_COMMAND_TIMEOUT_MS, PROTOCOL_VERSION,
};
use browser_use::peer::{PeerClient, PeerErrorKind};
use common::{
    free_port, http_get, next_close, next_json, send_json, start_hub, test_secret, ws_connect,
    FakeExtension, FakeReply, TestServer, WsConnectError,
};
use serde_json::{json, Value};

/// Poll `predicate` every 10ms until it is true or `timeout_ms` elapses.
async fn wait_until(mut predicate: impl FnMut() -> bool, timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if predicate() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn start_hub_at(port: u16, secret: &str) -> Hub {
    let hub = Hub::new(HubOptions {
        port,
        secret: secret.to_string(),
        ..Default::default()
    });
    let bound = hub.start().await.expect("test hub failed to start");
    assert_eq!(bound, port, "hub bound a different port than requested");
    hub
}

// ── HTTP surface ─────────────────────────────────────────────────────────

#[tokio::test]
async fn http_health_status_and_diagnostic_endpoints() {
    let (hub, port) = start_hub().await;

    let health = http_get(port, "/health").await;
    assert_eq!(health.status, 200);
    let headers = health.headers.to_ascii_lowercase();
    assert!(headers.contains("content-type: application/json"));
    assert!(headers.contains("cache-control: no-store"));
    assert!(headers.contains("x-content-type-options: nosniff"));

    let body = health.json();
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["service"], json!("browser-use"));
    assert_eq!(body["mode"], json!("hub"));
    assert_eq!(body["protocol"], json!(PROTOCOL_VERSION));
    assert_eq!(body["port"], json!(port));
    assert_eq!(body["extensionConnected"], json!(false));
    assert_eq!(body["peers"], json!(0));
    // The fixed /health payload mirrors legacy: no `extension` object.
    assert!(body.get("extension").is_none());

    let status = http_get(port, "/status").await;
    assert_eq!(status.status, 200);
    assert_eq!(status.json()["port"], json!(port));
    assert_eq!(status.json()["protocol"], json!(PROTOCOL_VERSION));

    let root = http_get(port, "/").await;
    assert_eq!(root.status, 200);
    assert!(root.body.contains("browser-use hub"));
    assert!(root.body.contains(&format!("ws://127.0.0.1:{port}/peer")));

    // `/api/*` is intentionally gone (Swift talks through the C-ABI now).
    assert_eq!(http_get(port, "/api/status").await.status, 404);
    assert_eq!(http_get(port, "/api/command").await.status, 404);
    assert_eq!(http_get(port, "/nope").await.status, 404);

    hub.stop().await;
}

// ── Upgrade origin enforcement + authentication ──────────────────────────

#[tokio::test]
async fn rejects_web_origins_before_handshake() {
    let (hub, port) = start_hub().await;

    assert_eq!(
        ws_connect(port, "/", Some("https://evil.example"))
            .await
            .unwrap_err(),
        WsConnectError::Http(403)
    );
    assert_eq!(
        ws_connect(port, "/peer", Some("http://localhost:3000"))
            .await
            .unwrap_err(),
        WsConnectError::Http(403)
    );
    // Extension paths require an extension Origin (unchanged legacy rule).
    assert_eq!(
        ws_connect(port, "/", None).await.unwrap_err(),
        WsConnectError::Http(403)
    );
    // Unknown upgrade path.
    assert_eq!(
        ws_connect(port, "/other", Some("moz-extension://test"))
            .await
            .unwrap_err(),
        WsConnectError::Http(404)
    );
    assert!(!hub.is_extension_connected());

    hub.stop().await;
}

#[tokio::test]
async fn rejects_bad_extension_and_peer_secrets_with_4001() {
    let (hub, port) = start_hub().await;

    let mut extension = ws_connect(port, "/", Some("moz-extension://bad-secret"))
        .await
        .expect("extension origin must be accepted");
    assert!(
        send_json(
            &mut extension,
            &json!({ "type": "hello", "secret": "not-the-secret", "client": "gecko-orange", "version": "3" })
        )
        .await
    );
    let close = next_close(&mut extension, 2_000)
        .await
        .expect("extension should be closed");
    assert_eq!(close.0, 4001);
    assert_eq!(close.1, "Unauthorized");
    assert!(!hub.is_extension_connected());

    // `/peer` accepts native clients (no Origin) but still checks the secret.
    let mut peer = ws_connect(port, "/peer", None)
        .await
        .expect("native peer origin must be accepted");
    assert!(send_json(&mut peer, &json!({ "type": "hello", "secret": "wrong" })).await);
    let close = next_close(&mut peer, 2_000)
        .await
        .expect("peer should be closed");
    assert_eq!(close.0, 4001);
    assert_eq!(close.1, "Unauthorized");

    // Correct secret gets the hub-mode hello_ok.
    let mut peer = ws_connect(port, "/peer", None).await.expect("peer upgrade");
    assert!(
        send_json(
            &mut peer,
            &json!({ "type": "hello", "secret": test_secret(), "client": "browser-use", "version": "0.1.0" })
        )
        .await
    );
    let reply = next_json(&mut peer, 2_000).await.expect("hello_ok");
    assert_eq!(reply["type"], json!("hello_ok"));
    assert_eq!(reply["hub"], json!("browser-use"));
    assert_eq!(reply["mode"], json!("hub"));
    assert_eq!(reply["protocol"], json!(PROTOCOL_VERSION));
    assert_eq!(reply["port"], json!(port));

    hub.stop().await;
}

#[tokio::test]
async fn extension_auth_timeout_closes_with_4001() {
    let (hub, port) = start_hub().await;

    let mut extension = ws_connect(port, "/", Some("moz-extension://no-hello"))
        .await
        .expect("upgrade");
    // Never send hello: the hub must drop the socket after AUTH_TIMEOUT_MS.
    let started = Instant::now();
    let close = next_close(&mut extension, 6_000)
        .await
        .expect("auth timeout close frame");
    assert_eq!(close.0, 4001);
    assert_eq!(close.1, "Unauthorized");
    assert!(
        started.elapsed() >= Duration::from_millis(2_500),
        "closed too early: {:?}",
        started.elapsed()
    );

    hub.stop().await;
}

// ── Command round-trips + peer multiplexing ──────────────────────────────

#[tokio::test]
async fn round_trips_commands_and_multiplexes_peers() {
    let server = TestServer::start().await;
    let port = server.port;

    // Per-selector result so the two concurrent peers can be told apart.
    server.extension.set_handler(|message| {
        let command = message.get("cmd").cloned().unwrap_or(Value::Null);
        match command.get("action").and_then(Value::as_str).unwrap_or("") {
            "tab_list" => common::default_reply(message),
            _ => FakeReply::Ok(json!({
                "ok": true,
                "name": if command.get("selector") == Some(&json!("@e1")) { "One" } else { "Two" },
                "tabId": message.get("tabId").cloned().unwrap_or(Value::Null),
            })),
        }
    });

    // 1. Direct hub -> extension command.
    let direct = server
        .hub
        .send(
            json!({ "action": "snapshot" }),
            Some(7),
            DEFAULT_COMMAND_TIMEOUT_MS,
            false,
        )
        .await
        .expect("direct command");
    assert_eq!(direct["tabId"], json!(7));

    // 2. Two peers multiplex over the one extension socket.
    let peer_one = PeerClient::new(port, test_secret());
    let peer_two = PeerClient::new(port, test_secret());
    let (one, two) = tokio::join!(
        peer_one.command(json!({ "action": "click", "selector": "@e1" }), None, 5_000),
        peer_two.command(
            json!({ "action": "click", "selector": "@e2" }),
            Some(42),
            5_000
        ),
    );
    let one = one.expect("peer one command");
    let two = two.expect("peer two command");
    assert_eq!(one["name"], json!("One"));
    assert_eq!(two["name"], json!("Two"));
    assert_eq!(two["tabId"], json!(42));

    // 3. Peer status includes the full legacy HubStatus shape + tabs.
    let status = peer_one.status().await.expect("peer status");
    assert_eq!(status["type"], json!("status_res"));
    assert_eq!(status["hub"], json!(true));
    assert_eq!(status["ok"], json!(true));
    assert_eq!(status["service"], json!("browser-use"));
    assert_eq!(status["mode"], json!("hub"));
    assert_eq!(status["port"], json!(port));
    assert_eq!(status["protocol"], json!(PROTOCOL_VERSION));
    assert_eq!(status["extensionConnected"], json!(true));
    assert_eq!(status["peers"], json!(2));
    assert_eq!(status["tabs"].as_array().unwrap().len(), 2);
    assert!(status["tabsError"].is_null() || status.get("tabsError").is_none());
    assert_eq!(status["extension"]["client"], json!("gecko-orange"));
    assert_eq!(status["extension"]["version"], json!("3.0.0"));

    // 4. Local status with and without tabs.
    let with_tabs = server.hub.status(true).await;
    assert_eq!(
        with_tabs.tabs.as_ref().unwrap().as_array().unwrap().len(),
        2
    );
    assert!(with_tabs.tabs_error.is_none());
    let without_tabs = server.hub.status(false).await;
    assert!(without_tabs.tabs.is_none());
    assert!(server.hub.status_snapshot().extension_connected);
    assert_eq!(
        server.hub.extension_info().unwrap().client.as_deref(),
        Some("gecko-orange")
    );

    // 5. Invalid command object is rejected like legacy.
    let error = server
        .hub
        .send(json!("not-an-object"), None, 1_000, true)
        .await
        .unwrap_err();
    assert!(error.message.contains("Command must be an object"));

    // 6. `close` is best-effort and the client lazily reconnects.
    peer_one.close().await;
    assert!(!peer_one.is_connected().await);
    let again = peer_one
        .command(json!({ "action": "click", "selector": "@e1" }), None, 5_000)
        .await
        .expect("lazy reconnect");
    assert_eq!(again["name"], json!("One"));

    peer_one.close().await;
    peer_two.close().await;
    server.stop().await;
}

// ── State callback ───────────────────────────────────────────────────────

#[tokio::test]
async fn state_callback_mirrors_extension_and_peer_changes() {
    let snapshots: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&snapshots);
    let hub = Hub::new(HubOptions {
        port: 0,
        secret: test_secret(),
        on_state: Some(Arc::new(move |json: &str| {
            if let Ok(value) = serde_json::from_str::<Value>(json) {
                sink.lock().unwrap().push(value);
            }
        })),
        ..Default::default()
    });
    let port = hub.start().await.expect("hub start");
    assert!(snapshots.lock().unwrap().is_empty());

    let extension = FakeExtension::connect(port).await.expect("extension");
    assert!(
        wait_until(|| !snapshots.lock().unwrap().is_empty(), 2_000).await,
        "extension connect must invoke on_state"
    );
    {
        let snapshots = snapshots.lock().unwrap();
        let latest = snapshots.last().unwrap();
        assert_eq!(latest["ok"], json!(true));
        assert_eq!(latest["extensionConnected"], json!(true));
        assert_eq!(latest["peers"], json!(0));
        assert!(latest["extension"]["client"] == json!("gecko-orange"));
        // Guardrail 4: the callback snapshot never carries `tabs`.
        assert!(latest.get("tabs").is_none());
    }

    let peer = PeerClient::new(port, test_secret());
    peer.status().await.expect("peer connects");
    assert!(
        wait_until(
            || snapshots
                .lock()
                .unwrap()
                .last()
                .map(|snapshot| snapshot["peers"] == json!(1))
                .unwrap_or(false),
            2_000
        )
        .await,
        "peer connect must invoke on_state"
    );

    peer.close().await;
    assert!(
        wait_until(
            || snapshots
                .lock()
                .unwrap()
                .last()
                .map(|snapshot| snapshot["peers"] == json!(0))
                .unwrap_or(false),
            2_000
        )
        .await,
        "peer disconnect must invoke on_state"
    );

    extension.abort();
    assert!(
        wait_until(
            || snapshots
                .lock()
                .unwrap()
                .last()
                .map(|snapshot| snapshot["extensionConnected"] == json!(false))
                .unwrap_or(false),
            2_000
        )
        .await,
        "extension disconnect must invoke on_state"
    );

    hub.stop().await;
}

// ── Extension reconnect waiting ──────────────────────────────────────────

#[tokio::test]
async fn send_waits_for_a_reconnecting_extension_then_errors_actionably() {
    let hub = Arc::new(Hub::new(HubOptions {
        port: 0,
        secret: test_secret(),
        ..Default::default()
    }));
    let port = hub.start().await.expect("hub start");

    let sender = {
        let hub = Arc::clone(&hub);
        tokio::spawn(async move {
            hub.send(json!({ "action": "ping" }), None, 5_000, true)
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(150)).await;
    let extension = FakeExtension::connect(port)
        .await
        .expect("extension reconnect");
    let result = sender.await.unwrap().expect("command after reconnect");
    assert_eq!(result["action"], json!("ping"));

    extension.close().await;
    hub.stop().await;

    // No extension ever connects: the actionable legacy error must surface.
    let (empty_hub, empty_port) = start_hub().await;
    let error = empty_hub
        .send(json!({ "action": "snapshot" }), None, 1_000, true)
        .await
        .unwrap_err();
    assert!(error
        .message
        .contains("Gecko Orange extension is not connected"));
    assert!(error
        .message
        .contains(&format!("ws://127.0.0.1:{empty_port}")));
    assert!(error.message.contains("browser-use secret"));
    assert_eq!(
        empty_hub.extension_not_connected_message(),
        error.message,
        "the public helper and the send error must match"
    );
    empty_hub.stop().await;
}

// ── Single-extension replacement ─────────────────────────────────────────

#[tokio::test]
async fn newer_extension_replaces_old_socket_and_fails_pending_command() {
    let hub = Arc::new(Hub::new(HubOptions {
        port: 0,
        secret: test_secret(),
        ..Default::default()
    }));
    let port = hub.start().await.expect("hub start");

    let mut first = FakeExtension::connect(port).await.expect("first extension");
    first.set_ignore_all();

    let pending = {
        let hub = Arc::clone(&hub);
        tokio::spawn(async move {
            hub.send(json!({ "action": "snapshot" }), None, 5_000, false)
                .await
        })
    };
    let command = first.next_command().await;
    assert_eq!(command["cmd"]["action"], json!("snapshot"));

    // A reloaded background page reconnects: the newer connection wins.
    let second = FakeExtension::connect(port)
        .await
        .expect("second extension");

    let error = pending.await.unwrap().unwrap_err();
    assert_eq!(error.message, "Extension reconnected before replying");
    let close = first
        .next_close()
        .await
        .expect("old extension must be closed");
    assert_eq!(close.0, 4002);
    assert_eq!(close.1, "Replaced by a newer extension connection");
    assert!(hub.is_extension_connected());
    assert_eq!(
        hub.extension_info().unwrap().client.as_deref(),
        Some("gecko-orange")
    );

    let direct = hub
        .send(json!({ "action": "snapshot" }), None, 2_000, true)
        .await
        .expect("new extension answers");
    assert_eq!(direct["action"], json!("snapshot"));

    second.close().await;
    hub.stop().await;
}

#[tokio::test]
async fn hub_stop_fails_pending_commands_and_closes_sockets() {
    let hub = Arc::new(Hub::new(HubOptions {
        port: 0,
        secret: test_secret(),
        ..Default::default()
    }));
    let port = hub.start().await.expect("hub start");
    let mut extension = FakeExtension::connect(port).await.expect("extension");
    extension.set_ignore_all();

    let pending = {
        let hub = Arc::clone(&hub);
        tokio::spawn(async move {
            hub.send(json!({ "action": "snapshot" }), None, 5_000, false)
                .await
        })
    };
    extension.next_command().await;

    hub.stop().await;
    let error = pending.await.unwrap().unwrap_err();
    assert_eq!(error.message, "Hub stopped");

    let close = extension
        .next_close()
        .await
        .expect("extension must observe the shutdown close");
    assert_eq!(close.0, 1001);
    assert_eq!(close.1, "Hub shutting down");
    extension.close().await;
}

// ── PeerClient connection policy ─────────────────────────────────────────

#[tokio::test]
async fn peer_first_connect_refused_is_actionable_offline_error() {
    let port = free_port();
    let peer = PeerClient::new(port, test_secret());

    let started = Instant::now();
    let error = peer
        .command(json!({ "action": "snapshot" }), None, 1_000)
        .await
        .unwrap_err();
    assert_eq!(error.kind, PeerErrorKind::Offline);
    assert_eq!(
        error.message,
        format!(
            "Browser Use is not running at ws://127.0.0.1:{port}. Launch {} first.",
            common::APP_LAUNCH_NAME
        )
    );
    // First-connect refusal must not burn the (post-connect) retry window.
    assert!(
        started.elapsed() < Duration::from_millis(4_000),
        "first connect took {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn peer_wrong_secret_is_unauthorized() {
    let (hub, port) = start_hub().await;
    let peer = PeerClient::new(port, "definitely-not-the-secret".to_string());

    let error = peer
        .command(json!({ "action": "snapshot" }), None, 1_000)
        .await
        .unwrap_err();
    assert_eq!(error.kind, PeerErrorKind::Unauthorized);
    assert!(error.message.contains("Hub rejected the shared secret"));
    assert!(error.message.contains("browser-use secret"));

    hub.stop().await;
}

#[tokio::test]
async fn peer_reconnects_through_hub_restart_then_times_out_the_retry_window() {
    let port = free_port();
    let secret = test_secret();

    let hub_one = start_hub_at(port, &secret).await;
    let extension_one = FakeExtension::connect(port).await.expect("extension one");
    let peer = Arc::new(PeerClient::new(port, secret.clone()));
    let status = peer.status().await.expect("initial status");
    assert_eq!(status["extensionConnected"], json!(true));

    // Simulate `just reload`: hub dies, peer socket closes.
    drop(extension_one);
    hub_one.stop().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!peer.is_connected().await);

    // Start a call while the hub is down, then bring the hub/sub-extension
    // back inside the 2.5s retry window: the call must succeed.
    let call = {
        let peer = Arc::clone(&peer);
        tokio::spawn(async move {
            peer.command(json!({ "action": "snapshot" }), None, 5_000)
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(150)).await;
    let hub_two = start_hub_at(port, &secret).await;
    let extension_two = FakeExtension::connect(port).await.expect("extension two");
    let result = call.await.unwrap().expect("command rides out the restart");
    assert_eq!(result["action"], json!("snapshot"));

    // Now leave the hub down: the retry window must expire with the
    // actionable offline error (and take roughly the configured 2.5s).
    drop(extension_two);
    hub_two.stop().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = Instant::now();
    let error = peer
        .command(json!({ "action": "snapshot" }), None, 1_000)
        .await
        .unwrap_err();
    assert_eq!(error.kind, PeerErrorKind::Offline);
    assert!(error
        .message
        .contains(&format!("Launch {} first.", common::APP_LAUNCH_NAME)));
    assert!(
        started.elapsed() >= Duration::from_millis(2_000),
        "retry window did not run: {:?}",
        started.elapsed()
    );
}

// ── Constants + port resolution ──────────────────────────────────────────

#[test]
fn protocol_constants_match_legacy_wire_values() {
    assert_eq!(PROTOCOL_VERSION, "3.0.0");
    assert_eq!(DEFAULT_PORT, 19455);
    assert_eq!(HEARTBEAT_INTERVAL_MS, 15_000);
    assert_eq!(DEFAULT_COMMAND_TIMEOUT_MS, 30_000);
    assert_eq!(MAX_COMMAND_TIMEOUT_MS, 600_000);
}

#[test]
fn resolve_port_honors_orange_port_rules() {
    assert_eq!(try_resolve_port(Some("12345")), Ok(12345));
    assert_eq!(try_resolve_port(Some(" 19455 ")), Ok(19455));
    assert_eq!(try_resolve_port(Some("")), Ok(DEFAULT_PORT));
    assert!(try_resolve_port(Some("0")).is_err());
    assert!(try_resolve_port(Some("65536")).is_err());
    assert!(try_resolve_port(Some("http")).is_err());
    assert!(try_resolve_port(Some("1.5")).is_err());
    assert_eq!(browser_use::hub::resolve_port(Some("1")), 1);

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var("ORANGE_PORT").ok();
    std::env::set_var("ORANGE_PORT", "12345");
    assert_eq!(browser_use::hub::resolve_port(None), 12345);
    std::env::remove_var("ORANGE_PORT");
    assert_eq!(browser_use::hub::resolve_port(None), DEFAULT_PORT);
    if let Some(previous) = previous {
        std::env::set_var("ORANGE_PORT", previous);
    } else {
        std::env::remove_var("ORANGE_PORT");
    }
}

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::*;

use axum_test::TestServer;

static WS_ID: AtomicU64 = AtomicU64::new(0);

async fn ws_conn() -> DbConn {
    let id = WS_ID.fetch_add(1, Ordering::Relaxed);
    crate::db::connect(&format!(
        "sqlite://file%3Arhp_ws_test_{id}?mode=memory&cache=shared"
    ))
    .await
    .expect("in memory db")
}

fn write_ws_script(src: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rhp_ws_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("socket.rhp");
    std::fs::write(&path, src).unwrap();
    path
}

async fn ws_server(src: &str) -> TestServer {
    let path = write_ws_script(src);
    let folder = path.parent().unwrap().to_path_buf();
    let conn = ws_conn().await;
    TestServer::builder()
        .http_transport()
        .build(build_router(folder, conn))
}

async fn ws_connect(server: &TestServer, path: &str) -> axum_test::TestWebSocket {
    server.get_websocket(path).await.into_websocket().await
}

/// ws_connect returns once the 101 handshake completes, but the server task
/// registers the client only after that. Awaiting a welcome message guarantees
/// the client's script ran and the client is registered before we send.
async fn ws_ready(server: &TestServer, path: &str) -> axum_test::TestWebSocket {
    let mut ws = ws_connect(server, path).await;
    assert_eq!(ws.receive_text().await, "ready");
    ws
}

async fn assert_no_message(ws: &mut axum_test::TestWebSocket) {
    let result = tokio::time::timeout(Duration::from_millis(200), ws.receive_text()).await;
    assert!(result.is_err(), "expected no message, but got one");
}

#[tokio::test]
async fn test_ws_chat_relay_no_self_echo() {
    let server = ws_server(
        r#"<rhp method="SOCKET">
SOCKET.Client().OnMessage(msg => SOCKET.Peers().Send(msg))
return "ready"
</rhp>"#,
    )
    .await;

    let mut a = ws_ready(&server, "/socket.rhp").await;
    let mut b = ws_ready(&server, "/socket.rhp").await;

    a.send_text("hello").await;

    assert_eq!(b.receive_text().await, "hello");
    assert_no_message(&mut a).await;
}

#[tokio::test]
async fn test_ws_everyone_echo_includes_self() {
    let server = ws_server(
        r#"<rhp method="SOCKET">
SOCKET.Client().OnMessage(msg => SOCKET.Everyone().Send(msg))
return "ready"
</rhp>"#,
    )
    .await;

    let mut a = ws_ready(&server, "/socket.rhp").await;
    let mut b = ws_ready(&server, "/socket.rhp").await;

    a.send_text("hi").await;

    assert_eq!(a.receive_text().await, "hi");
    assert_eq!(b.receive_text().await, "hi");
}

#[tokio::test]
async fn test_ws_rooms_are_isolated() {
    let server = ws_server(
        r#"<rhp method="SOCKET">
SOCKET.Join(QUERY.room)
SOCKET.Client().OnMessage(msg => SOCKET.Peers().Send(msg))
return "ready"
</rhp>"#,
    )
    .await;

    let mut a = ws_ready(&server, "/socket.rhp?room=alpha").await;
    let mut b = ws_ready(&server, "/socket.rhp?room=alpha").await;
    let mut c = ws_ready(&server, "/socket.rhp?room=beta").await;

    a.send_text("room message").await;

    assert_eq!(b.receive_text().await, "room message");
    assert_no_message(&mut c).await;
}

#[tokio::test]
async fn test_ws_first_message_from_return() {
    let server = ws_server(
        r#"<rhp method="SOCKET">
return "welcome " + QUERY.name
</rhp>"#,
    )
    .await;

    let mut ws = ws_connect(&server, "/socket.rhp?name=alice").await;
    assert_eq!(ws.receive_text().await, "welcome alice");
}

#[tokio::test]
async fn test_ws_first_message_json_object() {
    let server = ws_server(
        r#"<rhp method="SOCKET">
return { event: "welcome", id: SOCKET.Client().Id() }
</rhp>"#,
    )
    .await;

    let mut ws = ws_connect(&server, "/socket.rhp").await;
    let value: serde_json::Value = ws.receive_json().await;
    assert_eq!(value["event"], "welcome");
    assert!(value["id"].as_str().is_some());
    uuid::Uuid::parse_str(value["id"].as_str().unwrap()).expect("uuid");
}

#[tokio::test]
async fn test_ws_get_targets_specific_client() {
    let server = ws_server(
        r#"<rhp method="SOCKET">
SOCKET.Join("alpha")
SOCKET.Client().OnMessage(msg => {
  if (QUERY.target) {
    SOCKET.Peers().Get(QUERY.target).Send(msg)
  }
})
return SOCKET.Client().Id()
</rhp>"#,
    )
    .await;

    let mut b = ws_connect(&server, "/socket.rhp").await;
    let b_id = b.receive_text().await;
    let mut a = ws_connect(&server, &format!("/socket.rhp?target={}", urlencode(&b_id))).await;
    let _ = a.receive_text().await; // a's own id
    let mut c = ws_connect(&server, "/socket.rhp").await;
    let _ = c.receive_text().await;

    a.send_text("secret").await;

    assert_eq!(b.receive_text().await, "secret");
    assert_no_message(&mut c).await;
}

#[tokio::test]
async fn test_ws_on_close_broadcasts() {
    let server = ws_server(
        r#"<rhp method="SOCKET">
SOCKET.Join("alpha")
SOCKET.Client().OnClose(() => SOCKET.Everyone().Send("left"))
return "ready"
</rhp>"#,
    )
    .await;

    let a = ws_ready(&server, "/socket.rhp").await;
    let mut b = ws_ready(&server, "/socket.rhp").await;

    a.close().await;

    assert_eq!(b.receive_text().await, "left");
}

#[tokio::test]
async fn test_ws_get_request_skips_socket_sections() {
    let server = ws_server(
        r#"<rhp method="SOCKET">
return "never sent"
</rhp>"#,
    )
    .await;

    let response = server.get("/socket.rhp").await;
    response.assert_status_ok();
    assert_eq!(response.text().trim(), "");
}

#[tokio::test]
async fn test_chat_rhp_serves_page_and_relays_messages() {
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("public");
    let conn = ws_conn().await;
    let server = TestServer::builder()
        .http_transport()
        .build(build_router(folder, conn));

    let page = server.get("/chat.rhp").await;
    page.assert_status_ok();
    assert!(page.text().contains("<div id=\"messages\"></div>"));
    assert!(page.text().contains("new WebSocket("));
    assert!(!page.text().contains("method=\"SOCKET\""));

    let mut a = ws_connect(&server, "/chat.rhp?room=alpha").await;
    let mut b = ws_connect(&server, "/chat.rhp?room=alpha").await;
    let mut c = ws_connect(&server, "/chat.rhp?room=beta").await;

    let welcome_a: serde_json::Value = a.receive_json().await;
    let welcome_b: serde_json::Value = b.receive_json().await;
    let welcome_c: serde_json::Value = c.receive_json().await;
    assert_eq!(welcome_a["event"], "welcome");
    assert_eq!(welcome_a["room"], "alpha");
    assert_eq!(welcome_b["room"], "alpha");
    assert_eq!(welcome_c["room"], "beta");
    assert!(welcome_a["id"].as_str().is_some());

    a.send_text(r#"{"name":"alice","text":"hi"}"#).await;

    // The server renders the message HTML and relays it to everyone in the
    // room (including the sender), tagged with the sender's socket id.
    let to_a: serde_json::Value = a.receive_json().await;
    let to_b: serde_json::Value = b.receive_json().await;
    assert_eq!(
        to_a["html"],
        r#"<div class="message"><span class="sender">alice</span><span class="text">hi</span></div>"#
    );
    assert_eq!(to_a["html"], to_b["html"]);
    assert_eq!(to_a["from"], welcome_a["id"]);
    assert_no_message(&mut c).await;
}

#[tokio::test]
async fn test_chat_rhp_persists_and_replays_history() {
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("public");
    let conn = ws_conn().await;
    let server = TestServer::builder()
        .http_transport()
        .build(build_router(folder, conn));

    // The page GET runs the `<rhp method="GET">` section that creates the table.
    server.get("/chat.rhp").await.assert_status_ok();

    let mut a = ws_connect(&server, "/chat.rhp?room=alpha").await;
    let _: serde_json::Value = a.receive_json().await; // welcome
    let mut b = ws_connect(&server, "/chat.rhp?room=alpha").await;
    let _: serde_json::Value = b.receive_json().await; // welcome
    let mut c = ws_connect(&server, "/chat.rhp?room=beta").await;
    let _: serde_json::Value = c.receive_json().await; // welcome

    // Messages are rendered server-side and relayed to everyone in the room...
    a.send_text(r#"{"name":"alice","text":"first"}"#).await;
    a.send_text(r#"{"name":"alice","text":"second"}"#).await;
    let to_b_1: serde_json::Value = b.receive_json().await;
    let to_b_2: serde_json::Value = b.receive_json().await;
    assert_eq!(
        to_b_1["html"],
        r#"<div class="message"><span class="sender">alice</span><span class="text">first</span></div>"#
    );
    assert_eq!(
        to_b_2["html"],
        r#"<div class="message"><span class="sender">alice</span><span class="text">second</span></div>"#
    );
    assert_no_message(&mut c).await;

    // ...and a fresh join replays the stored history as rendered HTML for
    // that room only.
    let mut d = ws_connect(&server, "/chat.rhp?room=alpha").await;
    let welcome: serde_json::Value = d.receive_json().await;
    assert_eq!(welcome["event"], "welcome");
    assert_eq!(
        welcome["historyHtml"].as_str().unwrap(),
        r#"<div class="message"><span class="sender">alice</span><span class="text">first</span></div><div class="message"><span class="sender">alice</span><span class="text">second</span></div>"#
    );

    let mut e = ws_connect(&server, "/chat.rhp?room=beta").await;
    let welcome_beta: serde_json::Value = e.receive_json().await;
    assert_eq!(welcome_beta["historyHtml"].as_str().unwrap(), "");
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

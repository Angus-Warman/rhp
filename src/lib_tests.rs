use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

use axum_test::TestServer;

static DB_ID: AtomicU64 = AtomicU64::new(0);

async fn test_server() -> TestServer {
    let conn = unique_conn().await;
    TestServer::new(build_router("./public".into(), conn))
}

async fn unique_conn() -> DbConn {
    // sqlite ":memory:" is per-connection, so use a unique named shared
    // in-memory database per test to keep every pooled connection on the same db.
    let id = DB_ID.fetch_add(1, Ordering::Relaxed);
    connect(&format!("file%3Arhp_lib_test_{id}?mode=memory&cache=shared")).await.expect("in memory db")
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[tokio::test]
async fn test_hello_world() {
    let server = test_server().await;
    let response = server.get("/hello.rhp").await;
    response.assert_status_ok();
    response.assert_text_contains("Hello\nWorld");
}

#[tokio::test]
async fn test_non_rhp() {
    let server = test_server().await;
    let response = server.get("/plain.html").await;
    response.assert_status_ok();
    response.assert_text_contains("Plain");
}

#[tokio::test]
async fn test_method_routing_post() {
    let server = test_server().await;
    let response = server.post("/methods.rhp").await;
    response.assert_status_ok();
    response.assert_text("\nthis is a post request");
}

#[tokio::test]
async fn test_method_routing_put() {
    let server = test_server().await;
    let response = server.put("/methods.rhp").await;
    response.assert_status_ok();
    response.assert_text("this is a put request\n");
}

#[tokio::test]
async fn test_query_global() {
    let server = test_server().await;
    let response = server.put("/query.rhp?id=123").await;
    response.assert_status_ok();
    response.assert_text("123");
}

#[tokio::test]
async fn test_body_global_text() {
    let server = test_server().await;
    let response = server.post("/body.rhp").text("hello world").await;
    response.assert_status_ok();
    response.assert_text(r#"{ text: "hello world" }"#);
}

#[tokio::test]
async fn test_body_global_json() {
    let server = test_server().await;
    let response = server.post("/body.rhp").json(&serde_json::json!({"name": "rhp"})).await;
    response.assert_status_ok();
    response.assert_text(r#"{ name: "rhp" }"#);
}

#[tokio::test]
async fn test_body_global_form() {
    let server = test_server().await;
    let response = server
        .post("/body.rhp")
        .form(&[("color", "red"), ("color", "blue")])
        .await;
    response.assert_status_ok();
    response.assert_text(r#"{ color: "red", colors: ["red", "blue"] }"#);
}

#[tokio::test]
async fn test_body_global_get_is_empty() {
    let server = test_server().await;
    let response = server.get("/body.rhp").await;
    response.assert_status_ok();
    response.assert_text("{}");
}

#[tokio::test]
async fn test_body_global_invalid_json_is_400() {
    let server = test_server().await;
    let response = server
        .post("/body.rhp")
        .bytes("{not json}".into())
        .content_type("application/json")
        .await;
    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_crud_workflow() {
    let conn = unique_conn().await;
    conn.exec("CREATE TABLE widgets (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT)").run().await;
    let server = TestServer::new(build_router("./public".into(), conn));

    // GET: no widgets yet
    assert_eq!(server.get("/crud.rhp").await.text().trim(), "[]");

    // POST: create a widget
    assert_eq!(
        server.post("/crud.rhp").json(&serde_json::json!({"name": "widget a"})).await.text().trim(),
        "{ ok: true, rowsAffected: 1 }"
    );

    // GET: now lists it
    assert_eq!(
        server.get("/crud.rhp").await.text().trim(),
        r#"[{ id: 1, name: "widget a" }]"#
    );

    // PUT: rename it by id
    assert_eq!(
        server.put("/crud.rhp?id=1").json(&serde_json::json!({"name": "widget a updated"})).await.text().trim(),
        "{ ok: true, rowsAffected: 1 }"
    );

    // GET with ?id= returns just that widget
    assert_eq!(
        server.get("/crud.rhp?id=1").await.text().trim(),
        r#"[{ id: 1, name: "widget a updated" }]"#
    );

    // GET: reflects the rename
    assert_eq!(
        server.get("/crud.rhp").await.text().trim(),
        r#"[{ id: 1, name: "widget a updated" }]"#
    );

    // DELETE: remove it by id
    assert_eq!(
        server.delete("/crud.rhp?id=1").await.text().trim(),
        "{ ok: true, rowsAffected: 1 }"
    );

    // GET: empty again
    assert_eq!(server.get("/crud.rhp").await.text().trim(), "[]");
}

#[tokio::test]
async fn test_crud_sql_injection_id_neither_leaks_nor_drops() {
    let conn = unique_conn().await;
    conn.exec("CREATE TABLE widgets (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT)").run().await;
    let server = TestServer::new(build_router("./public".into(), conn));
    server.post("/crud.rhp").json(&serde_json::json!({"name": "widget a"})).await;
    server.post("/crud.rhp").json(&serde_json::json!({"name": "widget b"})).await;

    // The whole ?id= value is bound as a single literal, so none of these may
    // leak rows, delete anything, or drop the table.
    let attacks = [
        "1 OR 1=1",
        "' OR 1=1",
        "1; DROP TABLE widgets",
        "1'; DROP TABLE widgets",
        "' UNION SELECT * FROM widgets",
        "1' OR '1'='1",
    ];
    for payload in attacks {
        let uri = format!("/crud.rhp?id={}", urlencode(payload));
        assert_eq!(
            server.get(&uri).await.text().trim(),
            "[]",
            "GET {payload} leaked rows"
        );
        assert_eq!(
            server.delete(&uri).await.text().trim(),
            "{ ok: true, rowsAffected: 0 }",
            "DELETE {payload} affected rows"
        );
    }

    // Neither row was deleted and the table still exists.
    assert_eq!(
        server.get("/crud.rhp").await.text().trim(),
        r#"[{ id: 1, name: "widget a" }, { id: 2, name: "widget b" }]"#
    );
}

#[tokio::test]
async fn test_crud_sql_injection_body_name_stored_safely() {
    let conn = unique_conn().await;
    conn.exec("CREATE TABLE widgets (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT)").run().await;
    let server = TestServer::new(build_router("./public".into(), conn));

    // A hostile name is bound as data, not spliced into SQL.
    assert_eq!(
        server
            .post("/crud.rhp")
            .json(&serde_json::json!({"name": "x'); DROP TABLE widgets;--"}))
            .await
            .text()
            .trim(),
        "{ ok: true, rowsAffected: 1 }"
    );

    // The value round-trips unchanged and the table survived.
    assert_eq!(
        server.get("/crud.rhp").await.text().trim(),
        r#"[{ id: 1, name: "x'); DROP TABLE widgets;--" }]"#
    );
}

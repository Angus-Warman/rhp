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

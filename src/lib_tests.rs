use super::*;

use axum_test::TestServer;

fn test_server() -> TestServer {
    TestServer::new(build_router("./public".into()))
}

#[tokio::test]
async fn test_hello_world() {
    let server = test_server();
    let response = server.get("/hello.rhp").await;
    response.assert_status_ok();
    response.assert_text_contains("Hello\nWorld");
}

#[tokio::test]
async fn test_non_rhp() {
    let server = test_server();
    let response = server.get("/plain.html").await;
    response.assert_status_ok();
    response.assert_text_contains("Plain");
}

#[tokio::test]
async fn test_method_routing_post() {
    let server = test_server();
    let response = server.post("/methods.rhp").await;
    response.assert_status_ok();
    response.assert_text("\nthis is a post request");
}

#[tokio::test]
async fn test_method_routing_put() {
    let server = test_server();
    let response = server.put("/methods.rhp").await;
    response.assert_status_ok();
    response.assert_text("this is a put request\n");
}

#[tokio::test]
async fn test_method_routing_mismatch() {
    let server = test_server();
    let response = server.get("/methods.rhp").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(!body.contains("put"), "GET response must not include the PUT block: {body:?}");
    assert!(!body.contains("post"), "GET response must not include the POST block: {body:?}");
}
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
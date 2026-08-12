use super::*;

use axum_test::TestServer;

#[tokio::test]
async fn test_root() {
    let server = TestServer::new(build_router());
    let response = server.get("/index.rhp").await;
    response.assert_status_ok();
    response.assert_text_contains("Hello World"); // Might be slightly off?
}
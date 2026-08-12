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
async fn test_query_global() {
    let server = test_server();
    let response = server.put("/query.rhp?id=123").await;
    response.assert_status_ok();
    response.assert_text("123");
}

// #[tokio::test]
// async fn test_body_global_text() {
//     let server = test_server();
//     let response = server.post("/body.rhp").text("hello world").await;
//     response.assert_status_ok();
//     response.assert_text("{ text: hello world }");
// }

// #[tokio::test]
// async fn test_body_global_json() {
//     let server = test_server();
//     let response = server.post("/body.rhp").json(&serde_json::json!({"name": "rhp"})).await;
//     response.assert_status_ok();
//     response.assert_text("{ name: rhp }");
// }

// #[tokio::test]
// async fn test_body_global_form() {
//     let server = test_server();
//     let response = server
//         .post("/body.rhp")
//         .form(&[("color", "red"), ("color", "blue")])
//         .await;
//     response.assert_status_ok();
//     response.assert_text("{ color: [red, blue] }");
// }

// #[tokio::test]
// async fn test_body_global_get_is_empty() {
//     let server = test_server();
//     let response = server.get("/body.rhp").await;
//     response.assert_status_ok();
//     response.assert_text("{}");
// }

// #[tokio::test]
// async fn test_body_global_invalid_json_is_400() {
//     let server = test_server();
//     let response = server
//         .post("/body.rhp")
//         .bytes("{not json}".into())
//         .content_type("application/json")
//         .await;
//     response.assert_status_bad_request();
// }

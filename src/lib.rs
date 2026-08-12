use axum::{Router, extract::Request, response::Html, routing::{get, get_service}};
use tower_http::services::ServeDir;

use crate::process::process_src;


mod ast;
mod eval;
mod lexer;
mod parser;
mod value;
mod process;

pub async fn run_server() {
    let app = build_router();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    dbg!(&listener);
    axum::serve(listener, app).await.unwrap();
}

fn build_router() -> Router {
    let app = Router::new()
    .route("/{*path}.rhp", get(rhp_handler))
    .fallback_service(get_service(ServeDir::new("public")));

    app
}

async fn rhp_handler(request: Request) -> Html<String> {
    let path = "./public/".to_string() + request.uri().path();
    let src = tokio::fs::read_to_string(path).await.unwrap();
    let output = process_src(&src);
    Html(output)
}

#[cfg(test)]
#[path = "./lib_tests.rs"]
mod lib_tests;

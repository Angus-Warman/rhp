use axum::{
    Router,
    extract::Request,
    http::StatusCode,
    response::Html,
    routing::{get, get_service},
};
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
    .route("/{*path}", get(rhp_handler))
    .fallback_service(get_service(ServeDir::new("public")));

    app
}

async fn rhp_handler(request: Request) -> Result<Html<String>, StatusCode> {
    let path = "./public/".to_string() + request.uri().path();
    if !path.ends_with(".rhp") {
        return Err(StatusCode::NOT_FOUND);
    }
    let src = tokio::fs::read_to_string(path).await.map_err(|_| StatusCode::NOT_FOUND)?;
    let output = process_src(&src);
    Ok(Html(output))
}

#[cfg(test)]
#[path = "./lib_tests.rs"]
mod lib_tests;

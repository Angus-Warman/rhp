use std::path::{Path, PathBuf};

use axum::{
    Router, extract::Request, http::StatusCode,
    response::{Html, IntoResponse, Response}, routing::{any},
};
use tower::util::ServiceExt;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::process::process_src;

mod ast;
mod eval;
mod lexer;
mod parser;
mod process;
mod value;

pub async fn run_server(port: u16, folder: PathBuf, _db_conn: &str) {
    let addr = format!("0.0.0.0:{port}");
    let app = build_router();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    dbg!(&listener);
    axum::serve(listener, app).await.unwrap();
}

fn build_router() -> Router {
    let app = Router::new()
        .route("/{*path}", any(rhp_handler))
        .layer(TraceLayer::new_for_http());

    app
}

async fn rhp_handler(request: Request) -> Response {
    let path = request.uri().path();

    if Path::new(path).extension().map_or(false, |ext| ext == "rhp") {
        process_rhp(path).await.into_response()
    } else {
        ServeDir::new("public")
            .oneshot(request)
            .await
            .into_response()
    }
}

async fn process_rhp(path: &str) -> Response {
    let path = "./public/".to_string() + path;
    match tokio::fs::read_to_string(path).await {
        Ok(src) => Html(process_src(&src)).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
#[path = "./lib_tests.rs"]
mod lib_tests;

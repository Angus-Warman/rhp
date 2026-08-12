use std::path::{Path, PathBuf};

use axum::{
    Router, extract::{Request, State}, http::StatusCode, response::{Html, IntoResponse, Response}, routing::any,
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
    let app = build_router(folder);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    dbg!(&listener);
    axum::serve(listener, app).await.unwrap();
}

#[derive(Clone)]
struct AppState {
    folder: PathBuf,
}

fn build_router(folder: PathBuf) -> Router {
    let state = AppState {
        folder: folder,
    };

    let app = Router::new()
        .route("/{*path}", any(rhp_handler))
        .with_state(state)
        .layer(TraceLayer::new_for_http());

    app
}

async fn rhp_handler(State(state): State<AppState>, request: Request) -> Response {
    let path = state.folder.join(request.uri().path().trim_start_matches('/'));

    if Path::new(&path).extension().map_or(false, |ext| ext == "rhp") {
        process_rhp(path, request.method().as_str()).await.into_response()
    } else {
        ServeDir::new(state.folder)
            .oneshot(request)
            .await
            .into_response()
    }
}

async fn process_rhp(path: PathBuf, method: &str) -> Response {
    match tokio::fs::read_to_string(path).await {
        Ok(src) => Html(process_src(&src, method)).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
#[path = "./lib_tests.rs"]
mod lib_tests;

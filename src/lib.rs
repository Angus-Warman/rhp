use std::{path::{Path, PathBuf}};

use axum::{
    Router, extract::{Request, State}, http::StatusCode, response::{Html, IntoResponse, Response}, routing::any, debug_handler,
};
use tower::util::ServiceExt;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use anyhow::Result;

use crate::{db::{DbConn, connect}, process::{Context, process_src}};

mod ast;
mod db;
mod eval;
mod lexer;
mod parser;
mod process;
mod value;

pub async fn run_server(port: u16, folder: PathBuf, db_conn: &str) -> Result<()> {
    let addr = format!("0.0.0.0:{port}");
    let conn = connect(db_conn).await?;
    let app = build_router(folder, conn);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    dbg!(&listener);
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Clone)]
struct AppState {
    folder: PathBuf,
    conn: DbConn,
}

fn build_router(folder: PathBuf, conn: DbConn) -> Router {
    let state = AppState { folder, conn };

    Router::new()
        .route("/{*path}", any(rhp_handler))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

#[debug_handler]
async fn rhp_handler(State(state): State<AppState>, request: Request) -> Response {
    let path = state.folder.join(request.uri().path().trim_start_matches('/'));

    if Path::new(&path).extension().is_some_and(|ext| ext == "rhp") {
        process_rhp(path, request, state.conn).await.into_response()
    } else {
        ServeDir::new(state.folder)
            .oneshot(request)
            .await
            .into_response()
    }
}

async fn process_rhp(path: PathBuf, request: Request, conn: DbConn) -> Response {
    match tokio::fs::read_to_string(path).await {
        Ok(src) => {
            let context = match Context::from_request(request).await {
                Ok(context) => context,
                Err(e) => {
                    return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
                }
            };
            let html = process_src(src, context, conn).await;
            Html(html).into_response()
        },
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
#[path = "./lib_tests.rs"]
mod lib_tests;

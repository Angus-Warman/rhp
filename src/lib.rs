use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use axum::{
    Router, debug_handler,
    extract::{FromRequestParts, Request, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::any,
};
use tower::util::ServiceExt;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::{
    db::{DbConn, connect},
    process::{Context, process_src},
    ws::{SocketRegistry, run_socket},
};

mod ast;
mod db;
mod eval;
mod lexer;
mod parser;
mod process;
mod value;
mod ws;

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
    sockets: Arc<SocketRegistry>,
}

fn build_router(folder: PathBuf, conn: DbConn) -> Router {
    let state = AppState {
        folder,
        conn,
        sockets: Arc::new(SocketRegistry::new()),
    };

    Router::new()
        .route("/{*path}", any(rhp_handler))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

#[debug_handler]
async fn rhp_handler(State(state): State<AppState>, request: Request) -> Response {
    let path = state
        .folder
        .join(request.uri().path().trim_start_matches('/'));

    if is_ws_upgrade(&request) {
        let uri = request.uri().clone();
        let mut parts = request.into_parts().0;
        let upgrade =
            match axum::extract::ws::WebSocketUpgrade::from_request_parts(&mut parts, &state).await
            {
                Ok(upgrade) => upgrade,
                Err(_) => return StatusCode::BAD_REQUEST.into_response(),
            };
        return upgrade
            .on_upgrade(move |socket| {
                let folder = state.folder.clone();
                let conn = state.conn.clone();
                let sockets = state.sockets.clone();
                async move {
                    let path = folder.join(uri.path().trim_start_matches('/'));
                    run_socket(socket, uri, path, conn, sockets).await;
                }
            })
            .into_response();
    }

    if Path::new(&path).extension().is_some_and(|ext| ext == "rhp") {
        process_rhp(path, request, state.conn).await.into_response()
    } else {
        ServeDir::new(state.folder)
            .oneshot(request)
            .await
            .into_response()
    }
}

fn is_ws_upgrade(request: &Request) -> bool {
    let has = |name: axum::http::HeaderName| {
        request
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.to_ascii_lowercase().contains("upgrade")
                    || v.to_ascii_lowercase().contains("websocket")
            })
    };
    has(header::CONNECTION) && has(header::UPGRADE)
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
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
#[path = "./lib_tests.rs"]
mod lib_tests;

#[cfg(test)]
#[path = "./ws_tests.rs"]
mod ws_tests;

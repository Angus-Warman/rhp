use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use axum::{
    Router, debug_handler,
    extract::{FromRequestParts, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
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
        .route("/", any(rhp_handler))
        .route("/{*path}", any(rhp_handler))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

#[debug_handler]
async fn rhp_handler(State(state): State<AppState>, request: Request) -> Response {
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
                    let path = resolve_rhp(&folder, uri.path());
                    run_socket(socket, uri, path.unwrap_or(folder), conn, sockets).await;
                }
            })
            .into_response();
    }

    match resolve_rhp(&state.folder, request.uri().path()) {
        Some(rhp) => process_rhp(rhp, request, state.conn).await.into_response(),
        None => ServeDir::new(state.folder)
            .oneshot(request)
            .await
            .into_response(),
    }
}

/// Resolve a request path to the .rhp source it should execute, if any:
/// either the file itself when it ends in .rhp, or `index.rhp` when the
/// path names a directory.
fn resolve_rhp(folder: &Path, uri_path: &str) -> Option<PathBuf> {
    let path = folder.join(uri_path.trim_start_matches('/'));
    if path.extension().is_some_and(|ext| ext == "rhp") {
        Some(path)
    } else if path.is_dir() {
        let index = path.join("index.rhp");
        index.is_file().then_some(index)
    } else {
        None
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
            let (html, response) = process_src(src, context, conn).await;
            response.into_axum(html)
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

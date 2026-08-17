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
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::broadcast;
use tower::util::ServiceExt;
use tower_http::trace::TraceLayer;
use tower_http::{
    services::ServeDir,
    trace::{DefaultMakeSpan, DefaultOnRequest},
};
use tracing::Level;

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

pub async fn run_server(port: u16, folder: PathBuf, db_conn: &str, hot_reload: bool) -> Result<()> {
    let addr = format!("0.0.0.0:{port}");
    let conn = connect(db_conn).await?;

    let tx = if hot_reload {
        let watch_path = folder.canonicalize().unwrap_or_else(|_| folder.clone());
        tracing::info!("hot-reload enabled, watching {watch_path:?}");
        let tx = broadcast::Sender::new(64);
        let watcher = start_file_watcher(&watch_path, tx.clone());
        tokio::spawn(async move {
            let _watcher = watcher;
            std::future::pending::<()>().await;
        });
        Some(tx)
    } else {
        None
    };

    let app = build_router(folder, conn, tx);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    tracing::info!("listening on http://{local}");
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Clone)]
struct AppState {
    folder: PathBuf,
    conn: DbConn,
    sockets: Arc<SocketRegistry>,
    tx: Option<broadcast::Sender<String>>,
}

fn build_router(folder: PathBuf, conn: DbConn, tx: Option<broadcast::Sender<String>>) -> Router {
    let state = AppState {
        folder,
        conn,
        sockets: Arc::new(SocketRegistry::new()),
        tx,
    };

    let mut router = Router::new()
        .route("/", any(rhp_handler))
        .route("/{*path}", any(rhp_handler));

    if state.tx.is_some() {
        router = router.route("/_rhp/hot-reload", any(sse_handler));
    }

    router.with_state(state).layer(
        TraceLayer::new_for_http()
            .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
            .on_request(DefaultOnRequest::new().level(Level::INFO)),
    )
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
        Some(rhp) => process_rhp(rhp, request, state.conn, state.tx.is_some())
            .await
            .into_response(),
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

async fn process_rhp(path: PathBuf, request: Request, conn: DbConn, hot_reload: bool) -> Response {
    match tokio::fs::read_to_string(path).await {
        Ok(src) => {
            let context = match Context::from_request(request).await {
                Ok(context) => context,
                Err(e) => {
                    return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
                }
            };
            let (html, response) = process_src(src, context, conn).await;
            let html = if hot_reload {
                inject_hot_reload_script(&html)
            } else {
                html
            };
            response.into_axum(html)
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

const HOT_RELOAD_SCRIPT: &str = r#"<script>
(() => {
  const s = new EventSource("/_rhp/hot-reload");
  s.onmessage = () => { location.reload(); };
})();
</script>"#;

fn inject_hot_reload_script(html: &str) -> String {
    if let Some(pos) = html.rfind("</body>") {
        let mut out = String::with_capacity(html.len() + HOT_RELOAD_SCRIPT.len() + 1);
        out.push_str(&html[..pos]);
        out.push('\n');
        out.push_str(HOT_RELOAD_SCRIPT);
        out.push('\n');
        out.push_str(&html[pos..]);
        out
    } else {
        format!("{html}\n{HOT_RELOAD_SCRIPT}\n")
    }
}

async fn sse_handler(State(state): State<AppState>) -> Response {
    let Some(tx) = state.tx else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let rx = tx.subscribe();

    let stream = async_stream::stream! {
        let mut rx = rx;
        loop {
            match rx.recv().await {
                Ok(path) => {
                    yield Ok::<_, std::convert::Infallible>(
                        axum::response::sse::Event::default().data(path),
                    );
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    axum::response::sse::Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response()
}

fn start_file_watcher(folder: &Path, tx: broadcast::Sender<String>) -> RecommendedWatcher {
    let folder_clone = folder.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        if let Ok(event) = res {
            match event.kind {
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                    for path in &event.paths {
                        if let Ok(rel) = path.strip_prefix(&folder_clone)
                            && let Some(s) = rel.to_str()
                        {
                            let _ = tx.send(s.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    })
    .expect("failed to create file watcher");

    watcher
        .watch(folder, RecursiveMode::Recursive)
        .expect("failed to watch folder");

    watcher
}

#[cfg(test)]
#[path = "./lib_tests.rs"]
mod lib_tests;

#[cfg(test)]
#[path = "./ws_tests.rs"]
mod ws_tests;

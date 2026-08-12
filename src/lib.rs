use axum::{Router, extract::Request, response::Html, routing::get_service};
use tower_http::services::ServeDir;


mod ast;
mod eval;
mod lexer;
mod parser;
mod value;
mod process;

pub async fn run_server() {
    let app = Router::new()
        // .route("/*.rhp", get_service(rhp_handler))
        .fallback_service(get_service(ServeDir::new("public")));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    dbg!(&listener);
    axum::serve(listener, app).await.unwrap();
}
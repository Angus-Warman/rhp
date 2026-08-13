use anyhow::{Context, Result};
use std::path::PathBuf;
use clap::{Parser};
use dotenv::dotenv;

use rhp::run_server;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
    /// Port number to serve on
    #[arg(short, long, env = "PORT", default_value = "3000")]
    port: Option<u16>,

    /// Files to serve
    #[arg(short, long, env = "FOLDER", default_value = ".")]
    folder: Option<PathBuf>,

    /// Turn debugging information on
    #[arg(short, long, env = "DEBUG")]
    debug: bool,

    #[arg(long, env = "DB_CONN", default_value = ":memory:")]
    db_conn: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv()?;
    let args = Args::parse();

    let port = args.port.context("no port provided")?;
    let folder = args.folder.context("no folder provided")?;
    let db_conn = args.db_conn.context("no db_conn provided")?;

    let level = if args.debug { tracing::Level::DEBUG } else { tracing::Level::INFO };

    tracing_subscriber::fmt()
        .with_max_level(level)
        .init();

    run_server(port, folder, &db_conn).await?;
    Ok(())
}

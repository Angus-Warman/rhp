use anyhow::{Context, Result};
use clap::Parser;
use dotenv::dotenv;
use std::path::{self, PathBuf};

use rhp::run_server;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
    /// Port number to serve on
    #[arg(short, long, env = "PORT", default_value = "3000")]
    port: Option<u16>,

    /// Files to serve
    #[arg(short, long, env = "FOLDER", default_value = "./public")]
    folder: Option<PathBuf>,

    /// Turn debugging information on
    #[arg(short, long, env = "DEBUG")]
    debug: bool,

    #[arg(long, env = "DB_CONN", default_value = ":memory:")]
    db_conn: Option<String>,

    /// Enable hot-reload: watch files and auto-reload the browser on changes
    #[arg(long, env = "HOT_RELOAD")]
    watch: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok(); // No .env file? Not a problem
    let args = Args::parse();

    let port = args.port.context("no port provided")?;
    let folder = args.folder.context("no folder provided")?;
    let db_conn = args.db_conn.context("no db_conn provided")?;

    let level = if args.debug {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    tracing_subscriber::fmt().with_max_level(level).init();

    let actual_folder = path::absolute(&folder)?;
    tracing::info!("serving {actual_folder:?}");

    run_server(port, folder, &db_conn, args.watch).await?;
    Ok(())
}

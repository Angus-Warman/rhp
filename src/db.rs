use anyhow::Result;
use sqlx::AnyPool;

pub async fn ping() -> Result<String> {
    Ok("pong".to_string())
}

fn normalise_dsn(dsn: &str) -> String {
    if dsn.starts_with("postgres") {
        dsn.into()
    } else if dsn == ":memory:" {
        "sqlite::memory:".into()
    } else {
        format!("sqlite://{dsn}")
    }
}

pub async fn connect(dsn: &str) -> Result<AnyPool, sqlx::Error> {
    sqlx::any::install_default_drivers();
    AnyPool::connect(&normalise_dsn(dsn)).await
}
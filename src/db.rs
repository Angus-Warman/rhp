use anyhow::Result;
use sqlx::AnyPool;

#[derive(Clone)]
pub struct DbConn {
    pool: AnyPool
}

impl DbConn {
    pub async fn ping(&self) -> Result<String> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok("pong".to_string())
    }
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

pub async fn connect(dsn: &str) -> Result<DbConn, sqlx::Error> {
    sqlx::any::install_default_drivers();
    let pool = AnyPool::connect(&normalise_dsn(dsn)).await?;
    Ok(DbConn { pool })
}
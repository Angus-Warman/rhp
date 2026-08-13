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

    pub async fn query(&self, sql: &str) -> Result<String> { // Return String for now, will be changed later
        use sqlx::any::AnyTypeInfoKind;
        use sqlx::{Column, Row};

        let mut conn = self.pool.acquire().await?;
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql.to_string()))
            .fetch_all(&mut *conn)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut obj = serde_json::Map::new();
            for i in 0..row.len() {
                let col = row.column(i).name().to_string();
                let value: serde_json::Value = match row.column(i).type_info().kind {
                    AnyTypeInfoKind::Null => serde_json::Value::Null,
                    AnyTypeInfoKind::Bool => serde_json::to_value(row.try_get::<Option<bool>, _>(i)?)?,
                    AnyTypeInfoKind::SmallInt
                    | AnyTypeInfoKind::Integer
                    | AnyTypeInfoKind::BigInt => serde_json::to_value(row.try_get::<Option<i64>, _>(i)?)?,
                    AnyTypeInfoKind::Real => serde_json::to_value(row.try_get::<Option<f32>, _>(i)?)?,
                    AnyTypeInfoKind::Double => serde_json::to_value(row.try_get::<Option<f64>, _>(i)?)?,
                    AnyTypeInfoKind::Text => serde_json::to_value(row.try_get::<Option<String>, _>(i)?)?,
                    AnyTypeInfoKind::Blob => match row.try_get::<Option<Vec<u8>>, _>(i)? {
                        Some(bytes) => serde_json::Value::String(String::from_utf8_lossy(&bytes).into_owned()),
                        None => serde_json::Value::Null,
                    },
                };
                obj.insert(col, value);
            }
            out.push(serde_json::Value::Object(obj));
        }
        Ok(serde_json::to_string(&serde_json::Value::Array(out))?)
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
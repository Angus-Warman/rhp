use anyhow::Result;
use serde_json::{Map, Value};
use sqlx::AnyPool;

pub type Object = Map<String, Value>;

#[derive(Clone)]
pub struct DbConn {
    pool: AnyPool
}

impl DbConn {
    pub async fn ping(&self) -> Result<String> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok("pong".to_string())
    }

    /// Run a SQL statement. Always returns a Vec of objects:
    /// - statements that return rows produce one object per row
    /// - statements that don't return rows (INSERT/UPDATE/etc.) produce
    ///   a single `{ ok: true, rowsAffected: n }` object
    /// - failures produce a single `{ ok: false, error: msg }` object
    pub async fn query(&self, sql: &str) -> Vec<Object> {
        match self.run(sql).await {
            Ok(objects) => objects,
            Err(e) => vec![error_object(&e.to_string())],
        }
    }

    async fn run(&self, sql: &str) -> Result<Vec<Object>> {
        if is_query(sql) {
            self.fetch(sql).await
        } else {
            Ok(vec![self.exec(sql).await?])
        }
    }

    /// Fetch rows for a statement that returns a result set.
    async fn fetch(&self, sql: &str) -> Result<Vec<Object>> {
        use sqlx::any::AnyTypeInfoKind;
        use sqlx::{Column, Row};

        let mut conn = self.pool.acquire().await?;
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql.to_string()))
            .fetch_all(&mut *conn)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut obj = Map::new();
            for i in 0..row.len() {
                let col = row.column(i).name().to_string();
                let value: Value = match row.column(i).type_info().kind {
                    AnyTypeInfoKind::Null => Value::Null,
                    AnyTypeInfoKind::Bool => serde_json::to_value(row.try_get::<Option<bool>, _>(i)?)?,
                    AnyTypeInfoKind::SmallInt
                    | AnyTypeInfoKind::Integer
                    | AnyTypeInfoKind::BigInt => serde_json::to_value(row.try_get::<Option<i64>, _>(i)?)?,
                    AnyTypeInfoKind::Real => serde_json::to_value(row.try_get::<Option<f32>, _>(i)?)?,
                    AnyTypeInfoKind::Double => serde_json::to_value(row.try_get::<Option<f64>, _>(i)?)?,
                    AnyTypeInfoKind::Text => serde_json::to_value(row.try_get::<Option<String>, _>(i)?)?,
                    AnyTypeInfoKind::Blob => match row.try_get::<Option<Vec<u8>>, _>(i)? {
                        Some(bytes) => Value::String(String::from_utf8_lossy(&bytes).into_owned()),
                        None => Value::Null,
                    },
                };
                obj.insert(col, value);
            }
            out.push(obj);
        }
        Ok(out)
    }

    /// Run a statement that does not return rows, returning the affected count.
    async fn exec(&self, sql: &str) -> Result<Object> {
        let mut conn = self.pool.acquire().await?;
        let result = sqlx::query(sqlx::AssertSqlSafe(sql.to_string()))
            .execute(&mut *conn)
            .await?;
        let mut obj = Map::new();
        obj.insert("ok".to_string(), Value::Bool(true));
        obj.insert("rowsAffected".to_string(), Value::from(result.rows_affected()));
        Ok(obj)
    }
}

fn error_object(msg: &str) -> Object {
    let mut obj = Map::new();
    obj.insert("ok".to_string(), Value::Bool(false));
    obj.insert("error".to_string(), Value::String(msg.to_string()));
    obj
}

/// Best-effort guess of whether a statement returns rows, so we know whether
/// to fetch them or just count affected rows.
fn is_query(sql: &str) -> bool {
    let sql = strip_leading_comments(sql).trim_start();
    let first = sql.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
    matches!(
        first.as_str(),
        "SELECT" | "WITH" | "EXPLAIN" | "PRAGMA" | "SHOW" | "VALUES" | "DESCRIBE"
    ) || sql.to_ascii_uppercase().contains("RETURNING")
}

fn strip_leading_comments(sql: &str) -> &str {
    sql.trim_start()
        .strip_prefix("--")
        .and_then(|rest| rest.split_once('\n').map(|(_, after)| after))
        .map(strip_leading_comments)
        .unwrap_or(sql)
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

#[cfg(test)]
#[path = "./db_tests.rs"]
mod db_tests;

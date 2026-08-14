use anyhow::Result;
use serde_json::{Map, Value};
use sqlx::{AnyPool, Row};

pub type Object = Map<String, Value>;

#[derive(Clone)]
pub struct DbConn {
    pool: AnyPool
}

#[derive(Clone)]
pub struct QueryStmt {
    sql: String,
    pool: AnyPool,
}

#[derive(Clone)]
pub struct ExecStmt {
    sql: String,
    pool: AnyPool,
}

impl DbConn {
    pub async fn ping(&self) -> Result<String> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok("pong".to_string())
    }

    /// Lazily prepare a statement that returns rows. No I/O happens until
    /// [`QueryStmt::all`] or [`QueryStmt::one`] is called.
    pub fn query(&self, sql: &str) -> QueryStmt {
        QueryStmt { sql: sql.to_string(), pool: self.pool.clone() }
    }

    /// Lazily prepare a statement that does not return rows. No I/O happens
    /// until [`ExecStmt::run`] is called.
    pub fn exec(&self, sql: &str) -> ExecStmt {
        ExecStmt { sql: sql.to_string(), pool: self.pool.clone() }
    }
}

impl QueryStmt {
    /// Run the statement and collect every returned row.
    pub async fn all(&self) -> Vec<Object> {
        match self.fetch_all().await {
            Ok(objects) => objects,
            Err(e) => vec![error_object(&e.to_string())],
        }
    }

    /// Run the statement and return exactly one row. Fails with an error
    /// object if zero or more than one rows are returned.
    pub async fn one(&self) -> Object {
        match self.fetch_one().await {
            Ok(obj) => obj,
            Err(e) => error_object(&e.to_string()),
        }
    }

    async fn fetch_all(&self) -> Result<Vec<Object>> {
        let mut conn = self.pool.acquire().await?;
        let rows = sqlx::query(sqlx::AssertSqlSafe(self.sql.clone()))
            .fetch_all(&mut *conn)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            out.push(row_to_object(row)?);
        }
        Ok(out)
    }

    async fn fetch_one(&self) -> Result<Object> {
        let mut conn = self.pool.acquire().await?;
        let row = sqlx::query(sqlx::AssertSqlSafe(self.sql.clone()))
            .fetch_one(&mut *conn)
            .await?;
        row_to_object(&row)
    }
}

impl ExecStmt {
    /// Run the statement, returning `{ ok: true, rowsAffected: n }` on
    /// success or an error object on failure.
    pub async fn run(&self) -> Object {
        match self.execute().await {
            Ok(obj) => obj,
            Err(e) => error_object(&e.to_string()),
        }
    }

    async fn execute(&self) -> Result<Object> {
        let mut conn = self.pool.acquire().await?;
        let result = sqlx::query(sqlx::AssertSqlSafe(self.sql.clone()))
            .execute(&mut *conn)
            .await?;
        let mut obj = Map::new();
        obj.insert("ok".to_string(), Value::Bool(true));
        obj.insert("rowsAffected".to_string(), Value::from(result.rows_affected()));
        Ok(obj)
    }
}

fn row_to_object(row: &sqlx::any::AnyRow) -> Result<Object> {
    use sqlx::any::AnyTypeInfoKind;
    use sqlx::Column;

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
    Ok(obj)
}

fn error_object(msg: &str) -> Object {
    let mut obj = Map::new();
    obj.insert("ok".to_string(), Value::Bool(false));
    obj.insert("error".to_string(), Value::String(msg.to_string()));
    obj
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

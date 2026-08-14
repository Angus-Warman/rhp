use anyhow::Result;
use serde_json::{Map, Value};
use sqlx::{AnyPool, Row};

pub type Object = Map<String, Value>;

#[derive(Clone)]
pub struct DbConn {
    pool: AnyPool,
}

#[derive(Clone)]
pub struct QueryStmt {
    sql: String,
    binds: Vec<BindValue>,
    pool: AnyPool,
}

#[derive(Clone)]
pub struct ExecStmt {
    sql: String,
    binds: Vec<BindValue>,
    pool: AnyPool,
}

#[derive(Clone)]
pub struct TableStmt {
    table: String,
    pool: AnyPool,
}

/// A dynamically-typed value that can be bound to a query parameter.
#[derive(Clone)]
enum BindValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

impl BindValue {
    fn from_json(value: &Value) -> BindValue {
        match value {
            Value::Null => BindValue::Null,
            Value::Bool(b) => BindValue::Bool(*b),
            Value::Number(n) => n
                .as_i64()
                .map(BindValue::Int)
                .unwrap_or_else(|| BindValue::Float(n.as_f64().unwrap_or(0.0))),
            Value::String(s) => BindValue::Text(s.clone()),
            Value::Array(_) | Value::Object(_) => {
                BindValue::Text(serde_json::to_string(value).unwrap_or_default())
            }
        }
    }
}

impl DbConn {
    pub async fn ping(&self) -> Result<String> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok("pong".to_string())
    }

    /// Lazily prepare a statement that returns rows. No I/O happens until
    /// [`QueryStmt::all`] or [`QueryStmt::one`] is called.
    pub fn query(&self, sql: &str) -> QueryStmt {
        QueryStmt { sql: sql.to_string(), binds: Vec::new(), pool: self.pool.clone() }
    }

    /// Lazily prepare a statement that does not return rows. No I/O happens
    /// until [`ExecStmt::run`] is called.
    pub fn exec(&self, sql: &str) -> ExecStmt {
        ExecStmt { sql: sql.to_string(), binds: Vec::new(), pool: self.pool.clone() }
    }

    /// Lazily build statements targeting a single table.
    pub fn table(&self, name: &str) -> TableStmt {
        TableStmt { table: name.to_string(), pool: self.pool.clone() }
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
    /// object if zero rows are returned.
    pub async fn one(&self) -> Object {
        match self.fetch_one().await {
            Ok(obj) => obj,
            Err(e) => error_object(&e.to_string()),
        }
    }

    async fn fetch_all(&self) -> Result<Vec<Object>> {
        let mut conn = self.pool.acquire().await?;
        let mut q = sqlx::query(sqlx::AssertSqlSafe(self.sql.clone()));
        for b in &self.binds {
            q = match b {
                BindValue::Null => q.bind(None::<i32>),
                BindValue::Bool(v) => q.bind(*v),
                BindValue::Int(v) => q.bind(*v),
                BindValue::Float(v) => q.bind(*v),
                BindValue::Text(v) => q.bind(v.clone()),
            };
        }
        let rows = q.fetch_all(&mut *conn).await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            out.push(row_to_object(row)?);
        }
        Ok(out)
    }

    async fn fetch_one(&self) -> Result<Object> {
        let mut conn = self.pool.acquire().await?;
        let mut q = sqlx::query(sqlx::AssertSqlSafe(self.sql.clone()));
        for b in &self.binds {
            q = match b {
                BindValue::Null => q.bind(None::<i32>),
                BindValue::Bool(v) => q.bind(*v),
                BindValue::Int(v) => q.bind(*v),
                BindValue::Float(v) => q.bind(*v),
                BindValue::Text(v) => q.bind(v.clone()),
            };
        }
        let row = q.fetch_one(&mut *conn).await?;
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
        let mut q = sqlx::query(sqlx::AssertSqlSafe(self.sql.clone()));
        for b in &self.binds {
            q = match b {
                BindValue::Null => q.bind(None::<i32>),
                BindValue::Bool(v) => q.bind(*v),
                BindValue::Int(v) => q.bind(*v),
                BindValue::Float(v) => q.bind(*v),
                BindValue::Text(v) => q.bind(v.clone()),
            };
        }
        let result = q.execute(&mut *conn).await?;
        let mut obj = Map::new();
        obj.insert("ok".to_string(), Value::Bool(true));
        obj.insert("rowsAffected".to_string(), Value::from(result.rows_affected()));
        Ok(obj)
    }
}

impl TableStmt {
    fn query(&self, sql: String) -> QueryStmt {
        QueryStmt { sql, binds: Vec::new(), pool: self.pool.clone() }
    }

    /// `SELECT * FROM <table>` as a lazy [`QueryStmt`].
    pub fn all(&self) -> QueryStmt {
        self.query(format!("SELECT * FROM {}", quote_ident(&self.table)))
    }

    /// `SELECT * FROM <table> LIMIT 1` as a lazy [`QueryStmt`].
    pub fn one(&self) -> QueryStmt {
        self.query(format!("SELECT * FROM {} LIMIT 1", quote_ident(&self.table)))
    }

    /// Return the number of rows in the table.
    pub async fn count(&self) -> i64 {
        // sqlx's Any driver only decodes columns that have a declared type, and
        // SQLite gives aggregates/expressions no declared type, so `COUNT(*)`
        // would come back null. Count the rows directly instead.
        let rows = self.all().all().await;
        match rows.first() {
            Some(r) if r.get("ok") == Some(&Value::Bool(false)) => 0,
            _ => rows.len() as i64,
        }
    }

    /// Return one object per column: `{ name, type }`.
    pub async fn columns(&self) -> Vec<Object> {
        use sqlx::{Column, Executor, Statement, SqlSafeStr};

        // Read the declared column types straight off the prepared statement:
        // `pragma_table_info` has no declared types, so the Any driver would
        // decode its columns as null.
        let sql = format!("SELECT * FROM {} LIMIT 0", quote_ident(&self.table));
        let mut conn = match self.pool.acquire().await {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let stmt = match conn.prepare(sqlx::AssertSqlSafe(sql).into_sql_str()).await {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let mut out = Vec::with_capacity(stmt.columns().len());
        for col in stmt.columns() {
            let mut m = Map::new();
            m.insert("name".to_string(), Value::String(col.name().to_string()));
            m.insert("type".to_string(), Value::String(kind_name(col.type_info().kind).to_string()));
            out.push(m);
        }
        out
    }

    /// Build an `INSERT` from an object of column -> value.
    pub fn insert(&self, values: &Object) -> ExecStmt {
        let cols: Vec<&String> = values.keys().collect();
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            quote_ident(&self.table),
            cols.iter().map(|c| quote_ident(c)).collect::<Vec<_>>().join(", "),
            vec!["?"; cols.len()].join(", "),
        );
        ExecStmt {
            sql,
            binds: values.values().map(BindValue::from_json).collect(),
            pool: self.pool.clone(),
        }
    }

    /// Build an `UPDATE` from an object of column -> value. Applies to every
    /// row (there is no WHERE clause).
    pub fn update(&self, values: &Object) -> ExecStmt {
        let sql = format!(
            "UPDATE {} SET {}",
            quote_ident(&self.table),
            values.keys().map(|c| format!("{} = ?", quote_ident(c))).collect::<Vec<_>>().join(", "),
        );
        ExecStmt {
            sql,
            binds: values.values().map(BindValue::from_json).collect(),
            pool: self.pool.clone(),
        }
    }
}

fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Map an Any driver type kind to a SQL-ish name. sqlite reports plain
/// `INTEGER` columns as BigInt, so fold SmallInt/Integer/BigInt together.
fn kind_name(kind: sqlx::any::AnyTypeInfoKind) -> &'static str {
    use sqlx::any::AnyTypeInfoKind;
    match kind {
        AnyTypeInfoKind::Null => "NULL",
        AnyTypeInfoKind::Bool => "BOOLEAN",
        AnyTypeInfoKind::SmallInt
        | AnyTypeInfoKind::Integer
        | AnyTypeInfoKind::BigInt => "INTEGER",
        AnyTypeInfoKind::Real => "REAL",
        AnyTypeInfoKind::Double => "DOUBLE",
        AnyTypeInfoKind::Blob => "BLOB",
        AnyTypeInfoKind::Text => "TEXT",
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

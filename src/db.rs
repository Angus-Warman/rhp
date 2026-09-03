use anyhow::Result;
use serde_json::{Map, Value};
use sqlx::{Any, AnyPool, Row};
use std::sync::Arc;
use tokio::sync::Mutex;

pub type Object = Map<String, Value>;

/// The connection reserved by an active transaction, if any. Statements run
/// on it (instead of acquiring a fresh pooled connection) until commit or
/// rollback releases it back to the pool.
type TxConnection = sqlx::pool::PoolConnection<Any>;

/// A connection acquired for running one statement: either the transaction's
/// reserved connection (while its slot is locked) or a fresh pooled one.
enum Acquired<'a> {
    Tx(tokio::sync::MutexGuard<'a, Option<TxConnection>>),
    Pool(TxConnection),
}

impl<'a> Acquired<'a> {
    fn conn_mut(&mut self) -> &mut sqlx::AnyConnection {
        match self {
            Acquired::Tx(guard) => guard.as_mut().expect("transaction connection present"),
            Acquired::Pool(conn) => conn,
        }
    }
}

async fn acquire<'a>(
    pool: &'a AnyPool,
    tx: &'a Mutex<Option<TxConnection>>,
) -> anyhow::Result<Acquired<'a>> {
    let guard = tx.lock().await;
    if guard.is_some() {
        Ok(Acquired::Tx(guard))
    } else {
        Ok(Acquired::Pool(pool.acquire().await?))
    }
}

#[derive(Clone)]
pub struct DbConn {
    pool: AnyPool,
    tx: Arc<Mutex<Option<TxConnection>>>,
}

#[derive(Clone)]
pub struct QueryStmt {
    sql: String,
    binds: Vec<BindValue>,
    pool: AnyPool,
    tx: Arc<Mutex<Option<TxConnection>>>,
}

#[derive(Clone)]
pub struct ExecStmt {
    sql: String,
    binds: Vec<BindValue>,
    pool: AnyPool,
    tx: Arc<Mutex<Option<TxConnection>>>,
}

#[derive(Clone)]
pub struct TableStmt {
    table: String,
    conditions: Object,
    pool: AnyPool,
    tx: Arc<Mutex<Option<TxConnection>>>,
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
    /// Lazily prepare a statement that returns rows. No I/O happens until
    /// [`QueryStmt::all`] or [`QueryStmt::one`] is called.
    pub fn query(&self, sql: &str) -> QueryStmt {
        QueryStmt {
            sql: sql.to_string(),
            binds: Vec::new(),
            pool: self.pool.clone(),
            tx: self.tx.clone(),
        }
    }

    /// Lazily prepare a statement that does not return rows. No I/O happens
    /// until [`ExecStmt::run`] is called.
    pub fn exec(&self, sql: &str) -> ExecStmt {
        ExecStmt {
            sql: sql.to_string(),
            binds: Vec::new(),
            pool: self.pool.clone(),
            tx: self.tx.clone(),
        }
    }

    /// Lazily build statements targeting a single table.
    pub fn table(&self, name: &str) -> TableStmt {
        TableStmt {
            table: name.to_string(),
            conditions: Map::new(),
            pool: self.pool.clone(),
            tx: self.tx.clone(),
        }
    }

    /// A clone sharing the same pool but with an independent transaction slot,
    /// so each request/engine gets its own transaction state.
    pub fn isolated(&self) -> DbConn {
        DbConn {
            pool: self.pool.clone(),
            tx: Arc::new(Mutex::new(None)),
        }
    }

    /// Begin a transaction, reserving a dedicated connection that subsequent
    /// statements run on until [`DbConn::commit`] or [`DbConn::rollback`].
    pub async fn start_transaction(&self) -> Object {
        let mut guard = self.tx.lock().await;
        if guard.is_some() {
            return error_object("a transaction is already open");
        }
        match self.pool.acquire().await {
            Ok(mut conn) => match sqlx::query("BEGIN").execute(&mut *conn).await {
                Ok(_) => {
                    *guard = Some(conn);
                    ok_object()
                }
                Err(e) => error_object(&e.to_string()),
            },
            Err(e) => error_object(&e.to_string()),
        }
    }

    /// Commit the active transaction, releasing its reserved connection.
    pub async fn commit(&self) -> Object {
        let mut guard = self.tx.lock().await;
        match guard.take() {
            Some(mut conn) => match sqlx::query("COMMIT").execute(&mut *conn).await {
                Ok(_) => ok_object(),
                Err(e) => error_object(&e.to_string()),
            },
            None => error_object("no transaction to commit"),
        }
    }

    /// Roll back the active transaction, releasing its reserved connection.
    pub async fn rollback(&self) -> Object {
        let mut guard = self.tx.lock().await;
        match guard.take() {
            Some(mut conn) => match sqlx::query("ROLLBACK").execute(&mut *conn).await {
                Ok(_) => ok_object(),
                Err(e) => error_object(&e.to_string()),
            },
            None => error_object("no transaction to roll back"),
        }
    }
}

impl QueryStmt {
    /// Append a bound parameter, replacing the `?` in the SQL in order.
    pub(crate) fn bind(&self, value: &serde_json::Value) -> QueryStmt {
        let mut binds = self.binds.clone();
        binds.push(BindValue::from_json(value));
        QueryStmt {
            sql: self.sql.clone(),
            binds,
            pool: self.pool.clone(),
            tx: self.tx.clone(),
        }
    }

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
        let mut conn = acquire(&self.pool, &self.tx).await?;
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
        let rows = q.fetch_all(conn.conn_mut()).await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            out.push(row_to_object(row)?);
        }
        Ok(out)
    }

    async fn fetch_one(&self) -> Result<Object> {
        let mut conn = acquire(&self.pool, &self.tx).await?;
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
        let row = q.fetch_one(conn.conn_mut()).await?;
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
        let mut conn = acquire(&self.pool, &self.tx).await?;
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
        let result = q.execute(conn.conn_mut()).await?;
        let mut obj = Map::new();
        obj.insert("ok".to_string(), Value::Bool(true));
        obj.insert(
            "rowsAffected".to_string(),
            Value::from(result.rows_affected()),
        );
        Ok(obj)
    }
}

impl TableStmt {
    fn query(&self, sql: String, binds: Vec<BindValue>) -> QueryStmt {
        QueryStmt {
            sql,
            binds,
            pool: self.pool.clone(),
            tx: self.tx.clone(),
        }
    }

    /// Narrow the statements to rows matching every condition (`col = value`).
    /// Returns a new `TableStmt`; further calls to `where_` are ANDed together.
    pub fn where_(&self, conditions: &Object) -> TableStmt {
        let mut merged = self.conditions.clone();
        for (k, v) in conditions {
            merged.insert(k.clone(), v.clone());
        }
        TableStmt {
            table: self.table.clone(),
            conditions: merged,
            pool: self.pool.clone(),
            tx: self.tx.clone(),
        }
    }

    /// `SELECT * FROM <table> [WHERE ...]` as a lazy [`QueryStmt`].
    pub fn all(&self) -> QueryStmt {
        let (clause, binds) = where_clause(&self.conditions);
        self.query(
            format!("SELECT * FROM {}{}", quote_ident(&self.table), clause),
            binds,
        )
    }

    /// `SELECT * FROM <table> [WHERE ...] LIMIT 1` as a lazy [`QueryStmt`].
    pub fn one(&self) -> QueryStmt {
        let (clause, binds) = where_clause(&self.conditions);
        self.query(
            format!(
                "SELECT * FROM {}{} LIMIT 1",
                quote_ident(&self.table),
                clause
            ),
            binds,
        )
    }

    /// Return the number of rows in the table matching the conditions.
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
        use sqlx::{Column, Executor, SqlSafeStr, Statement};

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
            m.insert(
                "type".to_string(),
                Value::String(kind_name(col.type_info().kind).to_string()),
            );
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
            cols.iter()
                .map(|c| quote_ident(c))
                .collect::<Vec<_>>()
                .join(", "),
            vec!["?"; cols.len()].join(", "),
        );
        ExecStmt {
            sql,
            binds: values.values().map(BindValue::from_json).collect(),
            pool: self.pool.clone(),
            tx: self.tx.clone(),
        }
    }

    /// Build an `UPDATE` from an object of column -> value, restricted to the
    /// rows matching the conditions. With no conditions it applies to every row.
    pub fn update(&self, values: &Object) -> ExecStmt {
        let set = values
            .keys()
            .map(|c| format!("{} = ?", quote_ident(c)))
            .collect::<Vec<_>>()
            .join(", ");
        let (clause, where_binds) = where_clause(&self.conditions);
        let mut binds = values
            .values()
            .map(BindValue::from_json)
            .collect::<Vec<_>>();
        binds.extend(where_binds);
        ExecStmt {
            sql: format!("UPDATE {} SET {}{}", quote_ident(&self.table), set, clause),
            binds,
            pool: self.pool.clone(),
            tx: self.tx.clone(),
        }
    }

    /// Build a `DELETE` for the rows matching the conditions. With no
    /// conditions it deletes every row.
    pub fn delete(&self) -> ExecStmt {
        let (clause, binds) = where_clause(&self.conditions);
        ExecStmt {
            sql: format!("DELETE FROM {}{}", quote_ident(&self.table), clause),
            binds,
            pool: self.pool.clone(),
            tx: self.tx.clone(),
        }
    }
}

/// Build the SQL and binds for `WHERE col1 = ? AND col2 = ? ...` from an
/// object of conditions. Returns an empty clause for an empty object.
fn where_clause(conditions: &Object) -> (String, Vec<BindValue>) {
    if conditions.is_empty() {
        return (String::new(), Vec::new());
    }
    let mut binds = Vec::with_capacity(conditions.len());
    let parts = conditions
        .iter()
        .map(|(k, v)| {
            binds.push(BindValue::from_json(v));
            format!("{} = ?", quote_ident(k))
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    (format!(" WHERE {parts}"), binds)
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
        AnyTypeInfoKind::SmallInt | AnyTypeInfoKind::Integer | AnyTypeInfoKind::BigInt => "INTEGER",
        AnyTypeInfoKind::Real => "REAL",
        AnyTypeInfoKind::Double => "DOUBLE",
        AnyTypeInfoKind::Blob => "BLOB",
        AnyTypeInfoKind::Text => "TEXT",
    }
}

fn row_to_object(row: &sqlx::any::AnyRow) -> Result<Object> {
    use sqlx::Column;
    use sqlx::any::AnyTypeInfoKind;

    let mut obj = Map::new();
    for i in 0..row.len() {
        let col = row.column(i).name().to_string();
        let value: Value = match row.column(i).type_info().kind {
            AnyTypeInfoKind::Null => Value::Null,
            AnyTypeInfoKind::Bool => serde_json::to_value(row.try_get::<Option<bool>, _>(i)?)?,
            AnyTypeInfoKind::SmallInt | AnyTypeInfoKind::Integer | AnyTypeInfoKind::BigInt => {
                serde_json::to_value(row.try_get::<Option<i64>, _>(i)?)?
            }
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

fn ok_object() -> Object {
    let mut obj = Map::new();
    obj.insert("ok".to_string(), Value::Bool(true));
    obj
}

fn normalise_dsn(dsn: &str) -> String {
    if dsn.starts_with("postgres") || dsn.starts_with("sqlite") {
        // Assume the user knows what they are doing
        dsn.into()
    } else if dsn == ":memory:" {
        // A plain ":memory:" database is per-connection, so each pooled
        // connection would get its own empty database. Use a named shared
        // in-memory database so every connection lands on the same one.
        "sqlite://file%3Arhp?mode=memory&cache=shared".into()
    } else {
        // A bare path: open read-write and create the file if missing, so
        // something like "test.db" just works.
        format!("sqlite://{dsn}?mode=rwc")
    }
}

pub async fn connect(dsn: &str) -> Result<DbConn, sqlx::Error> {
    sqlx::any::install_default_drivers();
    let pool = AnyPool::connect(&normalise_dsn(dsn)).await?;
    Ok(DbConn {
        pool,
        tx: Arc::new(Mutex::new(None)),
    })
}

#[cfg(test)]
#[path = "./db_tests.rs"]
mod db_tests;

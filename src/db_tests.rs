use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

static DB_ID: AtomicU64 = AtomicU64::new(0);

async fn test_conn() -> DbConn {
    // sqlite ":memory:" is per-connection, so use a unique named shared
    // in-memory database per test to keep every pooled connection on the same db.
    let id = DB_ID.fetch_add(1, Ordering::Relaxed);
    connect(&format!("file%3Arhp_test_{id}?mode=memory&cache=shared")).await.unwrap()
}

fn val(s: &str) -> Object {
    serde_json::from_str(s).unwrap()
}

#[tokio::test]
async fn test_query_returns_rows() {
    let conn = test_conn().await;
    conn.query("CREATE TABLE users (id INTEGER, name TEXT)").await;
    conn.query("INSERT INTO users (id, name) VALUES (1, 'alice'), (2, 'bob')").await;

    let rows = conn.query("SELECT id, name FROM users ORDER BY id").await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], val(r#"{"id":1,"name":"alice"}"#));
    assert_eq!(rows[1], val(r#"{"id":2,"name":"bob"}"#));
}

#[tokio::test]
async fn test_query_null_and_numeric_types() {
    let conn = test_conn().await;
    conn.query("CREATE TABLE t (id INTEGER, score REAL, note TEXT)").await;
    conn.query("INSERT INTO t (id, score, note) VALUES (1, 2.5, NULL)").await;

    let rows = conn.query("SELECT id, score, note FROM t").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0], val(r#"{"id":1,"score":2.5,"note":null}"#));
}

#[tokio::test]
async fn test_query_empty_result() {
    let conn = test_conn().await;
    conn.query("CREATE TABLE t (id INTEGER)").await;

    let rows = conn.query("SELECT id FROM t").await;
    assert!(rows.is_empty());
}

#[tokio::test]
async fn test_query_invalid_sql_returns_error_object() {
    let conn = test_conn().await;
    let rows = conn.query("SELECT FROM WHERE").await;

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("ok"), Some(&Value::Bool(false)));
    assert!(rows[0].get("error").is_some_and(Value::is_string));
}

#[tokio::test]
async fn test_exec_insert_returns_rows_affected() {
    let conn = test_conn().await;
    conn.query("CREATE TABLE t (id INTEGER, name TEXT)").await;

    let rows = conn.query("INSERT INTO t (id, name) VALUES (1, 'a'), (2, 'b'), (3, 'c')").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("ok"), Some(&Value::Bool(true)));
    assert_eq!(rows[0].get("rowsAffected"), Some(&Value::from(3u64)));
}

#[tokio::test]
async fn test_exec_update_returns_rows_affected() {
    let conn = test_conn().await;
    conn.query("CREATE TABLE t (id INTEGER)").await;
    conn.query("INSERT INTO t (id) VALUES (1), (2)").await;

    let rows = conn.query("UPDATE t SET id = id + 1").await;
    assert_eq!(rows[0].get("ok"), Some(&Value::Bool(true)));
    assert_eq!(rows[0].get("rowsAffected"), Some(&Value::from(2u64)));
}

#[tokio::test]
async fn test_exec_delete_returns_rows_affected() {
    let conn = test_conn().await;
    conn.query("CREATE TABLE t (id INTEGER)").await;
    conn.query("INSERT INTO t (id) VALUES (1), (2), (3)").await;

    let rows = conn.query("DELETE FROM t WHERE id > 1").await;
    assert_eq!(rows[0].get("rowsAffected"), Some(&Value::from(2u64)));
}

#[tokio::test]
async fn test_exec_create_table_returns_rows_affected() {
    let conn = test_conn().await;
    let rows = conn.query("CREATE TABLE t (id INTEGER)").await;

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("ok"), Some(&Value::Bool(true)));
    assert_eq!(rows[0].get("rowsAffected"), Some(&Value::from(0u64)));
}

#[tokio::test]
async fn test_exec_invalid_sql_returns_error_object() {
    let conn = test_conn().await;
    let rows = conn.query("INSEERT INTO nope").await;

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("ok"), Some(&Value::Bool(false)));
    assert!(rows[0].get("error").is_some_and(Value::is_string));
}

#[tokio::test]
async fn test_query_after_insert_round_trip() {
    let conn = test_conn().await;
    conn.query("CREATE TABLE t (id INTEGER, name TEXT)").await;
    conn.query("INSERT INTO t (id, name) VALUES (7, 'seven')").await;

    let rows = conn.query("SELECT id, name FROM t").await;
    assert_eq!(rows, vec![val(r#"{"id":7,"name":"seven"}"#)]);
}

#[tokio::test]
async fn test_returning_treated_as_query() {
    let conn = test_conn().await;
    conn.query("CREATE TABLE t (id INTEGER)").await;

    let rows = conn.query("INSERT INTO t (id) VALUES (1) RETURNING id").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0], val(r#"{"id":1}"#));
}

#[tokio::test]
async fn test_is_query_classification() {
    assert!(is_query("SELECT * FROM t"));
    assert!(is_query("  select 1"));
    assert!(is_query("-- comment\nSELECT * FROM t"));
    assert!(is_query("WITH x AS (SELECT 1) SELECT * FROM x"));
    assert!(is_query("PRAGMA user_version"));
    assert!(is_query("INSERT INTO t (id) VALUES (1) RETURNING id"));

    assert!(!is_query("INSERT INTO t (id) VALUES (1)"));
    assert!(!is_query("UPDATE t SET id = 1"));
    assert!(!is_query("DELETE FROM t"));
    assert!(!is_query("CREATE TABLE t (id INTEGER)"));
    assert!(!is_query("DROP TABLE t"));
}

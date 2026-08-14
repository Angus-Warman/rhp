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
async fn test_query_all_returns_rows() {
    let conn = test_conn().await;
    conn.exec("CREATE TABLE users (id INTEGER, name TEXT)").run().await;
    conn.exec("INSERT INTO users (id, name) VALUES (1, 'alice'), (2, 'bob')").run().await;

    let rows = conn.query("SELECT id, name FROM users ORDER BY id").all().await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], val(r#"{"id":1,"name":"alice"}"#));
    assert_eq!(rows[1], val(r#"{"id":2,"name":"bob"}"#));
}

#[tokio::test]
async fn test_query_all_null_and_numeric_types() {
    let conn = test_conn().await;
    conn.exec("CREATE TABLE t (id INTEGER, score REAL, note TEXT)").run().await;
    conn.exec("INSERT INTO t (id, score, note) VALUES (1, 2.5, NULL)").run().await;

    let rows = conn.query("SELECT id, score, note FROM t").all().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0], val(r#"{"id":1,"score":2.5,"note":null}"#));
}

#[tokio::test]
async fn test_query_all_empty_result() {
    let conn = test_conn().await;
    conn.exec("CREATE TABLE t (id INTEGER)").run().await;

    let rows = conn.query("SELECT id FROM t").all().await;
    assert!(rows.is_empty());
}

#[tokio::test]
async fn test_query_all_invalid_sql_returns_error_object() {
    let conn = test_conn().await;
    let rows = conn.query("SELECT FROM WHERE").all().await;

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("ok"), Some(&Value::Bool(false)));
    assert!(rows[0].get("error").is_some_and(Value::is_string));
}

#[tokio::test]
async fn test_query_one_returns_single_object() {
    let conn = test_conn().await;
    conn.exec("CREATE TABLE t (id INTEGER, name TEXT)").run().await;
    conn.exec("INSERT INTO t (id, name) VALUES (7, 'seven')").run().await;

    let row = conn.query("SELECT id, name FROM t").one().await;
    assert_eq!(row, val(r#"{"id":7,"name":"seven"}"#));
}

#[tokio::test]
async fn test_query_one_no_rows_returns_error_object() {
    let conn = test_conn().await;
    conn.exec("CREATE TABLE t (id INTEGER)").run().await;

    let row = conn.query("SELECT id FROM t").one().await;
    assert_eq!(row.get("ok"), Some(&Value::Bool(false)));
    assert!(row.get("error").is_some_and(Value::is_string));
}

#[tokio::test]
async fn test_query_one_multiple_rows_returns_first_row() {
    let conn = test_conn().await;
    conn.exec("CREATE TABLE t (id INTEGER)").run().await;
    conn.exec("INSERT INTO t (id) VALUES (1), (2)").run().await;

    let row = conn.query("SELECT id FROM t ORDER BY id").one().await;
    assert_eq!(row, val(r#"{"id":1}"#));
}

#[tokio::test]
async fn test_query_one_invalid_sql_returns_error_object() {
    let conn = test_conn().await;
    let row = conn.query("SELECT FROM WHERE").one().await;

    assert_eq!(row.get("ok"), Some(&Value::Bool(false)));
    assert!(row.get("error").is_some_and(Value::is_string));
}

#[tokio::test]
async fn test_query_stmt_is_reusable() {
    let conn = test_conn().await;
    conn.exec("CREATE TABLE t (id INTEGER)").run().await;
    conn.exec("INSERT INTO t (id) VALUES (1)").run().await;

    let stmt = conn.query("SELECT id FROM t");
    assert_eq!(stmt.all().await.len(), 1);
    assert_eq!(stmt.all().await.len(), 1);
    assert_eq!(stmt.one().await, val(r#"{"id":1}"#));
}

#[tokio::test]
async fn test_exec_insert_returns_rows_affected() {
    let conn = test_conn().await;
    conn.exec("CREATE TABLE t (id INTEGER, name TEXT)").run().await;

    let obj = conn.exec("INSERT INTO t (id, name) VALUES (1, 'a'), (2, 'b'), (3, 'c')").run().await;
    assert_eq!(obj.get("ok"), Some(&Value::Bool(true)));
    assert_eq!(obj.get("rowsAffected"), Some(&Value::from(3u64)));
}

#[tokio::test]
async fn test_exec_update_returns_rows_affected() {
    let conn = test_conn().await;
    conn.exec("CREATE TABLE t (id INTEGER)").run().await;
    conn.exec("INSERT INTO t (id) VALUES (1), (2)").run().await;

    let obj = conn.exec("UPDATE t SET id = id + 1").run().await;
    assert_eq!(obj.get("ok"), Some(&Value::Bool(true)));
    assert_eq!(obj.get("rowsAffected"), Some(&Value::from(2u64)));
}

#[tokio::test]
async fn test_exec_delete_returns_rows_affected() {
    let conn = test_conn().await;
    conn.exec("CREATE TABLE t (id INTEGER)").run().await;
    conn.exec("INSERT INTO t (id) VALUES (1), (2), (3)").run().await;

    let obj = conn.exec("DELETE FROM t WHERE id > 1").run().await;
    assert_eq!(obj.get("ok"), Some(&Value::Bool(true)));
    assert_eq!(obj.get("rowsAffected"), Some(&Value::from(2u64)));
}

#[tokio::test]
async fn test_exec_create_table_returns_rows_affected() {
    let conn = test_conn().await;
    let obj = conn.exec("CREATE TABLE t (id INTEGER)").run().await;

    assert_eq!(obj.get("ok"), Some(&Value::Bool(true)));
    assert_eq!(obj.get("rowsAffected"), Some(&Value::from(0u64)));
}

#[tokio::test]
async fn test_exec_invalid_sql_returns_error_object() {
    let conn = test_conn().await;
    let obj = conn.exec("INSEERT INTO nope").run().await;

    assert_eq!(obj.get("ok"), Some(&Value::Bool(false)));
    assert!(obj.get("error").is_some_and(Value::is_string));
}

#[tokio::test]
async fn test_query_after_exec_round_trip() {
    let conn = test_conn().await;
    conn.exec("CREATE TABLE t (id INTEGER, name TEXT)").run().await;
    conn.exec("INSERT INTO t (id, name) VALUES (7, 'seven')").run().await;

    let rows = conn.query("SELECT id, name FROM t").all().await;
    assert_eq!(rows, vec![val(r#"{"id":7,"name":"seven"}"#)]);
}

#[tokio::test]
async fn test_table_all_returns_query_stmt() {
    let conn = test_conn().await;
    conn.exec("CREATE TABLE users (id INTEGER, name TEXT)").run().await;
    conn.exec("INSERT INTO users (id, name) VALUES (1, 'alice'), (2, 'bob')").run().await;

    let rows = conn.table("users").all().all().await;
    assert_eq!(rows, vec![
        val(r#"{"id":1,"name":"alice"}"#),
        val(r#"{"id":2,"name":"bob"}"#),
    ]);
}

#[tokio::test]
async fn test_table_one_returns_query_stmt() {
    let conn = test_conn().await;
    conn.exec("CREATE TABLE users (id INTEGER, name TEXT)").run().await;
    conn.exec("INSERT INTO users (id, name) VALUES (1, 'alice'), (2, 'bob')").run().await;

    let row = conn.table("users").one().one().await;
    assert_eq!(row, val(r#"{"id":1,"name":"alice"}"#));
}

#[tokio::test]
async fn test_table_count() {
    let conn = test_conn().await;
    conn.exec("CREATE TABLE users (id INTEGER)").run().await;
    conn.exec("INSERT INTO users (id) VALUES (1), (2), (3)").run().await;

    assert_eq!(conn.table("users").count().await, 3);
}

#[tokio::test]
async fn test_table_count_missing_table_returns_zero() {
    let conn = test_conn().await;
    assert_eq!(conn.table("nope").count().await, 0);
}

#[tokio::test]
async fn test_table_columns() {
    let conn = test_conn().await;
    conn.exec("CREATE TABLE users (id INTEGER, name TEXT)").run().await;

    let cols = conn.table("users").columns().await;
    assert_eq!(cols, vec![
        val(r#"{"name":"id","type":"INTEGER"}"#),
        val(r#"{"name":"name","type":"TEXT"}"#),
    ]);
}

#[tokio::test]
async fn test_table_insert() {
    let conn = test_conn().await;
    conn.exec("CREATE TABLE users (id INTEGER, name TEXT)").run().await;

    let mut obj = Map::new();
    obj.insert("id".to_string(), Value::from(1));
    obj.insert("name".to_string(), Value::from("alice"));
    let result = conn.table("users").insert(&obj).run().await;
    assert_eq!(result.get("ok"), Some(&Value::Bool(true)));
    assert_eq!(result.get("rowsAffected"), Some(&Value::from(1u64)));

    let rows = conn.table("users").all().all().await;
    assert_eq!(rows, vec![val(r#"{"id":1,"name":"alice"}"#)]);
}

#[tokio::test]
async fn test_table_update() {
    let conn = test_conn().await;
    conn.exec("CREATE TABLE users (id INTEGER, name TEXT)").run().await;
    conn.exec("INSERT INTO users (id, name) VALUES (1, 'alice'), (2, 'bob')").run().await;

    let mut obj = Map::new();
    obj.insert("name".to_string(), Value::from("renamed"));
    let result = conn.table("users").update(&obj).run().await;
    assert_eq!(result.get("ok"), Some(&Value::Bool(true)));
    assert_eq!(result.get("rowsAffected"), Some(&Value::from(2u64)));

    let rows = conn.table("users").all().all().await;
    assert_eq!(rows, vec![
        val(r#"{"id":1,"name":"renamed"}"#),
        val(r#"{"id":2,"name":"renamed"}"#),
    ]);
}

#[tokio::test]
async fn test_table_insert_binds_types() {
    let conn = test_conn().await;
    conn.exec("CREATE TABLE t (i INTEGER, f REAL, s TEXT, b INTEGER)").run().await;

    let mut obj = Map::new();
    obj.insert("i".to_string(), Value::from(42));
    obj.insert("f".to_string(), Value::from(1.5));
    obj.insert("s".to_string(), Value::from("it's a quote"));
    obj.insert("b".to_string(), Value::Bool(true));
    let result = conn.table("t").insert(&obj).run().await;
    assert_eq!(result.get("ok"), Some(&Value::Bool(true)));

    let rows = conn.query("SELECT i, f, s, b FROM t").all().await;
    assert_eq!(rows, vec![val(r#"{"i":42,"f":1.5,"s":"it's a quote","b":1}"#)]);
}

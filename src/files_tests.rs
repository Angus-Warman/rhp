use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

static DIR_ID: AtomicU64 = AtomicU64::new(0);

fn test_store() -> (FileStore, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "rhp_files_test_{}",
        DIR_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create test dir");
    (FileStore::new(dir.clone()), dir)
}

fn val(s: &str) -> Object {
    serde_json::from_str(s).expect("valid json")
}

#[tokio::test]
async fn test_write_creates_file_and_read_back() {
    let (store, dir) = test_store();
    let o = store.write("hello.txt", "Hello, files!").await;
    assert_eq!(o, val(r#"{"ok":true}"#));
    assert!(dir.join("hello.txt").is_file());
    assert_eq!(
        std::fs::read_to_string(dir.join("hello.txt")).unwrap(),
        "Hello, files!"
    );

    let o = store.read("hello.txt").await;
    assert_eq!(o, val(r#"{"ok":true,"contents":"Hello, files!"}"#));
}

#[tokio::test]
async fn test_write_creates_missing_parent_directories() {
    let (store, dir) = test_store();
    let o = store.write("nested/deep/file.txt", "deep").await;
    assert_eq!(o, val(r#"{"ok":true}"#));
    assert!(dir.join("nested/deep/file.txt").is_file());
}

#[tokio::test]
async fn test_read_missing_file_returns_error_object() {
    let (store, _dir) = test_store();
    let o = store.read("nope.txt").await;
    assert_eq!(o.get("ok"), Some(&Value::Bool(false)));
    assert!(o.get("error").is_some_and(Value::is_string));
}

#[tokio::test]
async fn test_exists() {
    let (store, _dir) = test_store();
    store.write("present.txt", "x").await;
    let o = store.exists("present.txt").await;
    assert_eq!(o, val(r#"{"ok":true,"exists":true}"#));
    let o = store.exists("absent.txt").await;
    assert_eq!(o, val(r#"{"ok":true,"exists":false}"#));
}

#[tokio::test]
async fn test_is_dir() {
    let (store, _dir) = test_store();
    store.write("sub/file.txt", "x").await;
    let o = store.is_dir("sub").await;
    assert_eq!(o, val(r#"{"ok":true,"isDir":true}"#));
    let o = store.is_dir("sub/file.txt").await;
    assert_eq!(o, val(r#"{"ok":true,"isDir":false}"#));
}

#[tokio::test]
async fn test_delete_removes_file() {
    let (store, dir) = test_store();
    store.write("temp.txt", "gone soon").await;
    let o = store.delete("temp.txt").await;
    assert_eq!(o, val(r#"{"ok":true}"#));
    assert!(!dir.join("temp.txt").exists());
}

#[tokio::test]
async fn test_delete_missing_returns_error_object() {
    let (store, _dir) = test_store();
    let o = store.delete("not-there.txt").await;
    assert_eq!(o.get("ok"), Some(&Value::Bool(false)));
    assert!(o.get("error").is_some_and(Value::is_string));
}

#[tokio::test]
async fn test_list_directory() {
    let (store, _dir) = test_store();
    store.write("b.txt", "b").await;
    store.write("a.txt", "a").await;
    store.write("sub/c.txt", "c").await;

    let o = store.list(".").await;
    assert_eq!(o.get("ok"), Some(&Value::Bool(true)));
    assert_eq!(
        o.get("entries"),
        Some(&serde_json::json!([
            {"name": "a.txt", "isDir": false},
            {"name": "b.txt", "isDir": false},
            {"name": "sub", "isDir": true},
        ]))
    );
}

#[tokio::test]
async fn test_list_empty_directory() {
    let (store, _dir) = test_store();
    store.mkdir("empty").await;
    let o = store.list("empty").await;
    assert_eq!(o, val(r#"{"ok":true,"entries":[]}"#));
}

#[tokio::test]
async fn test_list_missing_directory_returns_error_object() {
    let (store, _dir) = test_store();
    let o = store.list("missing").await;
    assert_eq!(o.get("ok"), Some(&Value::Bool(false)));
    assert!(o.get("error").is_some_and(Value::is_string));
}

#[tokio::test]
async fn test_mkdir_creates_directory() {
    let (store, dir) = test_store();
    let o = store.mkdir("brand-new").await;
    assert_eq!(o, val(r#"{"ok":true}"#));
    assert!(dir.join("brand-new").is_dir());
}

#[tokio::test]
async fn test_mkdir_creates_nested_directories() {
    let (store, dir) = test_store();
    let o = store.mkdir("a/b/c").await;
    assert_eq!(o, val(r#"{"ok":true}"#));
    assert!(dir.join("a/b/c").is_dir());
}

#[tokio::test]
async fn test_path_traversal_back_to_root_is_rejected() {
    let (store, dir) = test_store();
    let outside = std::env::temp_dir().join(format!(
        "rhp_files_outside_{}",
        DIR_ID.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&outside, "secret").expect("write outside file");
    let rel = format!("../{}", outside.file_name().unwrap().to_string_lossy());
    let o = store.read(&rel).await;
    assert_eq!(o.get("ok"), Some(&Value::Bool(false)));
    assert!(
        o.get("error")
            .is_some_and(|e| e.as_str().is_some_and(|s| s.contains("outside"))),
        "error should mention invalid path: {o:?}"
    );

    let o = store.write(&rel, "overwrite").await;
    assert_eq!(o.get("ok"), Some(&Value::Bool(false)));

    let o = store.delete(&rel).await;
    assert_eq!(o.get("ok"), Some(&Value::Bool(false)));

    // And the outside file was never touched.
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "secret",
        "traversal must not read/write/delete outside files"
    );

    // Clean up the marker file.
    std::fs::remove_file(&outside).ok();
    let _ = dir;
}

#[tokio::test]
async fn test_absolute_path_is_rejected() {
    let (store, _dir) = test_store();
    let o = store.read("/etc/hostname").await;
    assert_eq!(o.get("ok"), Some(&Value::Bool(false)));
}

#[tokio::test]
async fn test_empty_path_resolves_to_root() {
    let (store, _dir) = test_store();
    store.write("root.txt", "root").await;
    let o = store.exists("").await;
    assert_eq!(o, val(r#"{"ok":true,"exists":true}"#));

    // A leading slash is rejected outright (absolute paths are not allowed).
    let o = store.read("/root.txt").await;
    assert_eq!(o.get("ok"), Some(&Value::Bool(false)));
}

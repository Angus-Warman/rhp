use serde_json::{Map, Value};
use std::path::{Component, Path, PathBuf};

pub type Object = Map<String, Value>;

/// A handle to the configured files folder. Scripts may read, write, list and
/// delete files within this folder (and its subdirectories), but never
/// outside it.
#[derive(Clone, Debug)]
pub struct FileStore {
    root: PathBuf,
}

impl FileStore {
    /// Create a store rooted at `root`. The folder is created if missing and
    /// canonicalised so path-containment checks are lexically sound (no `..`
    /// components in the root itself).
    pub fn new(root: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&root);
        let root = root.canonicalize().unwrap_or(root);
        Self { root }
    }

    /// Resolve a script-supplied relative path against the root, rejecting any
    /// path that would escape the folder (e.g. `../`) or is absolute.
    fn resolve(&self, rel: &str) -> Option<PathBuf> {
        if Path::new(rel).is_absolute() {
            return None;
        }
        let path = self.root.join(rel);
        if path.starts_with(&self.root) && !traversal(&path) {
            Some(path)
        } else {
            None
        }
    }

    /// Read a file's contents as UTF-8 text.
    pub async fn read(&self, rel: &str) -> Object {
        match self.resolve(rel) {
            Some(path) => match tokio::fs::read_to_string(&path).await {
                Ok(contents) => {
                    let mut obj = Object::new();
                    obj.insert("ok".to_string(), Value::Bool(true));
                    obj.insert("contents".to_string(), Value::String(contents));
                    obj
                }
                Err(e) => error_object(&e.to_string()),
            },
            None => error_object("invalid path: outside configured files folder"),
        }
    }

    /// Write text content to a file, creating the file (and any missing
    /// parent directories) if needed.
    pub async fn write(&self, rel: &str, contents: &str) -> Object {
        match self.resolve(rel) {
            Some(path) => {
                if let Some(parent) = path.parent()
                    && let Err(e) = tokio::fs::create_dir_all(parent).await
                {
                    return error_object(&e.to_string());
                }
                match tokio::fs::write(&path, contents).await {
                    Ok(_) => ok_object(),
                    Err(e) => error_object(&e.to_string()),
                }
            }
            None => error_object("invalid path: outside configured files folder"),
        }
    }

    /// Return `{ ok: true, exists: bool }` for whether a path exists.
    pub async fn exists(&self, rel: &str) -> Object {
        match self.resolve(rel) {
            Some(path) => {
                let mut obj = Object::new();
                obj.insert("ok".to_string(), Value::Bool(true));
                obj.insert("exists".to_string(), Value::Bool(path.exists()));
                obj
            }
            None => error_object("invalid path: outside configured files folder"),
        }
    }

    /// Return `{ ok: true, isDir: bool }` for whether a path is a directory.
    pub async fn is_dir(&self, rel: &str) -> Object {
        match self.resolve(rel) {
            Some(path) => {
                let mut obj = Object::new();
                obj.insert("ok".to_string(), Value::Bool(true));
                obj.insert("isDir".to_string(), Value::Bool(path.is_dir()));
                obj
            }
            None => error_object("invalid path: outside configured files folder"),
        }
    }

    /// Delete a file (or empty directory). Returns an error object if the
    /// path does not exist.
    pub async fn delete(&self, rel: &str) -> Object {
        match self.resolve(rel) {
            Some(path) => {
                if !path.exists() {
                    return error_object("path does not exist");
                }
                match tokio::fs::remove_file(&path).await {
                    Ok(_) => ok_object(),
                    Err(e) => error_object(&e.to_string()),
                }
            }
            None => error_object("invalid path: outside configured files folder"),
        }
    }

    /// List the entries in a directory as an array of `{ name, isDir }`
    /// objects. Returns an error object if the path is not a directory.
    pub async fn list(&self, rel: &str) -> Object {
        match self.resolve(rel) {
            Some(path) => {
                let mut entries = match tokio::fs::read_dir(&path).await {
                    Ok(entries) => entries,
                    Err(e) => return error_object(&e.to_string()),
                };
                let mut items = Vec::new();
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let file_type = match entry.file_type().await {
                        Ok(t) => t,
                        Err(_) => continue,
                    };
                    let mut obj = Object::new();
                    obj.insert(
                        "name".to_string(),
                        Value::String(entry.file_name().to_string_lossy().into_owned()),
                    );
                    obj.insert("isDir".to_string(), Value::Bool(file_type.is_dir()));
                    items.push(Value::Object(obj));
                }
                items.sort_by(|a, b| {
                    a.as_object()
                        .and_then(|o| o.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .cmp(
                            b.as_object()
                                .and_then(|o| o.get("name"))
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                        )
                });
                let mut obj = Object::new();
                obj.insert("ok".to_string(), Value::Bool(true));
                obj.insert("entries".to_string(), Value::Array(items));
                obj
            }
            None => error_object("invalid path: outside configured files folder"),
        }
    }

    /// Create a directory (and any missing parents).
    pub async fn mkdir(&self, rel: &str) -> Object {
        match self.resolve(rel) {
            Some(path) => match tokio::fs::create_dir_all(&path).await {
                Ok(_) => ok_object(),
                Err(e) => error_object(&e.to_string()),
            },
            None => error_object("invalid path: outside configured files folder"),
        }
    }
}

/// Whether a path contains `..` traversal components (other than a single
/// leading `..` that is part of a `../` attempt).
fn traversal(path: &Path) -> bool {
    path.components().any(|c| matches!(c, Component::ParentDir))
}

fn error_object(msg: &str) -> Object {
    let mut obj = Object::new();
    obj.insert("ok".to_string(), Value::Bool(false));
    obj.insert("error".to_string(), Value::String(msg.to_string()));
    obj
}

fn ok_object() -> Object {
    let mut obj = Object::new();
    obj.insert("ok".to_string(), Value::Bool(true));
    obj
}

#[cfg(test)]
#[path = "./files_tests.rs"]
mod files_tests;

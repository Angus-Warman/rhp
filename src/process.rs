use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::extract::{Query, Request};
use axum::http::HeaderMap;
use axum::http::header::CONTENT_TYPE;

use crate::db::DbConn;
use crate::{
    eval::Evaluator,
    lexer,
    parser::Parser,
    value::{
        self, Env, Function,
        FunctionBody::{self},
        Value,
    },
};

#[derive(Debug, Clone, PartialEq)]
pub struct Context {
    pub method: Method,
    pub query: HashMap<String, String>,
    pub body: Value,
}

#[derive(Debug)]
pub enum ContextError {
    Body(axum::Error),
    Json(serde_json::Error),
    Form(serde_urlencoded::de::Error),
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContextError::Body(e) => write!(f, "reading request body: {e}"),
            ContextError::Json(e) => write!(f, "parsing json body: {e}"),
            ContextError::Form(e) => write!(f, "parsing form body: {e}"),
        }
    }
}

impl std::error::Error for ContextError {}

impl Context {
    pub async fn from_request(request: Request) -> Result<Self, ContextError> {
        let (parts, body) = request.into_parts();
        let method = Method::from_str(parts.method.as_str());
        let query = Query::<HashMap<String, String>>::try_from_uri(&parts.uri)
            .map(|q| q.0)
            .unwrap_or_default();
        let bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .map_err(ContextError::Body)?;
        let body_value = parse_body(method, &parts.headers, &bytes)?;

        Ok(Self {
            method,
            query,
            body: body_value,
        })
    }
}

impl Default for Context {
    fn default() -> Self {
        Self {
            method: Method::All,
            query: HashMap::new(),
            body: empty_object(),
        }
    }
}

fn empty_object() -> Value {
    Value::Object(Arc::new(Mutex::new(HashMap::new())))
}

fn parse_body(method: Method, headers: &HeaderMap, bytes: &[u8]) -> Result<Value, ContextError> {
    if matches!(method, Method::Get | Method::Head) || bytes.is_empty() {
        return Ok(empty_object());
    }

    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if content_type.starts_with("application/json") || content_type.ends_with("+json") {
        let json: serde_json::Value = serde_json::from_slice(bytes).map_err(ContextError::Json)?;
        return Ok(json_to_value(json));
    }

    if content_type.starts_with("application/x-www-form-urlencoded") {
        let pairs: Vec<(String, String)> =
            serde_urlencoded::from_bytes(bytes).map_err(ContextError::Form)?;
        let mut map = HashMap::new();
        for (key, value) in pairs {
            map.entry(key.clone())
                .or_insert(Value::String(value.clone()));
            let arr_key = format!("{key}s");
            match map.get_mut(&arr_key) {
                Some(Value::Array(arr)) => arr.lock().unwrap().push(Value::String(value)),
                _ => {
                    map.insert(
                        arr_key,
                        Value::Array(Arc::new(Mutex::new(vec![Value::String(value)]))),
                    );
                }
            }
        }
        return Ok(Value::Object(Arc::new(Mutex::new(map))));
    }

    // Default: treat as text
    let mut map = HashMap::new();
    map.insert(
        "text".to_string(),
        Value::String(String::from_utf8_lossy(bytes).to_string()),
    );
    Ok(Value::Object(Arc::new(Mutex::new(map))))
}

fn json_to_value(json: serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(b),
        serde_json::Value::Number(n) => Value::Number(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(s) => Value::String(s),
        serde_json::Value::Array(items) => {
            let vals = items.into_iter().map(json_to_value).collect::<Vec<_>>();
            Value::Array(Arc::new(Mutex::new(vals)))
        }
        serde_json::Value::Object(map) => {
            let vals = map
                .into_iter()
                .map(|(k, v)| (k, json_to_value(v)))
                .collect::<HashMap<_, _>>();
            Value::Object(Arc::new(Mutex::new(vals)))
        }
    }
}

pub async fn process_src(src: String, context: Context, conn: DbConn) -> String {
    let env = setup_env(&context, conn);
    let mut output = "".to_string();

    let sections = split_src(&src);

    for section in sections {
        match section {
            Section::Html(html) => output += &html,
            Section::Code { code, method } if method.matches(&context.method) => {
                let result = process_script_section(env.clone(), &code).await;
                output += &result;
            }
            Section::Code { .. } => {}
        }
    }

    output
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Patch,
    Options,
    All,
}

impl Method {
    pub fn from_str(s: &str) -> Method {
        match s.to_ascii_uppercase().as_str() {
            "GET" => Method::Get,
            "HEAD" => Method::Head,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "DELETE" => Method::Delete,
            "PATCH" => Method::Patch,
            "OPTIONS" => Method::Options,
            _ => Method::All,
        }
    }

    fn matches(&self, request_method: &Method) -> bool {
        matches!(self, Method::All) || self == request_method
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Section {
    Html(String),
    Code { code: String, method: Method },
}

fn parse_method(attrs: &str) -> Method {
    const PREFIX: &str = "method=";
    let Some(i) = attrs.find(PREFIX) else {
        return Method::All;
    };
    let rest = attrs[i + PREFIX.len()..].trim_start();
    let Some(quote) = rest.chars().next() else {
        return Method::All;
    };
    if quote != '"' && quote != '\'' {
        return Method::All;
    }
    let rest = &rest[1..];
    let Some(end) = rest.find(quote) else {
        return Method::All;
    };
    Method::from_str(&rest[..end])
}

fn split_src(src: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut rest = src;

    while !rest.is_empty() {
        match rest.find("<rhp") {
            None => {
                // No more code blocks — remainder is HTML
                sections.push(Section::Html(rest.to_string()));
                break;
            }
            Some(start) => {
                // Capture HTML before the tag
                if start > 0 {
                    sections.push(Section::Html(rest[..start].to_string()));
                }

                let after_open = &rest[start + "<rhp".len()..];
                let (method, body) = match after_open.find('>') {
                    Some(gt) => (parse_method(&after_open[..gt]), &after_open[gt + 1..]),
                    None => (Method::All, after_open),
                };

                match body.find("</rhp>") {
                    None => {
                        // Unclosed tag — treat the rest as a code block anyway,
                        // or you could return an Err here
                        sections.push(Section::Code {
                            code: body.to_string(),
                            method,
                        });
                        break;
                    }
                    Some(code_end) => {
                        sections.push(Section::Code {
                            code: body[..code_end].to_string(),
                            method,
                        });
                        rest = &body[code_end + "</rhp>".len()..];
                    }
                }
            }
        }
    }

    sections
}

fn setup_env(context: &Context, conn: DbConn) -> Arc<Mutex<Env>> {
    let env = Env::new_root();

    {
        // Scopes env_mut
        let mut env_mut = env.lock().unwrap();
        env_mut.define("VERSION", value::Value::String("0.0.1".to_string()));

        let query_map: HashMap<String, Value> = context
            .query
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        let query = Value::Object(Arc::new(Mutex::new(query_map)));
        env_mut.define("QUERY", query);

        env_mut.define("BODY", context.body.clone());

        // Define console.log
        let log = Value::Function(Function {
            params: vec!["value".to_string()],
            body: FunctionBody::Native(Arc::new(|args| {
                Box::pin(async move {
                    let output = args
                        .iter()
                        .map(|v| v.display())
                        .collect::<Vec<_>>()
                        .join(" ");
                    println!("{}", output);
                    Ok(Value::Null)
                })
            })),
            captured: Env::new_root(),
        });

        let console = Value::Object(Arc::new(Mutex::new({
            let mut map = HashMap::new();
            map.insert("log".to_string(), log);
            map
        })));

        env_mut.define("console", console);

        // Define db
        let ping_conn = conn.clone();
        let ping = Value::Function(Function {
            params: vec!["value".to_string()],
            body: FunctionBody::Native(Arc::new(move |_args| {
                let conn = ping_conn.clone();
                Box::pin(async move {
                    let res = conn.ping().await;
                    let text = res.unwrap(); // TODO: Once this is a call that can actually fail, replace this unwrap with { ok: false, error: msg }
                    Ok(Value::String(text))
                })
            })),
            captured: Env::new_root(),
        });

        let query_conn = conn.clone();
        let query = Value::Function(Function {
            params: vec!["sql".to_string()],
            body: FunctionBody::Native(Arc::new(move |args| {
                let conn = query_conn.clone();
                Box::pin(async move {
                    let sql = match args.first() {
                        Some(Value::String(s)) => s.clone(),
                        Some(other) => {
                            return Ok(Value::String(format!(
                                "DB.Query: expected a SQL string, got {}",
                                other.type_name()
                            )));
                        }
                        None => {
                            return Ok(Value::String(
                                "DB.Query: expected a SQL string".to_string(),
                            ));
                        }
                    };
                    Ok(query_stmt_to_value(conn.query(&sql)))
                })
            })),
            captured: Env::new_root(),
        });

        let exec_conn = conn.clone();
        let exec = Value::Function(Function {
            params: vec!["sql".to_string()],
            body: FunctionBody::Native(Arc::new(move |args| {
                let conn = exec_conn.clone();
                Box::pin(async move {
                    let sql = match args.first() {
                        Some(Value::String(s)) => s.clone(),
                        Some(other) => {
                            return Ok(Value::String(format!(
                                "DB.Exec: expected a SQL string, got {}",
                                other.type_name()
                            )));
                        }
                        None => {
                            return Ok(Value::String("DB.Exec: expected a SQL string".to_string()));
                        }
                    };
                    Ok(exec_stmt_to_value(conn.exec(&sql)))
                })
            })),
            captured: Env::new_root(),
        });

        let table_conn = conn.clone();
        let table = Value::Function(Function {
            params: vec!["name".to_string()],
            body: FunctionBody::Native(Arc::new(move |args| {
                let conn = table_conn.clone();
                Box::pin(async move {
                    let name = match args.first() {
                        Some(Value::String(s)) => s.clone(),
                        Some(other) => {
                            return Ok(Value::String(format!(
                                "DB.Table: expected a table name, got {}",
                                other.type_name()
                            )));
                        }
                        None => {
                            return Ok(Value::String(
                                "DB.Table: expected a table name".to_string(),
                            ));
                        }
                    };
                    Ok(table_stmt_to_value(conn.table(&name)))
                })
            })),
            captured: Env::new_root(),
        });

        let db = Value::Object(Arc::new(Mutex::new({
            let mut map = HashMap::new();
            map.insert("Ping".to_string(), ping);
            map.insert("Query".to_string(), query);
            map.insert("Exec".to_string(), exec);
            map.insert("Table".to_string(), table);
            map
        })));

        env_mut.define("DB", db);
    }

    env
}

fn query_stmt_to_value(stmt: crate::db::QueryStmt) -> Value {
    let all_stmt = stmt.clone();
    let all = Value::Function(Function {
        params: vec![],
        body: FunctionBody::Native(Arc::new(move |_args| {
            let stmt = all_stmt.clone();
            Box::pin(async move {
                let objects = stmt.all().await;
                let values = objects
                    .into_iter()
                    .map(|obj| json_to_value(serde_json::Value::Object(obj)))
                    .collect();
                Ok(Value::Array(Arc::new(Mutex::new(values))))
            })
        })),
        captured: Env::new_root(),
    });

    let one = Value::Function(Function {
        params: vec![],
        body: FunctionBody::Native(Arc::new(move |_args| {
            let stmt = stmt.clone();
            Box::pin(async move {
                let obj = stmt.one().await;
                Ok(json_to_value(serde_json::Value::Object(obj)))
            })
        })),
        captured: Env::new_root(),
    });

    let mut map = HashMap::new();
    map.insert("All".to_string(), all);
    map.insert("One".to_string(), one);
    Value::Object(Arc::new(Mutex::new(map)))
}

fn exec_stmt_to_value(stmt: crate::db::ExecStmt) -> Value {
    let run = Value::Function(Function {
        params: vec![],
        body: FunctionBody::Native(Arc::new(move |_args| {
            let stmt = stmt.clone();
            Box::pin(async move {
                let obj = stmt.run().await;
                Ok(json_to_value(serde_json::Value::Object(obj)))
            })
        })),
        captured: Env::new_root(),
    });

    let mut map = HashMap::new();
    map.insert("Run".to_string(), run);
    Value::Object(Arc::new(Mutex::new(map)))
}

fn table_stmt_to_value(stmt: crate::db::TableStmt) -> Value {
    let all_stmt = stmt.clone();
    let all = Value::Function(Function {
        params: vec![],
        body: FunctionBody::Native(Arc::new(move |_args| {
            let stmt = all_stmt.clone();
            Box::pin(async move {
                let objects = stmt.all().all().await;
                let values = objects
                    .into_iter()
                    .map(|obj| json_to_value(serde_json::Value::Object(obj)))
                    .collect();
                Ok(Value::Array(Arc::new(Mutex::new(values))))
            })
        })),
        captured: Env::new_root(),
    });

    let one_stmt = stmt.clone();
    let one = Value::Function(Function {
        params: vec![],
        body: FunctionBody::Native(Arc::new(move |_args| {
            let stmt = one_stmt.clone();
            Box::pin(async move {
                let obj = stmt.one().one().await;
                Ok(json_to_value(serde_json::Value::Object(obj)))
            })
        })),
        captured: Env::new_root(),
    });

    let count_stmt = stmt.clone();
    let count = Value::Function(Function {
        params: vec![],
        body: FunctionBody::Native(Arc::new(move |_args| {
            let stmt = count_stmt.clone();
            Box::pin(async move { Ok(Value::Number(stmt.count().await as f64)) })
        })),
        captured: Env::new_root(),
    });

    let columns_stmt = stmt.clone();
    let columns = Value::Function(Function {
        params: vec![],
        body: FunctionBody::Native(Arc::new(move |_args| {
            let stmt = columns_stmt.clone();
            Box::pin(async move {
                let objects = stmt.columns().await;
                let values = objects
                    .into_iter()
                    .map(|obj| json_to_value(serde_json::Value::Object(obj)))
                    .collect();
                Ok(Value::Array(Arc::new(Mutex::new(values))))
            })
        })),
        captured: Env::new_root(),
    });

    let insert_stmt = stmt.clone();
    let insert = Value::Function(Function {
        params: vec!["object".to_string()],
        body: FunctionBody::Native(Arc::new(move |args| {
            let stmt = insert_stmt.clone();
            Box::pin(async move {
                let obj = match args.first().and_then(value_to_object) {
                    Some(obj) => obj,
                    None => {
                        return Ok(Value::String(
                            "TableStmt.Insert: expected an object".to_string(),
                        ));
                    }
                };
                Ok(exec_stmt_to_value(stmt.insert(&obj)))
            })
        })),
        captured: Env::new_root(),
    });

    let update_stmt = stmt.clone();
    let update = Value::Function(Function {
        params: vec!["object".to_string()],
        body: FunctionBody::Native(Arc::new(move |args| {
            let stmt = update_stmt.clone();
            Box::pin(async move {
                let obj = match args.first().and_then(value_to_object) {
                    Some(obj) => obj,
                    None => {
                        return Ok(Value::String(
                            "TableStmt.Update: expected an object".to_string(),
                        ));
                    }
                };
                Ok(exec_stmt_to_value(stmt.update(&obj)))
            })
        })),
        captured: Env::new_root(),
    });

    let where_stmt = stmt.clone();
    let where_fn = Value::Function(Function {
        params: vec!["conditions".to_string()],
        body: FunctionBody::Native(Arc::new(move |args| {
            let stmt = where_stmt.clone();
            Box::pin(async move {
                let conditions = match args.first().and_then(value_to_object) {
                    Some(obj) => obj,
                    None => {
                        return Ok(Value::String(
                            "TableStmt.Where: expected an object".to_string(),
                        ));
                    }
                };
                Ok(table_stmt_to_value(stmt.where_(&conditions)))
            })
        })),
        captured: Env::new_root(),
    });

    let delete_stmt = stmt.clone();
    let delete = Value::Function(Function {
        params: vec![],
        body: FunctionBody::Native(Arc::new(move |_args| {
            let stmt = delete_stmt.clone();
            Box::pin(async move { Ok(exec_stmt_to_value(stmt.delete())) })
        })),
        captured: Env::new_root(),
    });

    let mut map = HashMap::new();
    map.insert("All".to_string(), all);
    map.insert("One".to_string(), one);
    map.insert("Count".to_string(), count);
    map.insert("Columns".to_string(), columns);
    map.insert("Insert".to_string(), insert);
    map.insert("Update".to_string(), update);
    map.insert("Where".to_string(), where_fn);
    map.insert("Delete".to_string(), delete);
    Value::Object(Arc::new(Mutex::new(map)))
}

fn value_to_object(v: &Value) -> Option<serde_json::Map<String, serde_json::Value>> {
    match v {
        Value::Object(o) => {
            let lock = o.lock().unwrap();
            Some(
                lock.iter()
                    .map(|(k, val)| (k.clone(), value_to_json(val)))
                    .collect(),
            )
        }
        _ => None,
    }
}

fn value_to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Number(n) => serde_json::Number::from_f64(*n)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Array(a) => {
            serde_json::Value::Array(a.lock().unwrap().iter().map(value_to_json).collect())
        }
        Value::Object(o) => {
            let lock = o.lock().unwrap();
            let mut map = serde_json::Map::new();
            for (k, v) in lock.iter() {
                map.insert(k.clone(), value_to_json(v));
            }
            serde_json::Value::Object(map)
        }
        Value::Function(_) => serde_json::Value::Null,
    }
}

async fn process_script_section(env: Arc<Mutex<Env>>, script: &str) -> String {
    let tokens = lexer::lex_code(script).unwrap();
    let (stmts, _) = Parser::parse(tokens);
    let mut evalulator = Evaluator::new();

    evalulator.eval_stmts(&stmts, env).await.unwrap();
    evalulator.output
}

#[cfg(test)]
#[path = "./process_tests.rs"]
mod process_tests;

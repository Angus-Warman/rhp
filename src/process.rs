use std::{cell::RefCell, collections::HashMap, rc::Rc};

use axum::extract::{Query, Request};
use axum::http::header::CONTENT_TYPE;
use axum::http::HeaderMap;

use crate::{eval::Evaluator, lexer, parser::Parser, value::{
    self, Env, Function, FunctionBody::{self}, Value,
}};

#[derive(Debug, Clone, PartialEq)]
pub struct Context {
    pub method: Method,
    pub query:  HashMap<String, String>,
    pub body:   Value,
}

#[derive(Debug)]
pub enum ContextError {
    Body(axum::Error),
    Json(serde_json::Error),
    Form(serde_urlencoded::de::Error),
}

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

        Ok(Self { method, query, body: body_value })
    }
}

impl Default for Context {
    fn default() -> Self {
        Self {
            method: Method::All,
            query:  HashMap::new(),
            body:   empty_object(),
        }
    }
}

fn empty_object() -> Value {
    Value::Object(Rc::new(RefCell::new(HashMap::new())))
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
            map.entry(key.clone()).or_insert(Value::String(value.clone()));
            let arr_key = format!("{key}s");
            match map.get_mut(&arr_key) {
                Some(Value::Array(arr)) => arr.borrow_mut().push(Value::String(value)),
                _ => {
                    map.insert(arr_key, Value::Array(Rc::new(RefCell::new(vec![Value::String(value)]))));
                }
            }
        }
        return Ok(Value::Object(Rc::new(RefCell::new(map))));
    }

    // Default: treat as text
    let mut map = HashMap::new();
    map.insert("text".to_string(), Value::String(String::from_utf8_lossy(bytes).to_string()));
    Ok(Value::Object(Rc::new(RefCell::new(map))))
}

fn json_to_value(json: serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null    => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(b),
        serde_json::Value::Number(n) => Value::Number(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(s) => Value::String(s),
        serde_json::Value::Array(items) => {
            let vals = items.into_iter().map(json_to_value).collect::<Vec<_>>();
            Value::Array(Rc::new(RefCell::new(vals)))
        }
        serde_json::Value::Object(map) => {
            let vals = map.into_iter()
                .map(|(k, v)| (k, json_to_value(v)))
                .collect::<HashMap<_, _>>();
            Value::Object(Rc::new(RefCell::new(vals)))
        }
    }
}

pub fn process_src(src: &str, context: &Context) -> String {
    let env = setup_env(context);
    let mut output = "".to_string();

    let sections = split_src(src);

    for section in sections {
        match section {
            Section::Html(html) => output += &html,
            Section::Code { code, method } if method.matches(&context.method) => {
                let result = process_script_section(env.clone(), &code);
                output += &result;
            },
            Section::Code { .. } => {},
        }
    }

    return output;
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
            "GET"     => Method::Get,
            "HEAD"    => Method::Head,
            "POST"    => Method::Post,
            "PUT"     => Method::Put,
            "DELETE"  => Method::Delete,
            "PATCH"   => Method::Patch,
            "OPTIONS" => Method::Options,
            _         => Method::All,
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
                        sections.push(Section::Code { code: body.to_string(), method });
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

fn setup_env(context: &Context) -> Rc<RefCell<Env>> {
    let env = Env::new_root();

    { // Scopes env_mut
        let mut env_mut = env.borrow_mut();
        env_mut.define("VERSION", value::Value::String("0.0.1".to_string()));

        let query_map: HashMap<String, Value> = context.query.iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        let query = Value::Object(Rc::new(RefCell::new(query_map)));
        env_mut.define("QUERY", query);

        env_mut.define("BODY", context.body.clone());

        let log = Value::Function(Function {
            params: vec!["value".to_string()],
            body: FunctionBody::Native(Rc::new(|args| {
                let output = args
                    .iter()
                    .map(|v| v.display())
                    .collect::<Vec<_>>()
                    .join(" ");
                println!("{}", output);
                Ok(Value::Null)
            })),
            captured: Env::new_root(),
        });

        let console = Value::Object(Rc::new(RefCell::new({
            let mut map = HashMap::new();
            map.insert("log".to_string(), log);
            map
        })));

        env_mut.define("console", console);
    }

    env
}

fn process_script_section(env: Rc<RefCell<Env>>, script: &str) -> String {
    let tokens = lexer::lex_code(script).unwrap();
    let (stmts, _) = Parser::parse(tokens);
    let mut evalulator = Evaluator::new();
    

    evalulator.eval_stmts(&stmts, env).unwrap();
    evalulator.output
}

#[cfg(test)]
#[path = "./process_tests.rs"]
mod process_tests;

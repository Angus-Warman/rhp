use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::extract::{Query, Request};
use axum::http::HeaderMap;
use axum::http::header::CONTENT_TYPE;

use crate::db::DbConn;
use crate::ws::SocketRef;
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
    pub headers: HashMap<String, String>,
    pub body: Value,
    pub socket: Option<SocketRef>,
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

        let headers = parts
            .headers
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|v| (name.as_str().to_ascii_lowercase(), v.to_string()))
            })
            .collect();

        Ok(Self {
            method,
            query,
            headers,
            body: body_value,
            socket: None,
        })
    }
}

impl Default for Context {
    fn default() -> Self {
        Self {
            method: Method::All,
            query: HashMap::new(),
            headers: HashMap::new(),
            body: empty_object(),
            socket: None,
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

pub async fn process_src(src: String, context: Context, conn: DbConn) -> (String, HttpResponse) {
    let env = setup_env(&context, conn);
    let mut output = "".to_string();

    let sections = split_src(&src);

    for section in sections {
        match section {
            Section::Html(html) => output += &html,
            Section::Code { code, method } if method.matches(&context.method) => {
                let result = process_script_section(env.clone(), &code).await;
                output += &result;
                // A script that sent a full body or a redirect owns the
                // response; stop rendering further sections.
                if response_owns_body(&env) {
                    break;
                }
            }
            Section::Code { .. } => {}
        }
    }

    let response = HttpResponse::from_env(&env);
    (output, response)
}

/// True once the script set a body or redirect via RES.
fn response_owns_body(env: &Arc<Mutex<Env>>) -> bool {
    let Some(Value::Object(map)) = env.lock().unwrap().get("RES") else {
        return false;
    };
    let lock = map.lock().unwrap();
    lock.contains_key("_body") || lock.contains_key("_redirect")
}

/// Run the `<rhp method="SOCKET">` sections for a websocket connection and
/// return the value the first section returns with `return` (if any). The
/// returned value becomes the first message sent on the socket.
pub async fn process_socket_src(src: String, context: Context, conn: DbConn) -> Option<Value> {
    let env = setup_env(&context, conn);

    for section in split_src(&src) {
        if let Section::Code { code, method } = section
            && method.matches(&context.method)
        {
            let tokens = lexer::lex_code(&code).unwrap();
            let (stmts, _) = Parser::parse(tokens, &code);
            let mut evaluator = Evaluator::new();
            let _ = evaluator.eval_stmts(&stmts, env.clone()).await;
            if let Some(returned) = evaluator.returned
                && !matches!(returned, Value::Null)
            {
                return Some(returned);
            }
        }
    }

    None
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
    Socket,
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
            "SOCKET" => Method::Socket,
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

/// Script-controlled HTTP response state, populated by the `RES` object.
#[derive(Debug, Clone, Default)]
pub struct HttpResponse {
    pub status: Option<u16>,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub content_type: Option<String>,
    pub redirect: Option<String>,
}

impl HttpResponse {
    /// Read the response state written by the script into the `RES`
    /// global, if any.
    pub fn from_env(env: &Arc<Mutex<Env>>) -> Self {
        let mut response = Self::default();
        let Some(Value::Object(map)) = env.lock().unwrap().get("RES") else {
            return response;
        };
        let lock = map.lock().unwrap();
        if let Some(Value::Number(n)) = lock.get("_status") {
            response.status = Some(*n as u16);
        }
        if let Some(Value::String(url)) = lock.get("_redirect") {
            response.redirect = Some(url.clone());
        }
        if let Some(Value::String(body)) = lock.get("_body") {
            response.body = Some(body.clone());
        }
        if let Some(Value::String(ct)) = lock.get("_content_type") {
            response.content_type = Some(ct.clone());
        }
        if let Some(Value::Object(headers)) = lock.get("_headers") {
            let header_lock = headers.lock().unwrap();
            let mut pairs: Vec<(String, String)> = header_lock
                .iter()
                .filter_map(|(k, v)| match v {
                    Value::String(s) => Some((k.clone(), s.clone())),
                    _ => None,
                })
                .collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            response.headers = pairs;
        }
        if let Some(Value::Array(cookies)) = lock.get("_cookies") {
            let cookie_lock = cookies.lock().unwrap();
            for cookie in cookie_lock.iter() {
                if let Value::String(c) = cookie {
                    response.headers.push(("set-cookie".to_string(), c.clone()));
                }
            }
        }
        response
    }

    /// Build the final axum response from this state and the rendered body.
    pub fn into_axum(self, rendered: String) -> axum::response::Response {
        use axum::http::header;
        let status = self.status.unwrap_or(200);
        let mut builder = axum::response::Response::builder().status(status);

        if let Some(url) = &self.redirect {
            builder = builder.header(header::LOCATION, url);
        }
        for (name, value) in &self.headers {
            builder = builder.header(name.as_str(), value);
        }
        let has_content_type = self
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-type"));
        if !has_content_type && let Some(ct) = &self.content_type {
            builder = builder.header(header::CONTENT_TYPE, ct);
        }

        let body = self.body.unwrap_or(rendered);
        builder
            .body(axum::body::Body::from(body))
            .unwrap_or_else(|_| axum::response::Response::default())
    }
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

        // Expose request headers (lower-cased names) as HEADER.<name>
        let header_map: HashMap<String, Value> = context
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        let headers = Value::Object(Arc::new(Mutex::new(header_map)));
        env_mut.define("HEADER", headers);

        // Parse the `cookie` request header into COOKIE.<name>
        let cookie_map: HashMap<String, Value> = context
            .headers
            .get("cookie")
            .map(|raw| {
                raw.split(';')
                    .filter_map(|part| {
                        let (name, value) = part.trim().split_once('=')?;
                        Some((name.to_string(), Value::String(value.to_string())))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let cookies = Value::Object(Arc::new(Mutex::new(cookie_map)));
        env_mut.define("COOKIE", cookies);

        // Define JSON.Parse / JSON.Stringify
        let json_parse = Value::Function(Function {
            params: vec!["text".to_string()],
            body: FunctionBody::Native(Arc::new(|args| {
                Box::pin(async move {
                    let text = match args.first() {
                        Some(Value::String(s)) => s.clone(),
                        Some(other) => {
                            return Ok(error_value(format!(
                                "JSON.Parse: expected a JSON string, got {}",
                                other.type_name()
                            )));
                        }
                        None => {
                            return Ok(error_value("JSON.Parse: expected a JSON string"));
                        }
                    };
                    match serde_json::from_str::<serde_json::Value>(&text) {
                        Ok(json) => Ok(json_to_value(json)),
                        Err(e) => Ok(error_value(format!("JSON.Parse: {}", e))),
                    }
                })
            })),
            captured: Env::new_root(),
        });

        let json_stringify = Value::Function(Function {
            params: vec!["value".to_string()],
            body: FunctionBody::Native(Arc::new(|args| {
                Box::pin(async move {
                    let value = match args.first() {
                        Some(v) => v.clone(),
                        None => {
                            return Ok(error_value("JSON.Stringify: expected a value"));
                        }
                    };
                    match serde_json::to_string(&value_to_json(&value)) {
                        Ok(text) => Ok(Value::String(text)),
                        Err(e) => Ok(error_value(format!("JSON.Stringify: {}", e))),
                    }
                })
            })),
            captured: Env::new_root(),
        });

        let json = Value::Object(Arc::new(Mutex::new({
            let mut map = HashMap::new();
            map.insert("Parse".to_string(), json_parse);
            map.insert("Stringify".to_string(), json_stringify);
            map
        })));

        env_mut.define("JSON", json);

        // Define TIME.Unix_Sec / Unix_Ms / Unix_Ns
        let time_sec = Value::Function(Function {
            params: vec![],
            body: FunctionBody::Native(Arc::new(|_args| {
                Box::pin(async move { Ok(Value::Number(unix_time().as_secs() as f64)) })
            })),
            captured: Env::new_root(),
        });
        let time_ms = Value::Function(Function {
            params: vec![],
            body: FunctionBody::Native(Arc::new(|_args| {
                Box::pin(async move { Ok(Value::Number(unix_time().as_millis() as f64)) })
            })),
            captured: Env::new_root(),
        });
        let time_ns = Value::Function(Function {
            params: vec![],
            body: FunctionBody::Native(Arc::new(|_args| {
                Box::pin(async move { Ok(Value::Number(unix_time().as_nanos() as f64)) })
            })),
            captured: Env::new_root(),
        });

        let time = Value::Object(Arc::new(Mutex::new({
            let mut map = HashMap::new();
            map.insert("Unix_Sec".to_string(), time_sec);
            map.insert("Unix_Ms".to_string(), time_ms);
            map.insert("Unix_Ns".to_string(), time_ns);
            map
        })));

        env_mut.define("TIME", time);

        // Define MATH.Random / Ceil / Floor / Avg / Sum / Min / Max
        let math_random = Value::Function(Function {
            params: vec![],
            body: FunctionBody::Native(Arc::new(|_args| {
                Box::pin(async move { Ok(Value::Number(random_f64())) })
            })),
            captured: Env::new_root(),
        });

        let math_ceil = Value::Function(Function {
            params: vec![],
            body: FunctionBody::Native(Arc::new(|args| {
                Box::pin(async move {
                    match args.first() {
                        Some(Value::Number(n)) => Ok(Value::Number(n.ceil())),
                        Some(other) => Ok(error_value(format!(
                            "MATH.Ceil: expected a number, got {}",
                            other.type_name()
                        ))),
                        None => Ok(error_value("MATH.Ceil: expected a number")),
                    }
                })
            })),
            captured: Env::new_root(),
        });

        let math_floor = Value::Function(Function {
            params: vec![],
            body: FunctionBody::Native(Arc::new(|args| {
                Box::pin(async move {
                    match args.first() {
                        Some(Value::Number(n)) => Ok(Value::Number(n.floor())),
                        Some(other) => Ok(error_value(format!(
                            "MATH.Floor: expected a number, got {}",
                            other.type_name()
                        ))),
                        None => Ok(error_value("MATH.Floor: expected a number")),
                    }
                })
            })),
            captured: Env::new_root(),
        });

        let math_avg = Value::Function(Function {
            params: vec![],
            body: FunctionBody::Native(Arc::new(|args| {
                Box::pin(async move {
                    match math_numbers(args) {
                        Ok(nums) if nums.is_empty() => {
                            Ok(error_value("MATH.Avg: expected at least one number"))
                        }
                        Ok(nums) => Ok(Value::Number(nums.iter().sum::<f64>() / nums.len() as f64)),
                        Err(msg) => Ok(error_value(format!("MATH.Avg: {msg}"))),
                    }
                })
            })),
            captured: Env::new_root(),
        });

        let math_sum = Value::Function(Function {
            params: vec![],
            body: FunctionBody::Native(Arc::new(|args| {
                Box::pin(async move {
                    match math_numbers(args) {
                        Ok(nums) => Ok(Value::Number(nums.iter().sum())),
                        Err(msg) => Ok(error_value(format!("MATH.Sum: {msg}"))),
                    }
                })
            })),
            captured: Env::new_root(),
        });

        let math_min = Value::Function(Function {
            params: vec![],
            body: FunctionBody::Native(Arc::new(|args| {
                Box::pin(async move {
                    match math_numbers(args) {
                        Ok(nums) if nums.is_empty() => {
                            Ok(error_value("MATH.Min: expected at least one number"))
                        }
                        Ok(nums) => Ok(Value::Number(
                            nums.into_iter().fold(f64::INFINITY, f64::min),
                        )),
                        Err(msg) => Ok(error_value(format!("MATH.Min: {msg}"))),
                    }
                })
            })),
            captured: Env::new_root(),
        });

        let math_max = Value::Function(Function {
            params: vec![],
            body: FunctionBody::Native(Arc::new(|args| {
                Box::pin(async move {
                    match math_numbers(args) {
                        Ok(nums) if nums.is_empty() => {
                            Ok(error_value("MATH.Max: expected at least one number"))
                        }
                        Ok(nums) => Ok(Value::Number(
                            nums.into_iter().fold(f64::NEG_INFINITY, f64::max),
                        )),
                        Err(msg) => Ok(error_value(format!("MATH.Max: {msg}"))),
                    }
                })
            })),
            captured: Env::new_root(),
        });

        let math = Value::Object(Arc::new(Mutex::new({
            let mut map = HashMap::new();
            map.insert("Random".to_string(), math_random);
            map.insert("Ceil".to_string(), math_ceil);
            map.insert("Floor".to_string(), math_floor);
            map.insert("Avg".to_string(), math_avg);
            map.insert("Sum".to_string(), math_sum);
            map.insert("Min".to_string(), math_min);
            map.insert("Max".to_string(), math_max);
            map
        })));

        env_mut.define("MATH", math);

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

        // Define RES: script-controlled status/headers/body/redirect.
        // Methods write into a shared map that `process_src` reads afterwards.
        let response_map = Arc::new(Mutex::new(HashMap::new()));

        let response_set_status = {
            let map = response_map.clone();
            Value::Function(Function {
                params: vec!["status".to_string()],
                body: FunctionBody::Native(Arc::new(move |args| {
                    let map = map.clone();
                    Box::pin(async move {
                        match args.first() {
                            Some(Value::Number(n)) if *n >= 100.0 && *n <= 599.0 => {
                                map.lock()
                                    .unwrap()
                                    .insert("_status".to_string(), Value::Number(*n));
                                Ok(Value::Null)
                            }
                            Some(other) => Ok(error_value(format!(
                                "RES.SetStatus: expected a status code, got {}",
                                other.type_name()
                            ))),
                            None => Ok(error_value("RES.SetStatus: expected a status code")),
                        }
                    })
                })),
                captured: Env::new_root(),
            })
        };

        let response_set_header = {
            let map = response_map.clone();
            Value::Function(Function {
                params: vec!["name".to_string(), "value".to_string()],
                body: FunctionBody::Native(Arc::new(move |args| {
                    let map = map.clone();
                    Box::pin(async move {
                        match (args.first(), args.get(1)) {
                            (Some(Value::String(name)), Some(Value::String(value))) => {
                                let mut lock = map.lock().unwrap();
                                let headers =
                                    lock.entry("_headers".to_string()).or_insert_with(|| {
                                        Value::Object(Arc::new(Mutex::new(HashMap::new())))
                                    });
                                if let Value::Object(h) = headers {
                                    h.lock()
                                        .unwrap()
                                        .insert(name.clone(), Value::String(value.clone()));
                                }
                                Ok(Value::Null)
                            }
                            _ => Ok(error_value(
                                "RES.SetHeader: expected (name, value) strings",
                            )),
                        }
                    })
                })),
                captured: Env::new_root(),
            })
        };

        let response_set_cookie = {
            let map = response_map.clone();
            Value::Function(Function {
                params: vec!["name".to_string(), "value".to_string()],
                body: FunctionBody::Native(Arc::new(move |args| {
                    let map = map.clone();
                    Box::pin(async move {
                        match (args.first(), args.get(1)) {
                            (Some(Value::String(name)), Some(Value::String(value))) => {
                                let opts = args.get(2).and_then(|v| match v {
                                    Value::Object(o) => {
                                        let lock = o.lock().unwrap();
                                        let map: HashMap<String, Value> = lock.clone();
                                        Some(map)
                                    }
                                    _ => None,
                                });
                                let mut parts = vec![format!("{name}={value}")];
                                if let Some(opts) = opts {
                                    if let Some(Value::String(p)) = opts.get("Path") {
                                        parts.push(format!("Path={p}"));
                                    }
                                    if let Some(Value::String(m)) = opts.get("MaxAge") {
                                        parts.push(format!("Max-Age={m}"));
                                    }
                                    if opts.get("HttpOnly").is_some_and(Value::is_truthy) {
                                        parts.push("HttpOnly".to_string());
                                    }
                                    if opts.get("Secure").is_some_and(Value::is_truthy) {
                                        parts.push("Secure".to_string());
                                    }
                                    if let Some(Value::String(s)) = opts.get("SameSite") {
                                        parts.push(format!("SameSite={s}"));
                                    }
                                }
                                let mut lock = map.lock().unwrap();
                                let cookies = lock
                                    .entry("_cookies".to_string())
                                    .or_insert_with(|| Value::Array(Arc::new(Mutex::new(vec![]))));
                                if let Value::Array(c) = cookies {
                                    c.lock().unwrap().push(Value::String(parts.join("; ")));
                                }
                                Ok(Value::Null)
                            }
                            _ => Ok(error_value(
                                "RES.SetCookie: expected (name, value) strings",
                            )),
                        }
                    })
                })),
                captured: Env::new_root(),
            })
        };

        let response_json = {
            let map = response_map.clone();
            Value::Function(Function {
                params: vec!["value".to_string()],
                body: FunctionBody::Native(Arc::new(move |args| {
                    let map = map.clone();
                    Box::pin(async move {
                        let Some(value) = args.first() else {
                            return Ok(error_value("RES.Json: expected a value"));
                        };
                        match serde_json::to_string(&value_to_json(value)) {
                            Ok(text) => {
                                let mut lock = map.lock().unwrap();
                                lock.insert("_body".to_string(), Value::String(text));
                                lock.insert(
                                    "_content_type".to_string(),
                                    Value::String("application/json".to_string()),
                                );
                                Ok(Value::Null)
                            }
                            Err(e) => Ok(error_value(format!("RES.Json: {e}"))),
                        }
                    })
                })),
                captured: Env::new_root(),
            })
        };

        let response_html = {
            let map = response_map.clone();
            Value::Function(Function {
                params: vec!["html".to_string()],
                body: FunctionBody::Native(Arc::new(move |args| {
                    let map = map.clone();
                    Box::pin(async move {
                        let html = match args.first() {
                            Some(Value::String(s)) => s.clone(),
                            Some(other) => {
                                return Ok(error_value(format!(
                                    "RES.Html: expected a string, got {}",
                                    other.type_name()
                                )));
                            }
                            None => {
                                return Ok(error_value("RES.Html: expected a string"));
                            }
                        };
                        let mut lock = map.lock().unwrap();
                        lock.insert("_body".to_string(), Value::String(html));
                        lock.insert(
                            "_content_type".to_string(),
                            Value::String("text/html".to_string()),
                        );
                        Ok(Value::Null)
                    })
                })),
                captured: Env::new_root(),
            })
        };

        let response_redirect = {
            let map = response_map.clone();
            Value::Function(Function {
                params: vec!["url".to_string()],
                body: FunctionBody::Native(Arc::new(move |args| {
                    let map = map.clone();
                    Box::pin(async move {
                        let url = match args.first() {
                            Some(Value::String(s)) => s.clone(),
                            Some(other) => {
                                return Ok(error_value(format!(
                                    "RES.Redirect: expected a URL string, got {}",
                                    other.type_name()
                                )));
                            }
                            None => {
                                return Ok(error_value("RES.Redirect: expected a URL string"));
                            }
                        };
                        let mut lock = map.lock().unwrap();
                        lock.insert("_redirect".to_string(), Value::String(url));
                        if !lock.contains_key("_status") {
                            lock.insert("_status".to_string(), Value::Number(302.0));
                        }
                        Ok(Value::Null)
                    })
                })),
                captured: Env::new_root(),
            })
        };

        let response = Value::Object(response_map.clone());
        {
            let mut map = response_map.lock().unwrap();
            map.insert("SetStatus".to_string(), response_set_status);
            map.insert("SetHeader".to_string(), response_set_header);
            map.insert("SetCookie".to_string(), response_set_cookie);
            map.insert("Json".to_string(), response_json);
            map.insert("Html".to_string(), response_html);
            map.insert("Redirect".to_string(), response_redirect);
        }

        env_mut.define("RES", response);

        // Define db
        let ping_conn = conn.clone();
        let ping = Value::Function(Function {
            params: vec!["value".to_string()],
            body: FunctionBody::Native(Arc::new(move |_args| {
                let conn = ping_conn.clone();
                Box::pin(async move {
                    match conn.ping().await {
                        Ok(text) => Ok(Value::String(text)),
                        Err(e) => Ok(error_value(format!("DB.Ping: {e}"))),
                    }
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
                            return Ok(error_value(format!(
                                "DB.Query: expected a SQL string, got {}",
                                other.type_name()
                            )));
                        }
                        None => {
                            return Ok(error_value("DB.Query: expected a SQL string"));
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
                            return Ok(error_value(format!(
                                "DB.Exec: expected a SQL string, got {}",
                                other.type_name()
                            )));
                        }
                        None => {
                            return Ok(error_value("DB.Exec: expected a SQL string"));
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
                            return Ok(error_value(format!(
                                "DB.Table: expected a table name, got {}",
                                other.type_name()
                            )));
                        }
                        None => {
                            return Ok(error_value("DB.Table: expected a table name"));
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

        if let Some(socket) = &context.socket {
            env_mut.define("SOCKET", crate::ws::socket_value(socket));
        }
    }

    env
}

fn query_stmt_to_value(stmt: crate::db::QueryStmt) -> Value {
    let all_stmt = stmt.clone();
    let bind_stmt = stmt.clone();
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

    let bind = Value::Function(Function {
        params: vec!["value".to_string()],
        body: FunctionBody::Native(Arc::new(move |args| {
            let stmt = bind_stmt.clone();
            Box::pin(async move {
                let value = match args.first() {
                    Some(v) => value_to_json(v),
                    None => serde_json::Value::Null,
                };
                Ok(query_stmt_to_value(stmt.bind(&value)))
            })
        })),
        captured: Env::new_root(),
    });

    let mut map = HashMap::new();
    map.insert("All".to_string(), all);
    map.insert("One".to_string(), one);
    map.insert("Bind".to_string(), bind);
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
                        return Ok(error_value("TableStmt.Insert: expected an object"));
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
                        return Ok(error_value("TableStmt.Update: expected an object"));
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
                        return Ok(error_value("TableStmt.Where: expected an object"));
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

fn error_value(msg: impl Into<String>) -> Value {
    let mut map = HashMap::new();
    map.insert("ok".to_string(), Value::Bool(false));
    map.insert("error".to_string(), Value::String(msg.into()));
    Value::Object(Arc::new(Mutex::new(map)))
}

fn unix_time() -> std::time::Duration {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
}

fn random_f64() -> f64 {
    use std::cell::Cell;
    thread_local! {
        static STATE: Cell<u64> = const { Cell::new(0) };
    }
    STATE.with(|state| {
        let mut x = state.get();
        if x == 0 {
            x = unix_time().as_nanos() as u64 ^ 0x9E37_79B9_7F4A_7C15;
            if x == 0 {
                x = 1;
            }
        }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        state.set(x);
        (x >> 11) as f64 / (1u64 << 53) as f64
    })
}

fn math_numbers(args: Vec<Value>) -> Result<Vec<f64>, String> {
    let mut out = Vec::new();
    if let [Value::Array(arr)] = args.as_slice() {
        for v in arr.lock().unwrap().iter() {
            match v {
                Value::Number(n) => out.push(*n),
                other => {
                    return Err(format!("expected numbers, got {}", other.type_name()));
                }
            }
        }
        return Ok(out);
    }
    for v in &args {
        match v {
            Value::Number(n) => out.push(*n),
            other => return Err(format!("expected numbers, got {}", other.type_name())),
        }
    }
    Ok(out)
}

pub(crate) fn value_to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Number(n) => {
            if n.fract() == 0.0 && n.abs() < 1e15 {
                serde_json::Value::Number(serde_json::Number::from(*n as i64))
            } else {
                serde_json::Number::from_f64(*n)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null)
            }
        }
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
    let (stmts, _) = Parser::parse(tokens, script);
    let mut evalulator = Evaluator::new();

    evalulator.eval_stmts(&stmts, env).await.unwrap();
    evalulator.output
}

#[cfg(test)]
#[path = "./process_tests.rs"]
mod process_tests;

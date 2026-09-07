use std::collections::HashMap;

use axum::extract::{Query, Request};
use axum::http::HeaderMap;
use axum::http::header::CONTENT_TYPE;

use crate::db::DbConn;
use crate::quickjs::Engine;
use crate::ws::SocketRef;

#[derive(Debug, Clone, PartialEq)]
pub struct Context {
    pub method: Method,
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub cookies: HashMap<String, String>,
    pub body: serde_json::Value,
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

        let headers: HashMap<String, String> = parts
            .headers
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|v| (name.as_str().to_ascii_lowercase(), v.to_string()))
            })
            .collect();

        let cookies: HashMap<String, String> = headers
            .get("cookie")
            .map(|raw| {
                raw.split(';')
                    .filter_map(|part| {
                        let (name, value) = part.trim().split_once('=')?;
                        Some((name.to_string(), value.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(Self {
            method,
            query,
            headers,
            cookies,
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
            cookies: HashMap::new(),
            body: serde_json::Value::Object(serde_json::Map::new()),
            socket: None,
        }
    }
}

fn parse_body(
    method: Method,
    headers: &HeaderMap,
    bytes: &[u8],
) -> Result<serde_json::Value, ContextError> {
    if matches!(method, Method::Get | Method::Head) || bytes.is_empty() {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }

    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if content_type.starts_with("application/json") || content_type.ends_with("+json") {
        let json: serde_json::Value = serde_json::from_slice(bytes).map_err(ContextError::Json)?;
        return Ok(json);
    }

    if content_type.starts_with("application/x-www-form-urlencoded") {
        let pairs: Vec<(String, String)> =
            serde_urlencoded::from_bytes(bytes).map_err(ContextError::Form)?;
        let mut map: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
        for (key, value) in pairs {
            map.entry(key.clone())
                .or_insert(serde_json::Value::String(value.clone()));
            let arr_key = format!("{key}s");
            match map.get_mut(&arr_key) {
                Some(serde_json::Value::Array(arr)) => arr.push(serde_json::Value::String(value)),
                _ => {
                    map.insert(
                        arr_key,
                        serde_json::Value::Array(vec![serde_json::Value::String(value)]),
                    );
                }
            }
        }
        return Ok(serde_json::Value::Object(map));
    }

    // Default: treat as text
    let mut map = serde_json::Map::new();
    map.insert(
        "text".to_string(),
        serde_json::Value::String(String::from_utf8_lossy(bytes).to_string()),
    );
    Ok(serde_json::Value::Object(map))
}

pub async fn process_src(src: String, context: Context, conn: DbConn) -> (String, HttpResponse) {
    let engine = match Engine::new(conn).await {
        Ok(e) => {
            if let Err(err) = e.setup(&context).await {
                return (
                    String::new(),
                    HttpResponse {
                        status: Some(500),
                        body: Some(format!("engine setup error: {err}")),
                        ..Default::default()
                    },
                );
            }
            e
        }
        Err(err) => {
            return (
                String::new(),
                HttpResponse {
                    status: Some(500),
                    body: Some(format!("engine init error: {err}")),
                    ..Default::default()
                },
            );
        }
    };

    let mut output = String::new();

    let sections = split_src(&src);

    let mut script_error_detected = false;

    for section in sections {
        match section {
            Section::Html(html) => output += &html,
            Section::Code { code, method } if method.matches(&context.method) => {
                match engine.run_section(&code).await {
                    Ok((text, _)) => output += &text,
                    Err(err) => {
                        output += &format!("<pre>script error: {err}</pre>");
                        script_error_detected = true;
                    }
                }
                // A script that sent a full body or a redirect owns the
                // response; stop rendering further sections.
                if engine.read_response().await.owns_response {
                    break;
                }
            }
            Section::Code { .. } => {}
        }
    }

    let mut state = engine.read_response().await;

    if script_error_detected && state.status.is_none() {
        state.status = Some(500);
    }

    let response = HttpResponse::from_state(state);
    (output, response)
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

    pub fn matches(&self, request_method: &Method) -> bool {
        matches!(self, Method::All) || self == request_method
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Section {
    Html(String),
    Code { code: String, method: Method },
}

/// Host-side snapshot of the state written by a script's `RES` object.
#[derive(Debug, Clone, Default)]
pub struct HttpResponseState {
    pub status: Option<u16>,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub content_type: Option<String>,
    pub redirect: Option<String>,
    pub cookies: Vec<String>,
    pub owns_response: bool,
}

/// Script-controlled HTTP response state.
#[derive(Debug, Clone, Default)]
pub struct HttpResponse {
    pub status: Option<u16>,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub content_type: Option<String>,
    pub redirect: Option<String>,
}

impl HttpResponse {
    pub fn from_state(state: HttpResponseState) -> Self {
        let mut headers = state.headers;
        for cookie in state.cookies {
            headers.push(("set-cookie".to_string(), cookie));
        }
        Self {
            status: state.status,
            headers,
            body: state.body,
            content_type: state.content_type,
            redirect: state.redirect,
        }
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

pub fn split_src(src: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut rest = src;

    while !rest.is_empty() {
        match rest.find("<rhp") {
            None => {
                // Rest is HTML
                if !rest.trim().is_empty() {
                    sections.push(Section::Html(rest.to_string()));
                }
                break;
            }
            Some(start) => {
                // Capture HTML before the tag
                if start > 0 {
                    let section = rest[..start].to_string();
                    if !section.trim().is_empty() {
                        sections.push(Section::Html(section));
                    }
                }

                let after_open = &rest[start + "<rhp".len()..];
                let (method, body) = match after_open.find('>') {
                    Some(gt) => (parse_method(&after_open[..gt]), &after_open[gt + 1..]),
                    None => (Method::All, after_open),
                };

                match body.find("</rhp>") {
                    None => {
                        // Unclosed tag, treat the rest as a code block anyway.
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

#[cfg(test)]
#[path = "./process_tests.rs"]
mod process_tests;

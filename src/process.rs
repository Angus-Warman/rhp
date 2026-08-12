use std::{cell::RefCell, collections::HashMap, rc::Rc};

use axum::http::Request;

use crate::{eval::Evaluator, lexer, parser::Parser, value::{
    self, Env, Function, FunctionBody::{self}, Value,
}};

#[derive(Debug, Clone, PartialEq)]
pub struct Context {
    pub method: Method,
    pub query:  HashMap<String, String>,
}

impl Context {
    pub fn from_request<B>(request: &Request<B>) -> Self {
        Self {
            method: Method::from_str(request.method().as_str()),
            query:  parse_url_query(request.uri().query()),
        }
    }
}

impl Default for Context {
    fn default() -> Self {
        Self {
            method: Method::All,
            query:  HashMap::new(),
        }
    }
}

fn parse_url_query(query: Option<&str>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(q) = query {
        for pair in q.split('&') {
            if pair.is_empty() {
                continue;
            }
            match pair.split_once('=') {
                Some((k, v)) => { map.insert(k.to_string(), v.to_string()); }
                None         => { map.insert(pair.to_string(), String::new()); }
            }
        }
    }
    map
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

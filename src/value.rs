use crate::ast::*;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::eval::EvalError;

#[derive(Debug, Clone)]
pub struct Env {
    vars: HashMap<String, Value>,
    consts: HashSet<String>,
    parent: Option<Arc<Mutex<Env>>>,
}

impl Env {
    pub fn new_root() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            vars: HashMap::new(),
            consts: HashSet::new(),
            parent: None,
        }))
    }

    pub fn new_child(parent: Arc<Mutex<Env>>) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            vars: HashMap::new(),
            consts: HashSet::new(),
            parent: Some(parent),
        }))
    }

    pub fn get(&self, name: &str) -> Option<Value> {
        if let Some(v) = self.vars.get(name) {
            return Some(v.clone());
        }
        self.parent.as_ref()?.lock().unwrap().get(name)
    }

    pub fn set(&mut self, name: &str, value: Value) -> Result<(), String> {
        // Walk up to find where the variable lives, then update it there.
        // If not found anywhere, set in current scope (implicit global).
        if self.vars.contains_key(name) {
            if self.consts.contains(name) {
                return Err(format!("cannot assign to constant `{name}`"));
            }
            self.vars.insert(name.to_string(), value);
            return Ok(());
        }
        if let Some(parent) = &self.parent
            && parent.lock().unwrap().has(name)
        {
            return parent.lock().unwrap().set(name, value);
        }
        self.vars.insert(name.to_string(), value);
        Ok(())
    }

    pub fn define(&mut self, name: &str, value: Value) {
        // Always defines in the current scope (for let/var declarations)
        self.vars.insert(name.to_string(), value);
        self.consts.remove(name);
    }

    pub fn define_const(&mut self, name: &str, value: Value) {
        // Defines a constant: assigning to it later is an error.
        self.vars.insert(name.to_string(), value);
        self.consts.insert(name.to_string());
    }

    fn has(&self, name: &str) -> bool {
        if self.vars.contains_key(name) {
            return true;
        }
        self.parent
            .as_ref()
            .is_some_and(|p| p.lock().unwrap().has(name))
    }
}

// ---- Values ----

#[derive(Debug, Clone)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Arc<Mutex<Vec<Value>>>),
    Object(Arc<Mutex<HashMap<String, Value>>>),
    Function(Function),
}

#[derive(Debug, Clone)]
pub struct Function {
    pub params: Vec<String>,
    pub body: FunctionBody,
    pub captured: Arc<Mutex<Env>>, // closure environment
}

#[derive(Clone)]
pub enum FunctionBody {
    Block(Vec<Stmt>),
    Expr(Box<Expr>),
    Native(NativeFn),
}

pub(crate) type NativeFn = Arc<
    dyn Fn(Vec<Value>) -> Pin<Box<dyn Future<Output = Result<Value, EvalError>> + Send>>
        + Send
        + Sync,
>;

impl std::fmt::Debug for FunctionBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FunctionBody::Block(_) => write!(f, "FunctionBody::Block(...)"),
            FunctionBody::Expr(_) => write!(f, "FunctionBody::Expr(...)"),
            FunctionBody::Native(_) => write!(f, "FunctionBody::Native(...)"),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Array(a), Value::Array(b)) => Arc::ptr_eq(a, b),
            (Value::Object(a), Value::Object(b)) => Arc::ptr_eq(a, b),
            (Value::Function(_), Value::Function(_)) => false, // functions aren't equal
            _ => false,
        }
    }
}

impl Value {
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0 && !n.is_nan(),
            Value::String(s) => !s.is_empty(),
            Value::Array(_) => true,
            Value::Function(_) => true,
            Value::Object(o) => {
                let obj = o.lock().unwrap();

                if obj.is_empty() {
                    return false;
                }

                if obj.get("ok").is_some_and(|v| v.is_truthy()) {
                    return true;
                }

                if obj.get("error").is_some_and(|e| e.is_truthy()) {
                    return false;
                }

                true
            }
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
            Value::Function(_) => "function",
        }
    }

    pub fn display(&self) -> String {
        match self {
            Value::Null => "null".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    format!("{}", *n as i64)
                } else {
                    format!("{}", n)
                }
            }
            Value::String(s) => s.clone(),
            Value::Array(a) => {
                let items: Vec<String> = a
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|v| v.display_in_container())
                    .collect();
                format!("[{}]", items.join(", "))
            }
            Value::Object(o) => {
                let mut pairs: Vec<(String, String)> = o
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.display_in_container()))
                    .collect();
                pairs.sort_by(|a, b| a.0.cmp(&b.0));
                if pairs.is_empty() {
                    "{}".to_string()
                } else {
                    format!(
                        "{{ {} }}",
                        pairs
                            .iter()
                            .map(|(k, v)| format!("{}: {}", k, v))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
            Value::Function(_) => "[function]".to_string(),
        }
    }

    fn display_in_container(&self) -> String {
        match self {
            Value::String(s) => format!("\"{}\"", s),
            other => other.display(),
        }
    }
}

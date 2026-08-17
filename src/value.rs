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
    Float(f64),
    Integer(i64),
    String(String),
    Array(Arc<Mutex<Vec<Value>>>),
    Object(Arc<Mutex<HashMap<String, Value>>>),
    Function(Function),
}

#[derive(Debug, Clone, Copy)]
pub enum Numeric {
    Float(f64),
    Integer(i64),
}

impl Numeric {
    pub fn to_f64(self) -> f64 {
        match self {
            Numeric::Float(f) => f,
            Numeric::Integer(i) => i as f64,
        }
    }
}

impl std::ops::Add for Numeric {
    type Output = Numeric;
    fn add(self, rhs: Numeric) -> Numeric {
        match (self, rhs) {
            (Numeric::Float(a), Numeric::Float(b)) => Numeric::Float(a + b),
            (Numeric::Float(a), Numeric::Integer(b)) => Numeric::Float(a + b as f64),
            (Numeric::Integer(a), Numeric::Float(b)) => Numeric::Float(a as f64 + b),
            (Numeric::Integer(a), Numeric::Integer(b)) => Numeric::Integer(a + b),
        }
    }
}

impl std::ops::Sub for Numeric {
    type Output = Numeric;
    fn sub(self, rhs: Numeric) -> Numeric {
        match (self, rhs) {
            (Numeric::Float(a), Numeric::Float(b)) => Numeric::Float(a - b),
            (Numeric::Float(a), Numeric::Integer(b)) => Numeric::Float(a - b as f64),
            (Numeric::Integer(a), Numeric::Float(b)) => Numeric::Float(a as f64 - b),
            (Numeric::Integer(a), Numeric::Integer(b)) => Numeric::Integer(a - b),
        }
    }
}

impl std::ops::Mul for Numeric {
    type Output = Numeric;
    fn mul(self, rhs: Numeric) -> Numeric {
        match (self, rhs) {
            (Numeric::Float(a), Numeric::Float(b)) => Numeric::Float(a * b),
            (Numeric::Float(a), Numeric::Integer(b)) => Numeric::Float(a * b as f64),
            (Numeric::Integer(a), Numeric::Float(b)) => Numeric::Float(a as f64 * b),
            (Numeric::Integer(a), Numeric::Integer(b)) => Numeric::Integer(a * b),
        }
    }
}

impl std::ops::Div for Numeric {
    type Output = Numeric;
    fn div(self, rhs: Numeric) -> Numeric {
        match (self, rhs) {
            (Numeric::Float(a), Numeric::Float(b)) => Numeric::Float(a / b),
            (Numeric::Float(a), Numeric::Integer(b)) => Numeric::Float(a / b as f64),
            (Numeric::Integer(a), Numeric::Float(b)) => Numeric::Float(a as f64 / b),
            (Numeric::Integer(a), Numeric::Integer(b)) => Numeric::Integer(a / b),
        }
    }
}

impl std::ops::Rem for Numeric {
    type Output = Numeric;
    fn rem(self, rhs: Numeric) -> Numeric {
        match (self, rhs) {
            (Numeric::Float(a), Numeric::Float(b)) => Numeric::Float(a % b),
            (Numeric::Float(a), Numeric::Integer(b)) => Numeric::Float(a % b as f64),
            (Numeric::Integer(a), Numeric::Float(b)) => Numeric::Float(a as f64 % b),
            (Numeric::Integer(a), Numeric::Integer(b)) => Numeric::Integer(a % b),
        }
    }
}

impl PartialEq for Numeric {
    fn eq(&self, other: &Numeric) -> bool {
        self.to_f64() == other.to_f64()
    }
}

impl PartialOrd for Numeric {
    fn partial_cmp(&self, other: &Numeric) -> Option<std::cmp::Ordering> {
        self.to_f64().partial_cmp(&other.to_f64())
    }
}

impl From<Numeric> for Value {
    fn from(n: Numeric) -> Self {
        match n {
            Numeric::Float(f) => Value::Float(f),
            Numeric::Integer(i) => Value::Integer(i),
        }
    }
}

impl TryFrom<Value> for Numeric {
    type Error = ();
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::Float(f) => Ok(Numeric::Float(f)),
            Value::Integer(i) => Ok(Numeric::Integer(i)),
            _ => Err(()),
        }
    }
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
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Float(a), Value::Integer(b)) => *a == *b as f64,
            (Value::Integer(a), Value::Float(b)) => *a as f64 == *b,
            (Value::Integer(a), Value::Integer(b)) => a == b,
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
            Value::Float(f) => *f != 0.0 && !f.is_nan(),
            Value::Integer(i) => *i != 0,
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
            Value::Float(_) => "number",
            Value::Integer(_) => "number",
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
            Value::Float(f) => {
                if f.fract() == 0.0 && f.abs() < 1e15 {
                    format!("{}", *f as i64)
                } else {
                    format!("{}", f)
                }
            }
            Value::Integer(i) => i.to_string(),
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

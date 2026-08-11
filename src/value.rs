use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::*;
use crate::eval::EvalError;

#[derive(Debug, Clone)]
pub struct Env {
    vars:   HashMap<String, Value>,
    parent: Option<Rc<RefCell<Env>>>,
}

impl Env {
    pub fn new_root() -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self { vars: HashMap::new(), parent: None }))
    }

    pub fn new_child(parent: Rc<RefCell<Env>>) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self { vars: HashMap::new(), parent: Some(parent) }))
    }

    pub fn get(&self, name: &str) -> Option<Value> {
        if let Some(v) = self.vars.get(name) {
            return Some(v.clone());
        }
        self.parent.as_ref()?.borrow().get(name)
    }

    pub fn set(&mut self, name: &str, value: Value) {
        // Walk up to find where the variable lives, then update it there.
        // If not found anywhere, set in current scope (for let/const/var decls).
        if self.vars.contains_key(name) {
            self.vars.insert(name.to_string(), value);
            return;
        }
        if let Some(parent) = &self.parent {
            if parent.borrow().has(name) {
                parent.borrow_mut().set(name, value);
                return;
            }
        }
        self.vars.insert(name.to_string(), value);
    }

    pub fn define(&mut self, name: &str, value: Value) {
        // Always defines in the current scope (for let/const/var declarations)
        self.vars.insert(name.to_string(), value);
    }

    fn has(&self, name: &str) -> bool {
        if self.vars.contains_key(name) { return true; }
        self.parent.as_ref().map_or(false, |p| p.borrow().has(name))
    }
}

// ---- Values ----

#[derive(Debug, Clone)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Rc<RefCell<Vec<Value>>>),
    Object(Rc<RefCell<HashMap<String, Value>>>),
    Function(Function),
}

#[derive(Debug, Clone)]
pub struct Function {
    pub params:   Vec<String>,
    pub body:     FunctionBody,
    pub captured: Rc<RefCell<Env>>,  // closure environment
}

#[derive(Clone)]
pub enum FunctionBody {
    Block(Vec<SpannedStmt>),
    Expr(Box<SpannedExpr>),
    Native(Rc<dyn Fn(Vec<Value>) -> Result<Value, EvalError>>),
}

impl std::fmt::Debug for FunctionBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FunctionBody::Block(_)  => write!(f, "FunctionBody::Block(...)"),
            FunctionBody::Expr(_)   => write!(f, "FunctionBody::Expr(...)"),
            FunctionBody::Native(_) => write!(f, "FunctionBody::Native(...)"),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Null,        Value::Null)        => true,
            (Value::Bool(a),     Value::Bool(b))     => a == b,
            (Value::Number(a),   Value::Number(b))   => a == b,
            (Value::String(a),   Value::String(b))   => a == b,
            (Value::Array(a),    Value::Array(b))    => Rc::ptr_eq(a, b),
            (Value::Object(a),   Value::Object(b))   => Rc::ptr_eq(a, b),
            (Value::Function(_), Value::Function(_)) => false, // functions aren't equal
            _ => false,
        }
    }
}

impl Value {
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Null        => false,
            Value::Bool(b)     => *b,
            Value::Number(n)   => *n != 0.0 && !n.is_nan(),
            Value::String(s)   => !s.is_empty(),
            Value::Array(_)    => true,
            Value::Object(_)   => true,
            Value::Function(_) => true,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null        => "null",
            Value::Bool(_)     => "bool",
            Value::Number(_)   => "number",
            Value::String(_)   => "string",
            Value::Array(_)    => "array",
            Value::Object(_)   => "object",
            Value::Function(_) => "function",
        }
    }

    pub fn display(&self) -> String {
        match self {
            Value::Null        => "null".to_string(),
            Value::Bool(b)     => b.to_string(),
            Value::Number(n)   => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    format!("{}", *n as i64)
                } else {
                    format!("{}", n)
                }
            }
            Value::String(s)   => s.clone(),
            Value::Array(a)    => {
                let items: Vec<String> = a.borrow().iter().map(|v| v.display()).collect();
                format!("[{}]", items.join(", "))
            }
            Value::Object(o)   => {
                let pairs: Vec<String> = o.borrow().iter()
                    .map(|(k, v)| format!("{}: {}", k, v.display()))
                    .collect();
                format!("{{{}}}", pairs.join(", "))
            }
            Value::Function(_) => "[function]".to_string(),
        }
    }
}
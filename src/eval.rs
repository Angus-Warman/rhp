use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use async_recursion::async_recursion;
use crate::ast::*;

use crate::value::*;
#[derive(Debug, Clone)]
pub struct EvalError {
    pub message: String,
    pub span:    std::ops::Range<usize>,
}

impl EvalError {
    fn new(message: impl Into<String>, span: std::ops::Range<usize>) -> Self {
        Self { message: message.into(), span }
    }
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}..{}: {:?}", self.span.start, self.span.end, self.message)
    }
}

// ---- Control flow signals ----
// These aren't errors, they're how return/break/continue unwind the call stack.

#[derive(Debug)]
enum Signal {
    Return(Value),
    Break,
    Continue,
    Error(EvalError),
}

impl From<EvalError> for Signal {
    fn from(e: EvalError) -> Self { Signal::Error(e) }
}

type EvalResult  = Result<Value, Signal>;
type StmtResult  = Result<(), Signal>;

// ---- Evaluator ----

pub struct Evaluator {
    pub output: String, // HTML output buffer
}

impl Evaluator {
    pub fn new() -> Self {
        Self { output: String::new() }
    }

    // Entry point: evaluate a list of statements in a given environment
    pub async fn eval_stmts(
        &mut self,
        stmts: &[Stmt],
        env:   Arc<Mutex<Env>>,
    ) -> Result<(), EvalError> {
        for stmt in stmts {
            if let Err(signal) = self.eval_stmt(stmt, env.clone()).await {
                match signal {
                    Signal::Error(e) => return Err(e),
                    Signal::Return(value) => {
                        self.output += &value.display();
                        return Ok(());
                    },
                    Signal::Break => return Err(EvalError::new(
                        "break outside loop", stmt.span.clone()
                    )),
                    Signal::Continue => return Err(EvalError::new(
                        "continue outside loop", stmt.span.clone()
                    )),
                }
            }
        }
        Ok(())
    }

    // ---- Statements ----

    #[async_recursion]
    async fn eval_stmt(&mut self, stmt: &Stmt, env: Arc<Mutex<Env>>) -> StmtResult {
        match &stmt.node {
            RawStmt::Error => Ok(()), // already reported at parse time

            RawStmt::VarDecl { kind: _, name, init } => {
                let value = match init {
                    Some(expr) => self.eval_expr(expr, env.clone()).await?,
                    None       => Value::Null,
                };
                env.lock().unwrap().define(name, value);
                Ok(())
            }

            RawStmt::FunctionDecl { name, params, body } => {
                let func = Value::Function(Function {
                    params:   params.clone(),
                    body:     FunctionBody::Block(body.clone()),
                    captured: env.clone(),
                });
                env.lock().unwrap().define(name, func);
                Ok(())
            }

            RawStmt::If { cond, then, otherwise } => {
                let val = self.eval_expr(cond, env.clone()).await?;
                if val.is_truthy() {
                    let child = Env::new_child(env.clone());
                    self.eval_block(then, child).await?;
                } else if let Some(else_stmts) = otherwise {
                    let child = Env::new_child(env.clone());
                    self.eval_block(else_stmts, child).await?;
                }
                Ok(())
            }

            RawStmt::While { cond, body } => {
                loop {
                    let val = self.eval_expr(cond, env.clone()).await?;
                    if !val.is_truthy() { break; }
                    let child = Env::new_child(env.clone());
                    match self.eval_block(body, child).await {
                        Ok(())                  => {}
                        Err(Signal::Break)      => break,
                        Err(Signal::Continue)   => continue,
                        Err(e)                  => return Err(e),
                    }
                }
                Ok(())
            }

            RawStmt::For { init, cond, update, body } => {
                let loop_env = Env::new_child(env.clone());

                if let Some(init_stmt) = init {
                    self.eval_stmt(init_stmt, loop_env.clone()).await?;
                }

                loop {
                    if let Some(cond_expr) = cond {
                        let val = self.eval_expr(cond_expr, loop_env.clone()).await?;
                        if !val.is_truthy() { break; }
                    }

                    let iter_env = Env::new_child(loop_env.clone());
                    match self.eval_block(body, iter_env).await {
                        Ok(())                => {}
                        Err(Signal::Break)    => break,
                        Err(Signal::Continue) => {}
                        Err(e)               => return Err(e),
                    }

                    if let Some(update_expr) = update {
                        self.eval_expr(update_expr, loop_env.clone()).await?;
                    }
                }
                Ok(())
            }

            RawStmt::Return(expr) => {
                let value = match expr {
                    Some(e) => self.eval_expr(e, env).await?,
                    None    => Value::Null,
                };
                Err(Signal::Return(value))
            }

            RawStmt::Break    => Err(Signal::Break),
            RawStmt::Continue => Err(Signal::Continue),

            RawStmt::Expr(expr) => {
                self.eval_expr(expr, env).await?;
                Ok(())
            }
            RawStmt::Try(expr) => {
                let value = self.eval_expr(expr, env).await?;

                if value.is_truthy() {
                    Ok(())
                }
                else {
                    Err(Signal::Return(value))
                }
            },
        }
    }

    #[async_recursion]
    async fn eval_block(&mut self, stmts: &[Stmt], env: Arc<Mutex<Env>>) -> StmtResult {
        for stmt in stmts {
            self.eval_stmt(stmt, env.clone()).await?;
        }
        Ok(())
    }

    // ---- Expressions ----

    #[async_recursion]
    async fn eval_expr(&mut self, expr: &Expr, env: Arc<Mutex<Env>>) -> EvalResult {
        match &expr.node {
            RawExpr::Null        => Ok(Value::Null),
            RawExpr::Bool(b)     => Ok(Value::Bool(*b)),
            RawExpr::Number(n)   => Ok(Value::Number(n.parse::<f64>().unwrap_or(0.0))),
            RawExpr::StringLit(s) => Ok(Value::String(s.trim_matches(|c| c == '"' || c == '\'').to_string())),

            RawExpr::Ident(name) => {
                env.lock().unwrap().get(name).ok_or_else(|| Signal::Error(EvalError::new(
                    format!("undefined variable `{}`", name),
                    expr.span.clone(),
                )))
            }

            RawExpr::Array(items) => {
                let mut vals = Vec::new();
                for item in items {
                    vals.push(self.eval_expr(item, env.clone()).await?);
                }
                Ok(Value::Array(Arc::new(Mutex::new(vals))))
            }

            RawExpr::Object(pairs) => {
                let map = Arc::new(Mutex::new(HashMap::new()));
                for (key, expr) in pairs {
                    let value = self.eval_expr(expr, env.clone()).await?;
                    map.lock().unwrap().insert(key.clone(), value);
                }
                Ok(Value::Object(map))
            }

            RawExpr::Prefix { op, expr: inner } => {
                self.eval_prefix(op, inner, env, &expr.span).await
            }

            RawExpr::Postfix { op, expr: inner } => {
                self.eval_postfix(op, inner, env, &expr.span).await
            }

            RawExpr::Binary { op, left, right } => {
                self.eval_binary(op, left, right, env, &expr.span).await
            }

            RawExpr::Assign { op, target, value } => {
                self.eval_assign(op, target, value, env, &expr.span).await
            }

            RawExpr::Ternary { cond, then, otherwise } => {
                let c = self.eval_expr(cond, env.clone()).await?;
                if c.is_truthy() {
                    self.eval_expr(then, env).await
                } else {
                    self.eval_expr(otherwise, env).await
                }
            }

            RawExpr::Member { object, property } => {
                let obj = self.eval_expr(object, env).await?;
                self.eval_member(&obj, property, &expr.span)
            }

            RawExpr::Index { object, index } => {
                let obj = self.eval_expr(object, env.clone()).await?;
                let idx = self.eval_expr(index, env).await?;
                self.eval_index(&obj, &idx, &expr.span)
            }

            RawExpr::Call { callee, args } => {
                self.eval_call(callee, args, env, &expr.span).await
            }

            RawExpr::Arrow { params, body } => {
                let func = Function {
                    params:   params.clone(),
                    body:     match body {
                        ArrowBody::Block(stmts) => FunctionBody::Block(stmts.clone()),
                        ArrowBody::Expr(e)      => FunctionBody::Expr(e.clone()),
                    },
                    captured: env, // capture current scope
                };
                Ok(Value::Function(func))
            }

            RawExpr::HtmlBlock(stmts) => {
                // Evaluate stmts; any output they push goes to self.output
                let child = Env::new_child(env);
                self.eval_block(stmts, child).await
                    .map_err(|s| match s {
                        Signal::Error(e) => Signal::Error(e),
                        other => Signal::Error(EvalError::new(
                            format!("unexpected signal in html block: {:?}", other),
                            expr.span.clone(),
                        )),
                    })?;
                Ok(Value::Null)
            }
        }
    }

    // ---- Prefix / Postfix ----

    #[async_recursion]
    async fn eval_prefix(
        &mut self,
        op:   &PrefixOp,
        expr: &Expr,
        env:  Arc<Mutex<Env>>,
        span: &std::ops::Range<usize>,
    ) -> EvalResult {
        match op {
            PrefixOp::Not => {
                let v = self.eval_expr(expr, env).await?;
                Ok(Value::Bool(!v.is_truthy()))
            }
            PrefixOp::Neg => {
                let v = self.eval_expr(expr, env).await?;
                match v {
                    Value::Number(n) => Ok(Value::Number(-n)),
                    other => Err(Signal::Error(EvalError::new(
                        format!("cannot negate {}", other.type_name()), span.clone()
                    ))),
                }
            }
            PrefixOp::PlusPlus | PrefixOp::MinusMinus => {
                let delta: f64 = if matches!(op, PrefixOp::PlusPlus) { 1.0 } else { -1.0 };
                let new_val = self.numeric_mutate(expr, delta, env, span).await?;
                Ok(new_val) // prefix: return new value
            }
        }
    }

    #[async_recursion]
    async fn eval_postfix(
        &mut self,
        op:   &PostfixOp,
        expr: &Expr,
        env:  Arc<Mutex<Env>>,
        span: &std::ops::Range<usize>,
    ) -> EvalResult {
        let delta: f64 = if matches!(op, PostfixOp::PlusPlus) { 1.0 } else { -1.0 };
        let old_val = self.eval_expr(expr, env.clone()).await?;
        self.numeric_mutate(expr, delta, env, span).await?;
        Ok(old_val) // postfix: return old value
    }

    // Add `delta` to a numeric lvalue and write it back. Returns the new value.
    #[async_recursion]
    async fn numeric_mutate(
        &mut self,
        target: &Expr,
        delta:  f64,
        env:    Arc<Mutex<Env>>,
        span:   &std::ops::Range<usize>,
    ) -> Result<Value, Signal> {
        let current = self.eval_expr(target, env.clone()).await?;
        let n = match current {
            Value::Number(n) => n,
            other => return Err(Signal::Error(EvalError::new(
                format!("++ / -- requires a number, got {}", other.type_name()), span.clone()
            ))),
        };
        let new_val = Value::Number(n + delta);
        self.write_target(target, new_val.clone(), env, span).await?;
        Ok(new_val)
    }

    // ---- Binary ----

    #[async_recursion]
    async fn eval_binary(
        &mut self,
        op:    &BinOp,
        left:  &Expr,
        right: &Expr,
        env:   Arc<Mutex<Env>>,
        span:  &std::ops::Range<usize>,
    ) -> EvalResult {
        // Short-circuit for && and ||
        match op {
            BinOp::And => {
                let l = self.eval_expr(left, env.clone()).await?;
                return if !l.is_truthy() { Ok(l) } else { self.eval_expr(right, env).await };
            }
            BinOp::Or => {
                let l = self.eval_expr(left, env.clone()).await?;
                return if l.is_truthy() { Ok(l) } else { self.eval_expr(right, env).await };
            }
            _ => {}
        }

        let l = self.eval_expr(left,  env.clone()).await?;
        let r = self.eval_expr(right, env).await?;

        match op {
            BinOp::Add => match (&l, &r) {
                (Value::Number(a), Value::Number(b)) => Ok(Value::Number(a + b)),
                // String coercion: if either side is a string, concatenate
                _ => Ok(Value::String(format!("{}{}", l.display(), r.display()))),
            },
            BinOp::Sub => numeric_op(l, r, |a, b| a - b, span),
            BinOp::Mul => numeric_op(l, r, |a, b| a * b, span),
            BinOp::Div => numeric_op(l, r, |a, b| a / b, span),
            BinOp::Mod => numeric_op(l, r, |a, b| a % b, span),

            BinOp::Eq       => Ok(Value::Bool(loose_eq(&l, &r))),
            BinOp::Neq      => Ok(Value::Bool(!loose_eq(&l, &r))),
            BinOp::StrictEq  => Ok(Value::Bool(l == r)),
            BinOp::StrictNeq => Ok(Value::Bool(l != r)),

            BinOp::Lt  => compare(l, r, |a, b| a <  b, span),
            BinOp::Lte => compare(l, r, |a, b| a <= b, span),
            BinOp::Gt  => compare(l, r, |a, b| a >  b, span),
            BinOp::Gte => compare(l, r, |a, b| a >= b, span),

            BinOp::And | BinOp::Or => unreachable!(),
        }
    }

    // ---- Assignment ----

    #[async_recursion]
    async fn eval_assign(
        &mut self,
        op:     &AssignOp,
        target: &Expr,
        value:  &Expr,
        env:    Arc<Mutex<Env>>,
        span:   &std::ops::Range<usize>,
    ) -> EvalResult {
        let rhs = self.eval_expr(value, env.clone()).await?;

        let final_val = if matches!(op, AssignOp::Assign) {
            rhs
        } else {
            let current = self.eval_expr(target, env.clone()).await?;
            match (op, &current, &rhs) {
                (AssignOp::Add, Value::Number(a), Value::Number(b)) => Value::Number(a + b),
                (AssignOp::Add, _, _) => Value::String(format!("{}{}", current.display(), rhs.display())),
                (AssignOp::Sub, Value::Number(a), Value::Number(b)) => Value::Number(a - b),
                (AssignOp::Mul, Value::Number(a), Value::Number(b)) => Value::Number(a * b),
                (AssignOp::Div, Value::Number(a), Value::Number(b)) => Value::Number(a / b),
                _ => return Err(Signal::Error(EvalError::new(
                    "invalid operand types for compound assignment", span.clone()
                ))),
            }
        };

        self.write_target(target, final_val.clone(), env, span).await?;
        Ok(final_val)
    }

    // Write a value to an lvalue target: ident, member, or index
    #[async_recursion]
    async fn write_target(
        &mut self,
        target:    &Expr,
        value:     Value,
        env:       Arc<Mutex<Env>>,
        span:      &std::ops::Range<usize>,
    ) -> Result<(), Signal> {
        match &target.node {
            RawExpr::Ident(name) => {
                env.lock().unwrap().set(name, value);
                Ok(())
            }
            RawExpr::Member { object, property } => {
                let obj = self.eval_expr(object, env).await?;
                match obj {
                    Value::Object(map) => {
                        map.lock().unwrap().insert(property.clone(), value);
                        Ok(())
                    }
                    other => Err(Signal::Error(EvalError::new(
                        format!("cannot set property on {}", other.type_name()), span.clone()
                    ))),
                }
            }
            RawExpr::Index { object, index } => {
                let obj = self.eval_expr(object, env.clone()).await?;
                let idx = self.eval_expr(index, env).await?;
                match (&obj, &idx) {
                    (Value::Array(arr), Value::Number(n)) => {
                        let i = *n as usize;
                        let mut a = arr.lock().unwrap();
                        if i < a.len() {
                            a[i] = value;
                            Ok(())
                        } else {
                            Err(Signal::Error(EvalError::new(
                                format!("array index {} out of bounds", i), span.clone()
                            )))
                        }
                    }
                    (Value::Object(map), Value::String(key)) => {
                        map.lock().unwrap().insert(key.clone(), value);
                        Ok(())
                    }
                    _ => Err(Signal::Error(EvalError::new(
                        "invalid assignment target", span.clone()
                    ))),
                }
            }
            _ => Err(Signal::Error(EvalError::new(
                "invalid assignment target", span.clone()
            ))),
        }
    }

    // ---- Member / Index access ----

    fn eval_member(
        &self,
        obj:      &Value,
        property: &str,
        span:     &std::ops::Range<usize>,
    ) -> EvalResult {
        match obj {
            Value::Object(map) => {
                Ok(map.lock().unwrap().get(property).cloned().unwrap_or(Value::Null))
            }
            Value::Array(arr) => {
                match property {
                    "length" => Ok(Value::Number(arr.lock().unwrap().len() as f64)),
                    other => Err(Signal::Error(EvalError::new(
                        format!("array has no property `{}`", other), span.clone()
                    ))),
                }
            }
            Value::String(s) => {
                match property {
                    "length" => Ok(Value::Number(s.len() as f64)),
                    other => Err(Signal::Error(EvalError::new(
                        format!("string has no property `{}`", other), span.clone()
                    ))),
                }
            }
            other => Err(Signal::Error(EvalError::new(
                format!("cannot access property on {}", other.type_name()), span.clone()
            ))),
        }
    }

    fn eval_index(
        &self,
        obj:  &Value,
        idx:  &Value,
        span: &std::ops::Range<usize>,
    ) -> EvalResult {
        match (obj, idx) {
            (Value::Array(arr), Value::Number(n)) => {
                let i = *n as usize;
                Ok(arr.lock().unwrap().get(i).cloned().unwrap_or(Value::Null))
            }
            (Value::Object(map), Value::String(key)) => {
                Ok(map.lock().unwrap().get(key.as_str()).cloned().unwrap_or(Value::Null))
            }
            _ => Err(Signal::Error(EvalError::new(
                format!("cannot index {} with {}", obj.type_name(), idx.type_name()), span.clone()
            ))),
        }
    }

    // ---- Function calls ----

    #[async_recursion]
    async fn eval_call(
        &mut self,
        callee: &Expr,
        args:   &[Expr],
        env:    Arc<Mutex<Env>>,
        span:   &std::ops::Range<usize>,
    ) -> EvalResult {
        let callee_val = self.eval_expr(callee, env.clone()).await?;

        let mut arg_vals = Vec::new();
        for arg in args {
            arg_vals.push(self.eval_expr(arg, env.clone()).await?);
        }

        match callee_val {
            Value::Function(func) => self.call_function(func, arg_vals, span).await,
            other => Err(Signal::Error(EvalError::new(
                format!("cannot call {}", other.type_name()), span.clone()
            ))),
        }
    }

    #[async_recursion]
    async fn call_function(
        &mut self,
        func: Function,
        args: Vec<Value>,
        span: &std::ops::Range<usize>,
    ) -> EvalResult {
        match &func.body {
            FunctionBody::Native(f) => {
                f(args).await.map_err(Signal::Error)
            }

            FunctionBody::Block(stmts) => {
                // New scope rooted in the closure's captured environment
                let call_env = Env::new_child(func.captured.clone());
                self.bind_params(&func.params, args, &call_env, span)?;
                let stmts = stmts.clone();
                match self.eval_block(&stmts, call_env).await {
                    Ok(())                 => Ok(Value::Null),
                    Err(Signal::Return(v)) => Ok(v),
                    Err(other)             => Err(other),
                }
            }

            FunctionBody::Expr(expr) => {
                let call_env = Env::new_child(func.captured.clone());
                self.bind_params(&func.params, args, &call_env, span)?;
                let expr = expr.clone();
                self.eval_expr(&expr, call_env).await
            }
        }
    }

    fn bind_params(
        &self,
        params:   &[String],
        args:     Vec<Value>,
        call_env: &Arc<Mutex<Env>>,
        span:     &std::ops::Range<usize>,
    ) -> Result<(), Signal> {
        if args.len() != params.len() {
            return Err(Signal::Error(EvalError::new(
                format!("expected {} arguments, got {}", params.len(), args.len()),
                span.clone(),
            )));
        }
        for (name, value) in params.iter().zip(args) {
            call_env.lock().unwrap().define(name, value);
        }
        Ok(())
    }
}

// ---- Free helpers ----

fn numeric_op(
    l:    Value,
    r:    Value,
    op:   impl Fn(f64, f64) -> f64,
    span: &std::ops::Range<usize>,
) -> EvalResult {
    match (&l, &r) {
        (Value::Number(a), Value::Number(b)) => Ok(Value::Number(op(*a, *b))),
        _ => Err(Signal::Error(EvalError::new(
            format!("arithmetic requires numbers, got {} and {}", l.type_name(), r.type_name()),
            span.clone(),
        ))),
    }
}

fn compare(
    l:    Value,
    r:    Value,
    op:   impl Fn(f64, f64) -> bool,
    span: &std::ops::Range<usize>,
) -> EvalResult {
    match (&l, &r) {
        (Value::Number(a), Value::Number(b)) => Ok(Value::Bool(op(*a, *b))),
        (Value::String(a), Value::String(b)) => {
            let ord = a.cmp(b);
            Ok(Value::Bool(op(
                match ord { std::cmp::Ordering::Less => -1.0, std::cmp::Ordering::Equal => 0.0, std::cmp::Ordering::Greater => 1.0 },
                0.0,
            )))
        }
        _ => Err(Signal::Error(EvalError::new(
            format!("cannot compare {} and {}", l.type_name(), r.type_name()), span.clone()
        ))),
    }
}

fn loose_eq(l: &Value, r: &Value) -> bool {
    match (l, r) {
        (Value::Null,      Value::Null)      => true,
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Bool(a),   Value::Bool(b))   => a == b,
        // null == null only, not null == 0 etc, keep it sane
        _ => false,
    }
}

use crate::ast::*;
use async_recursion::async_recursion;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::value::*;
#[derive(Debug, Clone)]
pub struct EvalError {
    pub message: String,
    pub span: std::ops::Range<usize>,
}

impl EvalError {
    fn new(message: impl Into<String>, span: std::ops::Range<usize>) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}..{}: {:?}",
            self.span.start, self.span.end, self.message
        )
    }
}

// ---- Control flow signals ----
// These aren't errors, they're how return/break/continue unwind the call stack.

#[derive(Debug)]
enum Signal {
    Return(Value),
    // A statement evaluated to a value (e.g. `let x = 1`); only `try`
    // consumes it, everywhere else it's discarded.
    Value(Value),
    Break,
    Continue,
    Error(EvalError),
}

impl From<EvalError> for Signal {
    fn from(e: EvalError) -> Self {
        Signal::Error(e)
    }
}

type EvalResult = Result<Value, Signal>;
type StmtResult = Result<(), Signal>;

// ---- Evaluator ----

pub struct Evaluator {
    pub output: String,          // HTML output buffer
    pub returned: Option<Value>, // value returned by `return`, if any
}

impl Evaluator {
    pub fn new() -> Self {
        Self {
            output: String::new(),
            returned: None,
        }
    }

    // Entry point: evaluate a list of statements in a given environment
    pub async fn eval_stmts(
        &mut self,
        stmts: &[Stmt],
        env: Arc<Mutex<Env>>,
    ) -> Result<(), EvalError> {
        for stmt in stmts {
            if let Err(signal) = self.eval_stmt(stmt, env.clone()).await {
                match signal {
                    Signal::Error(e) => return Err(e),
                    Signal::Return(value) => {
                        self.output += &value.display();
                        self.returned = Some(value);
                        return Ok(());
                    }
                    Signal::Value(_) => {}
                    Signal::Break => {
                        return Err(EvalError::new("break outside loop", stmt.span.clone()));
                    }
                    Signal::Continue => {
                        return Err(EvalError::new("continue outside loop", stmt.span.clone()));
                    }
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

            RawStmt::VarDecl { kind, name, init } => {
                let value = match init {
                    Some(expr) => self.eval_expr(expr, env.clone()).await?,
                    None => Value::Null,
                };
                let mut env_guard = env.lock().expect("lock poisoned");
                match kind {
                    VarKind::Const => env_guard.define_const(name, value.clone()),
                    _ => env_guard.define(name, value.clone()),
                }
                drop(env_guard);
                Err(Signal::Value(value))
            }

            RawStmt::FunctionDecl { name, params, body } => {
                let func = Value::Function(Function {
                    params: params.clone(),
                    body: FunctionBody::Block(body.clone()),
                    captured: env.clone(),
                });
                env.lock().expect("lock poisoned").define(name, func);
                Ok(())
            }

            RawStmt::If {
                cond,
                then,
                otherwise,
            } => {
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
                    if !val.is_truthy() {
                        break;
                    }
                    let child = Env::new_child(env.clone());
                    match self.eval_block(body, child).await {
                        Ok(()) => {}
                        Err(Signal::Break) => break,
                        Err(Signal::Continue) => continue,
                        Err(e) => return Err(e),
                    }
                }
                Ok(())
            }

            RawStmt::For {
                init,
                cond,
                update,
                body,
            } => {
                let loop_env = Env::new_child(env.clone());

                if let Some(init_stmt) = init {
                    match self.eval_stmt(init_stmt, loop_env.clone()).await {
                        Ok(()) => {}
                        Err(Signal::Value(_)) => {} // init value discarded
                        Err(e) => return Err(e),
                    }
                }

                loop {
                    if let Some(cond_expr) = cond {
                        let val = self.eval_expr(cond_expr, loop_env.clone()).await?;
                        if !val.is_truthy() {
                            break;
                        }
                    }

                    let iter_env = Env::new_child(loop_env.clone());
                    match self.eval_block(body, iter_env).await {
                        Ok(()) => {}
                        Err(Signal::Break) => break,
                        Err(Signal::Continue) => {}
                        Err(e) => return Err(e),
                    }

                    if let Some(update_expr) = update {
                        self.eval_expr(update_expr, loop_env.clone()).await?;
                    }
                }
                Ok(())
            }

            RawStmt::ForIn {
                var,
                iterable,
                body,
            } => {
                let iter_val = self.eval_expr(iterable, env.clone()).await?;
                let loop_env = Env::new_child(env);
                loop_env
                    .lock()
                    .expect("lock poisoned")
                    .define(var, Value::Null);

                match &iter_val {
                    Value::Array(arr) => {
                        let items = arr.lock().expect("lock poisoned").clone();
                        for item in items {
                            if !self
                                .eval_for_in_iteration(var, item, &loop_env, body)
                                .await?
                            {
                                break;
                            }
                        }
                    }
                    Value::Float(f) => {
                        // TODO remove this? kind of weird
                        for i in 0..f.floor() as i64 {
                            if !self
                                .eval_for_in_iteration(var, Value::Integer(i), &loop_env, body)
                                .await?
                            {
                                break;
                            }
                        }
                    }
                    Value::Integer(i) => {
                        for idx in 0..*i {
                            if !self
                                .eval_for_in_iteration(var, Value::Integer(idx), &loop_env, body)
                                .await?
                            {
                                break;
                            }
                        }
                    }
                    Value::String(s) => {
                        for ch in s.chars() {
                            if !self
                                .eval_for_in_iteration(
                                    var,
                                    Value::String(ch.to_string()),
                                    &loop_env,
                                    body,
                                )
                                .await?
                            {
                                break;
                            }
                        }
                    }
                    Value::Object(map) => {
                        let keys: Vec<String> =
                            map.lock().expect("lock poisoned").keys().cloned().collect();
                        for key in keys {
                            if !self
                                .eval_for_in_iteration(var, Value::String(key), &loop_env, body)
                                .await?
                            {
                                break;
                            }
                        }
                    }
                    Value::Null => {}
                    other => {
                        return Err(Signal::Error(EvalError::new(
                            format!("cannot iterate over {}", other.type_name()),
                            stmt.span.clone(),
                        )));
                    }
                }
                Ok(())
            }

            RawStmt::Switch { expr, cases } => {
                let discriminant = self.eval_expr(expr, env.clone()).await?;

                // Find the first matching case index
                let mut start_idx = None;
                let mut default_idx = None;
                for (i, case) in cases.iter().enumerate() {
                    if case.test.is_none() && default_idx.is_none() {
                        default_idx = Some(i);
                    }
                    if let Some(ref test_expr) = case.test {
                        let case_val = self.eval_expr(test_expr, env.clone()).await?;
                        if loose_eq(&discriminant, &case_val) {
                            start_idx = Some(i);
                            break;
                        }
                    }
                }

                // Determine where to start executing
                let Some(mut from) = start_idx.or(default_idx) else {
                    return Ok(());
                };

                // Execute from the starting case, falling through
                while from < cases.len() {
                    let child = Env::new_child(env.clone());
                    match self.eval_block(&cases[from].body, child).await {
                        Ok(()) => {} // fall through to next case
                        Err(Signal::Break) => return Ok(()),
                        Err(Signal::Continue) => return Err(Signal::Continue),
                        Err(e) => return Err(e),
                    }
                    from += 1;
                }

                Ok(())
            }

            RawStmt::Return(expr) => {
                let value = match expr {
                    Some(e) => self.eval_expr(e, env).await?,
                    None => Value::Null,
                };
                Err(Signal::Return(value))
            }

            RawStmt::Break => Err(Signal::Break),
            RawStmt::Continue => Err(Signal::Continue),

            RawStmt::Expr(expr) => {
                let value = self.eval_expr(expr, env).await?;
                Err(Signal::Value(value))
            }
            RawStmt::Try(stmt) => match self.eval_stmt(stmt, env.clone()).await {
                Ok(()) => Ok(()),
                Err(Signal::Value(value)) => {
                    if value.is_truthy() {
                        Ok(())
                    } else {
                        Err(Signal::Return(value))
                    }
                }
                Err(signal) => Err(signal),
            },
        }
    }

    #[async_recursion]
    async fn eval_block(&mut self, stmts: &[Stmt], env: Arc<Mutex<Env>>) -> StmtResult {
        for stmt in stmts {
            match self.eval_stmt(stmt, env.clone()).await {
                Ok(()) => {}
                Err(Signal::Value(_)) => {} // statement value discarded here
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    // Bind `item` to `var` and run one for-in iteration. Returns Ok(true)
    // to keep looping, Ok(false) to stop (a `break` fired).
    #[async_recursion]
    async fn eval_for_in_iteration(
        &mut self,
        var: &str,
        item: Value,
        loop_env: &Arc<Mutex<Env>>,
        body: &[Stmt],
    ) -> Result<bool, Signal> {
        loop_env
            .lock()
            .expect("lock poisoned")
            .set(var, item)
            .map_err(|msg| Signal::Error(EvalError::new(msg, 0..0)))?;
        let iter_env = Env::new_child(loop_env.clone());
        match self.eval_block(body, iter_env).await {
            Ok(()) => Ok(true),
            Err(Signal::Break) => Ok(false),
            Err(Signal::Continue) => Ok(true),
            Err(e) => Err(e),
        }
    }

    // ---- Expressions ----

    #[async_recursion]
    async fn eval_expr(&mut self, expr: &Expr, env: Arc<Mutex<Env>>) -> EvalResult {
        match &expr.node {
            RawExpr::Null => Ok(Value::Null),
            RawExpr::Bool(b) => Ok(Value::Bool(*b)),
            RawExpr::Number(n) => {
                if n.contains(".") {
                    return Ok(Value::Float(n.parse::<f64>().unwrap_or(0.0)));
                } else {
                    return Ok(Value::Integer(n.parse::<i64>().unwrap_or(0)));
                }
            }
            RawExpr::StringLit(s) => Ok(Value::String(s.clone())),

            RawExpr::Ident(name) => env.lock().expect("lock poisoned").get(name).ok_or_else(|| {
                Signal::Error(EvalError::new(
                    format!("undefined variable `{}`", name),
                    expr.span.clone(),
                ))
            }),

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
                    map.lock()
                        .expect("lock poisoned")
                        .insert(key.clone(), value);
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

            RawExpr::Ternary {
                cond,
                then,
                otherwise,
            } => {
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

            RawExpr::Call { callee, args } => self.eval_call(callee, args, env, &expr.span).await,

            RawExpr::Arrow { params, body } => {
                let func = Function {
                    params: params.clone(),
                    body: match body {
                        ArrowBody::Block(stmts) => FunctionBody::Block(stmts.clone()),
                        ArrowBody::Expr(e) => FunctionBody::Expr(e.clone()),
                    },
                    captured: env, // capture current scope
                };
                Ok(Value::Function(func))
            }

            RawExpr::HtmlTemplate(nodes) => {
                let html = self.eval_template(nodes, env).await?;
                Ok(Value::String(html))
            }
        }
    }

    // Render a template, escaping every `{expr}` slot.
    #[async_recursion]
    async fn eval_template(
        &mut self,
        nodes: &[TemplateNode],
        env: Arc<Mutex<Env>>,
    ) -> Result<String, Signal> {
        let mut out = String::new();
        for node in nodes {
            match node {
                TemplateNode::Text(text) => out.push_str(text),
                TemplateNode::Element {
                    tag,
                    attrs,
                    children,
                } => {
                    out.push('<');
                    out.push_str(tag);
                    for attr in attrs {
                        match attr {
                            Attr::Bool(name) => {
                                out.push(' ');
                                out.push_str(name);
                            }
                            Attr::Static(name, value) => {
                                out.push(' ');
                                out.push_str(name);
                                out.push_str("=\"");
                                out.push_str(value);
                                out.push('"');
                            }
                            Attr::Expr(name, expr) => {
                                let value = self.eval_expr(expr, env.clone()).await?;
                                out.push(' ');
                                out.push_str(name);
                                out.push_str("=\"");
                                out.push_str(&escape_html(&value.display()));
                                out.push('"');
                            }
                        }
                    }
                    out.push('>');
                    out.push_str(&self.eval_template(children, env.clone()).await?);
                    out.push_str("</");
                    out.push_str(tag);
                    out.push('>');
                }
                TemplateNode::Expr(expr) => {
                    let value = self.eval_expr(expr, env.clone()).await?;
                    out.push_str(&escape_html(&value.display()));
                }
            }
        }
        Ok(out)
    }

    // ---- Prefix / Postfix ----

    #[async_recursion]
    async fn eval_prefix(
        &mut self,
        op: &PrefixOp,
        expr: &Expr,
        env: Arc<Mutex<Env>>,
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
                    Value::Float(n) => Ok(Value::Float(-n)),
                    Value::Integer(n) => Ok(Value::Integer(-n)),
                    other => Err(Signal::Error(EvalError::new(
                        format!("cannot negate {}", other.type_name()),
                        span.clone(),
                    ))),
                }
            }
            PrefixOp::Typeof => {
                let v = self.eval_expr(expr, env).await?;
                Ok(Value::String(v.type_name().to_string()))
            }
            PrefixOp::PlusPlus | PrefixOp::MinusMinus => {
                let delta: f64 = if matches!(op, PrefixOp::PlusPlus) {
                    1.0
                } else {
                    -1.0
                };
                let new_val = self.numeric_mutate(expr, delta, env, span).await?;
                Ok(new_val) // prefix: return new value
            }
        }
    }

    #[async_recursion]
    async fn eval_postfix(
        &mut self,
        op: &PostfixOp,
        expr: &Expr,
        env: Arc<Mutex<Env>>,
        span: &std::ops::Range<usize>,
    ) -> EvalResult {
        let delta: f64 = if matches!(op, PostfixOp::PlusPlus) {
            1.0
        } else {
            -1.0
        };
        let old_val = self.eval_expr(expr, env.clone()).await?;
        self.numeric_mutate(expr, delta, env, span).await?;
        Ok(old_val) // postfix: return old value
    }

    // Add `delta` to a numeric lvalue and write it back. Returns the new value.
    #[async_recursion]
    async fn numeric_mutate(
        &mut self,
        target: &Expr,
        delta: f64,
        env: Arc<Mutex<Env>>,
        span: &std::ops::Range<usize>,
    ) -> Result<Value, Signal> {
        let current = self.eval_expr(target, env.clone()).await?;
        let new_val = match current {
            Value::Float(n) => Value::Float(n + delta),
            Value::Integer(n) => {
                if delta.fract() == 0.0 {
                    Value::Integer(n + delta as i64)
                } else {
                    Value::Float(n as f64 + delta)
                }
            }
            other => {
                return Err(Signal::Error(EvalError::new(
                    format!("++ / -- requires a number, got {}", other.type_name()),
                    span.clone(),
                )));
            }
        };
        self.write_target(target, new_val.clone(), env, span)
            .await?;
        Ok(new_val)
    }

    // ---- Binary ----

    #[async_recursion]
    async fn eval_binary(
        &mut self,
        op: &BinOp,
        left: &Expr,
        right: &Expr,
        env: Arc<Mutex<Env>>,
        span: &std::ops::Range<usize>,
    ) -> EvalResult {
        // Short-circuit for && and ||
        match op {
            BinOp::And => {
                let l = self.eval_expr(left, env.clone()).await?;
                return if !l.is_truthy() {
                    Ok(l)
                } else {
                    self.eval_expr(right, env).await
                };
            }
            BinOp::Or => {
                let l = self.eval_expr(left, env.clone()).await?;
                return if l.is_truthy() {
                    Ok(l)
                } else {
                    self.eval_expr(right, env).await
                };
            }
            _ => {}
        }

        let l = self.eval_expr(left, env.clone()).await?;
        let r = self.eval_expr(right, env).await?;

        match op {
            BinOp::Add => match (&l, &r) {
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
                (Value::Float(a), Value::Integer(b)) => Ok(Value::Float(a + *b as f64)),
                (Value::Integer(a), Value::Float(b)) => Ok(Value::Float(*a as f64 + b)),
                (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a + b)),
                // String coercion: if either side is a string, concatenate
                _ => Ok(Value::String(format!("{}{}", l.display(), r.display()))),
            },
            BinOp::Sub => numeric_op(l, r, |a, b| a - b, span),
            BinOp::Mul => numeric_op(l, r, |a, b| a * b, span),
            BinOp::Div => numeric_op(l, r, |a, b| a / b, span),
            BinOp::Mod => numeric_op(l, r, |a, b| a % b, span),

            BinOp::BitAnd => bitwise_op(&l, &r, |a, b| a & b, span),
            BinOp::BitOr => bitwise_op(&l, &r, |a, b| a | b, span),
            BinOp::BitXor => bitwise_op(&l, &r, |a, b| a ^ b, span),
            BinOp::Shl => bitwise_op(&l, &r, |a, b| a << b, span),
            BinOp::Shr => bitwise_op(&l, &r, |a, b| a >> b, span),

            BinOp::Eq => Ok(Value::Bool(loose_eq(&l, &r))),
            BinOp::Neq => Ok(Value::Bool(!loose_eq(&l, &r))),
            BinOp::StrictEq => Ok(Value::Bool(l == r)),
            BinOp::StrictNeq => Ok(Value::Bool(l != r)),

            BinOp::Lt => compare(l, r, |a, b| a < b, span),
            BinOp::Lte => compare(l, r, |a, b| a <= b, span),
            BinOp::Gt => compare(l, r, |a, b| a > b, span),
            BinOp::Gte => compare(l, r, |a, b| a >= b, span),

            BinOp::And | BinOp::Or => unreachable!(),
        }
    }

    // ---- Assignment ----

    #[async_recursion]
    async fn eval_assign(
        &mut self,
        op: &AssignOp,
        target: &Expr,
        value: &Expr,
        env: Arc<Mutex<Env>>,
        span: &std::ops::Range<usize>,
    ) -> EvalResult {
        let rhs = self.eval_expr(value, env.clone()).await?;

        let final_val = if matches!(op, AssignOp::Assign) {
            rhs
        } else {
            let current = self.eval_expr(target, env.clone()).await?;
            match (op, &current, &rhs) {
                (AssignOp::Add, Value::Float(a), Value::Float(b)) => Value::Float(a + b),
                (AssignOp::Add, Value::Float(a), Value::Integer(b)) => Value::Float(a + *b as f64),
                (AssignOp::Add, Value::Integer(a), Value::Float(b)) => Value::Float(*a as f64 + b),
                (AssignOp::Add, Value::Integer(a), Value::Integer(b)) => Value::Integer(a + b),
                (AssignOp::Add, _, _) => {
                    Value::String(format!("{}{}", current.display(), rhs.display()))
                }
                (AssignOp::Sub, Value::Float(a), Value::Float(b)) => Value::Float(a - b),
                (AssignOp::Sub, Value::Integer(a), Value::Integer(b)) => Value::Integer(a - b),
                (AssignOp::Mul, Value::Float(a), Value::Float(b)) => Value::Float(a * b),
                (AssignOp::Mul, Value::Integer(a), Value::Integer(b)) => Value::Integer(a * b),
                (AssignOp::Div, Value::Float(a), Value::Float(b)) => Value::Float(a / b),
                (AssignOp::Div, Value::Integer(a), Value::Integer(b)) => Value::Integer(a / b),
                (AssignOp::Mod, Value::Float(a), Value::Float(b)) => Value::Float(a % b),
                (AssignOp::Mod, Value::Integer(a), Value::Integer(b)) => Value::Integer(a % b),
                (AssignOp::BitAnd, Value::Integer(a), Value::Integer(b)) => Value::Integer(a & b),
                (AssignOp::BitOr, Value::Integer(a), Value::Integer(b)) => Value::Integer(a | b),
                (AssignOp::BitXor, Value::Integer(a), Value::Integer(b)) => Value::Integer(a ^ b),
                (AssignOp::Shl, Value::Integer(a), Value::Integer(b)) => Value::Integer(a << b),
                (AssignOp::Shr, Value::Integer(a), Value::Integer(b)) => Value::Integer(a >> b),
                _ => {
                    return Err(Signal::Error(EvalError::new(
                        "invalid operand types for compound assignment",
                        span.clone(),
                    )));
                }
            }
        };

        self.write_target(target, final_val.clone(), env, span)
            .await?;
        Ok(final_val)
    }

    // Write a value to an lvalue target: ident, member, or index
    #[async_recursion]
    async fn write_target(
        &mut self,
        target: &Expr,
        value: Value,
        env: Arc<Mutex<Env>>,
        span: &std::ops::Range<usize>,
    ) -> Result<(), Signal> {
        match &target.node {
            RawExpr::Ident(name) => {
                env.lock()
                    .expect("lock poisoned")
                    .set(name, value)
                    .map_err(|msg| Signal::Error(EvalError::new(msg, span.clone())))?;
                Ok(())
            }
            RawExpr::Member { object, property } => {
                let obj = self.eval_expr(object, env).await?;
                match obj {
                    Value::Object(map) => {
                        map.lock()
                            .expect("lock poisoned")
                            .insert(property.clone(), value);
                        Ok(())
                    }
                    other => Err(Signal::Error(EvalError::new(
                        format!("cannot set property on {}", other.type_name()),
                        span.clone(),
                    ))),
                }
            }
            RawExpr::Index { object, index } => {
                let obj = self.eval_expr(object, env.clone()).await?;
                let idx = self.eval_expr(index, env).await?;
                match (&obj, &idx) {
                    (Value::Array(arr), Value::Integer(n)) => {
                        let i = *n as usize;
                        let mut a = arr.lock().expect("lock poisoned");
                        if i < a.len() {
                            a[i] = value;
                            Ok(())
                        } else {
                            Err(Signal::Error(EvalError::new(
                                format!("array index {} out of bounds", i),
                                span.clone(),
                            )))
                        }
                    }
                    (Value::Object(map), Value::String(key)) => {
                        map.lock()
                            .expect("lock poisoned")
                            .insert(key.clone(), value);
                        Ok(())
                    }
                    _ => Err(Signal::Error(EvalError::new(
                        "invalid assignment target",
                        span.clone(),
                    ))),
                }
            }
            _ => Err(Signal::Error(EvalError::new(
                "invalid assignment target",
                span.clone(),
            ))),
        }
    }

    // ---- Member / Index access ----

    fn eval_member(
        &self,
        obj: &Value,
        property: &str,
        span: &std::ops::Range<usize>,
    ) -> EvalResult {
        // `x.type` reports the runtime type for any non-object value. Objects
        // keep their own `type` field (e.g. `{ type: "person" }`).
        if property == "type" && !matches!(obj, Value::Object(_) | Value::Function(_)) {
            return Ok(Value::String(obj.type_name().to_string()));
        }
        match obj {
            Value::Object(map) => Ok(map
                .lock()
                .expect("lock poisoned")
                .get(property)
                .cloned()
                .unwrap_or(Value::Null)),
            Value::Array(arr) => match property {
                "length" => Ok(Value::Integer(
                    arr.lock().expect("lock poisoned").len() as i64
                )),
                other => bind_method(obj.clone(), other).ok_or_else(|| {
                    Signal::Error(EvalError::new(
                        format!("array has no property `{}`", other),
                        span.clone(),
                    ))
                }),
            },
            Value::String(s) => match property {
                "length" => Ok(Value::Integer(s.len() as i64)),
                other => bind_method(obj.clone(), other).ok_or_else(|| {
                    Signal::Error(EvalError::new(
                        format!("string has no property `{}`", other),
                        span.clone(),
                    ))
                }),
            },
            other => bind_method(obj.clone(), property).ok_or_else(|| {
                Signal::Error(EvalError::new(
                    format!("cannot access property on {}", other.type_name()),
                    span.clone(),
                ))
            }),
        }
    }

    fn eval_index(&self, obj: &Value, idx: &Value, span: &std::ops::Range<usize>) -> EvalResult {
        match (obj, idx) {
            (Value::Array(arr), Value::Integer(n)) => {
                let i = *n as usize;
                Ok(arr
                    .lock()
                    .expect("lock poisoned")
                    .get(i)
                    .cloned()
                    .unwrap_or(Value::Null))
            }
            (Value::Object(map), Value::String(key)) => Ok(map
                .lock()
                .expect("lock poisoned")
                .get(key.as_str())
                .cloned()
                .unwrap_or(Value::Null)),
            _ => Err(Signal::Error(EvalError::new(
                format!("cannot index {} with {}", obj.type_name(), idx.type_name()),
                span.clone(),
            ))),
        }
    }

    // ---- Function calls ----

    #[async_recursion]
    async fn eval_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        env: Arc<Mutex<Env>>,
        span: &std::ops::Range<usize>,
    ) -> EvalResult {
        let callee_val = self.eval_expr(callee, env.clone()).await?;

        let mut arg_vals = Vec::new();
        for arg in args {
            arg_vals.push(self.eval_expr(arg, env.clone()).await?);
        }

        match callee_val {
            Value::Function(func) => self.call_function(func, arg_vals, span).await,
            other => Err(Signal::Error(EvalError::new(
                format!("cannot call {}", other.type_name()),
                span.clone(),
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
            FunctionBody::Native(f) => f(args).await.map_err(Signal::Error),

            FunctionBody::Block(stmts) => {
                // New scope rooted in the closure's captured environment
                let call_env = Env::new_child(func.captured.clone());
                self.bind_params(&func.params, args, &call_env, span)?;
                let stmts = stmts.clone();
                match self.eval_block(&stmts, call_env).await {
                    Ok(()) => Ok(Value::Null),
                    Err(Signal::Return(v)) => Ok(v),
                    Err(other) => Err(other),
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
        params: &[String],
        args: Vec<Value>,
        call_env: &Arc<Mutex<Env>>,
        span: &std::ops::Range<usize>,
    ) -> Result<(), Signal> {
        if args.len() < params.len() {
            return Err(Signal::Error(EvalError::new(
                format!("expected {} arguments, got {}", params.len(), args.len()),
                span.clone(),
            )));
        }
        for (name, value) in params.iter().zip(args) {
            call_env.lock().expect("lock poisoned").define(name, value);
        }
        Ok(())
    }
}

// ---- Free helpers ----

/// Invoke a stored script function from native/pump code.
pub async fn call_value(value: Value, args: Vec<Value>) -> Result<Value, EvalError> {
    let func = match value {
        Value::Function(func) => func,
        other => {
            return Err(EvalError::new(
                format!("cannot call {}", other.type_name()),
                0..0,
            ));
        }
    };
    let mut evaluator = Evaluator::new();
    let span = 0..0;
    evaluator
        .call_function(func, args, &span)
        .await
        .map_err(|signal| match signal {
            Signal::Error(e) => e,
            other => EvalError::new(
                format!("unexpected control flow in callback: {:?}", other),
                span.clone(),
            ),
        })
}

fn numeric_op(
    l: Value,
    r: Value,
    op: impl Fn(Numeric, Numeric) -> Numeric,
    span: &std::ops::Range<usize>,
) -> EvalResult {
    match (&l, &r) {
        (Value::Float(a), Value::Float(b)) => Ok(op(Numeric::Float(*a), Numeric::Float(*b)).into()),
        (Value::Float(a), Value::Integer(b)) => {
            Ok(op(Numeric::Float(*a), Numeric::Float(*b as f64)).into())
        }
        (Value::Integer(a), Value::Float(b)) => {
            Ok(op(Numeric::Float(*a as f64), Numeric::Float(*b)).into())
        }
        (Value::Integer(a), Value::Integer(b)) => {
            Ok(op(Numeric::Integer(*a), Numeric::Integer(*b)).into())
        }

        _ => Err(Signal::Error(EvalError::new(
            format!(
                "arithmetic requires numbers, got {} and {}",
                l.type_name(),
                r.type_name()
            ),
            span.clone(),
        ))),
    }
}

fn bitwise_op(
    l: &Value,
    r: &Value,
    op: impl Fn(i64, i64) -> i64,
    span: &std::ops::Range<usize>,
) -> EvalResult {
    match (l, r) {
        (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(op(*a, *b))),
        _ => Err(Signal::Error(EvalError::new(
            format!(
                "bitwise operation requires integers, got {} and {}",
                l.type_name(),
                r.type_name()
            ),
            span.clone(),
        ))),
    }
}

fn compare(
    l: Value,
    r: Value,
    op: impl Fn(Numeric, Numeric) -> bool,
    span: &std::ops::Range<usize>,
) -> EvalResult {
    match (&l, &r) {
        (Value::Float(a), Value::Float(b)) => {
            Ok(Value::Bool(op(Numeric::Float(*a), Numeric::Float(*b))))
        }
        (Value::Float(a), Value::Integer(b)) => Ok(Value::Bool(op(
            Numeric::Float(*a),
            Numeric::Float(*b as f64),
        ))),
        (Value::Integer(a), Value::Float(b)) => Ok(Value::Bool(op(
            Numeric::Float(*a as f64),
            Numeric::Float(*b),
        ))),
        (Value::Integer(a), Value::Integer(b)) => {
            Ok(Value::Bool(op(Numeric::Integer(*a), Numeric::Integer(*b))))
        }

        (Value::String(a), Value::String(b)) => {
            let ord = a.cmp(b) as i8; // -1, 0, 1
            Ok(Value::Bool(op(
                Numeric::Integer(ord as i64),
                Numeric::Integer(0),
            )))
        }
        _ => Err(Signal::Error(EvalError::new(
            format!("cannot compare {} and {}", l.type_name(), r.type_name()),
            span.clone(),
        ))),
    }
}

fn loose_eq(l: &Value, r: &Value) -> bool {
    match (l, r) {
        (Value::Null, Value::Null) => true,
        (Value::Float(a), Value::Float(b)) => a == b,
        (Value::Float(a), Value::Integer(b)) => *a == *b as f64,
        (Value::Integer(a), Value::Float(b)) => *a as f64 == *b,
        (Value::Integer(a), Value::Integer(b)) => a == b,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        // null == null only, not null == 0 etc, keep it sane
        _ => false,
    }
}

/// Escape a dynamic value for safe inclusion in HTML output.
fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            '/' => out.push_str("&#x2F;"),
            '`' => out.push_str("&grave;"),
            '=' => out.push_str("&#x3D;"),
            other => out.push(other),
        }
    }
    out
}

// ---- Value methods (x.split(" "), x.trim(), x.map(...), ...) ----

type MethodImpl = fn(&Value, &[Value]) -> Result<Value, String>;

fn bind_method(receiver: Value, name: &str) -> Option<Value> {
    // Callback-taking array methods are async (they await script functions).
    let f: Option<NativeFn> = match &receiver {
        Value::Array(arr) => match name {
            "map" => Some(array_map(arr.clone())),
            "filter" => Some(array_filter(arr.clone())),
            "forEach" => Some(array_for_each(arr.clone())),
            "reduce" => Some(array_reduce(arr.clone())),
            "sort" => Some(array_sort(arr.clone())),
            _ => sync_method(receiver, name),
        },
        _ => sync_method(receiver, name),
    };
    f.map(|f| {
        Value::Function(Function {
            params: vec![],
            body: FunctionBody::Native(f),
            captured: Env::new_root(),
        })
    })
}

fn sync_method(receiver: Value, name: &str) -> Option<NativeFn> {
    let f: MethodImpl = match (&receiver, name) {
        (Value::String(_), "split") => string_split,
        (Value::String(_), "trim") => string_trim,
        (Value::String(_), "toUpper") => string_to_upper,
        (Value::String(_), "toLower") => string_to_lower,
        (Value::String(_), "replace") => string_replace,
        (Value::String(_), "contains") => string_contains,
        (Value::Array(_), "push") => array_push,
        (Value::Array(_), "join") => array_join,
        (Value::Array(_), "slice") => array_slice,
        (Value::Array(_), "indexOf") => array_index_of,
        (Value::Array(_), "includes") => array_includes,
        _ => return None,
    };
    Some(Arc::new(move |args| {
        let receiver = receiver.clone();
        Box::pin(async move {
            match f(&receiver, &args) {
                Ok(v) => Ok(v),
                Err(msg) => Ok(method_error(msg)),
            }
        })
    }))
}

fn method_error(msg: String) -> Value {
    let mut map = HashMap::new();
    map.insert("ok".to_string(), Value::Bool(false));
    map.insert("error".to_string(), Value::String(msg));
    Value::Object(Arc::new(Mutex::new(map)))
}

fn string_split(s: &Value, args: &[Value]) -> Result<Value, String> {
    let Value::String(s) = s else {
        unreachable!("bound method receiver")
    };
    let parts: Vec<Value> = match args.first() {
        None => s
            .split_whitespace()
            .map(|p| Value::String(p.to_string()))
            .collect(),
        Some(Value::String(sep)) if sep.is_empty() => {
            s.chars().map(|c| Value::String(c.to_string())).collect()
        }
        Some(Value::String(sep)) => s
            .split(sep.as_str())
            .map(|p| Value::String(p.to_string()))
            .collect(),
        Some(other) => {
            return Err(format!(
                "split: expected a string separator, got {}",
                other.type_name()
            ));
        }
    };
    Ok(Value::Array(Arc::new(Mutex::new(parts))))
}

fn string_trim(s: &Value, _args: &[Value]) -> Result<Value, String> {
    let Value::String(s) = s else {
        unreachable!("bound method receiver")
    };
    Ok(Value::String(s.trim().to_string()))
}

fn string_to_upper(s: &Value, _args: &[Value]) -> Result<Value, String> {
    let Value::String(s) = s else {
        unreachable!("bound method receiver")
    };
    Ok(Value::String(s.to_uppercase()))
}

fn string_to_lower(s: &Value, _args: &[Value]) -> Result<Value, String> {
    let Value::String(s) = s else {
        unreachable!("bound method receiver")
    };
    Ok(Value::String(s.to_lowercase()))
}

fn string_replace(s: &Value, args: &[Value]) -> Result<Value, String> {
    let Value::String(s) = s else {
        unreachable!("bound method receiver")
    };
    match args {
        [Value::String(from), Value::String(to)] => {
            Ok(Value::String(s.replace(from.as_str(), to.as_str())))
        }
        _ => Err("replace: expected (old, new) string arguments".to_string()),
    }
}

fn string_contains(s: &Value, args: &[Value]) -> Result<Value, String> {
    let Value::String(s) = s else {
        unreachable!("bound method receiver")
    };
    match args {
        [Value::String(needle)] => Ok(Value::Bool(s.contains(needle.as_str()))),
        _ => Err("contains: expected a string argument".to_string()),
    }
}

fn array_push(a: &Value, args: &[Value]) -> Result<Value, String> {
    let Value::Array(arr) = a else {
        unreachable!("bound method receiver")
    };
    let Some(item) = args.first().cloned() else {
        return Err("push: expected an item".to_string());
    };
    let mut lock = arr.lock().expect("lock poisoned");
    lock.push(item);
    Ok(Value::Integer(lock.len() as i64))
}

fn array_join(a: &Value, args: &[Value]) -> Result<Value, String> {
    let Value::Array(arr) = a else {
        unreachable!("bound method receiver")
    };
    let sep = match args.first() {
        None => ",".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => {
            return Err(format!(
                "join: expected a string separator, got {}",
                other.type_name()
            ));
        }
    };
    let lock = arr.lock().expect("lock poisoned");
    let joined = lock
        .iter()
        .map(|v| v.display())
        .collect::<Vec<String>>()
        .join(&sep);
    Ok(Value::String(joined))
}

fn array_slice(a: &Value, args: &[Value]) -> Result<Value, String> {
    let Value::Array(arr) = a else {
        unreachable!("bound method receiver")
    };
    let items = arr.lock().expect("lock poisoned").clone();
    let n = items.len() as i64;
    let idx = |arg: Option<&Value>| -> Result<i64, String> {
        match arg {
            None => Ok(n),
            Some(Value::Float(x)) => {
                let mut i = x.round() as i64;
                if i < 0 {
                    i += n;
                }
                Ok(i.max(0).min(n))
            }
            Some(Value::Integer(x)) => {
                let mut i = *x;
                if i < 0 {
                    i += n;
                }
                Ok(i.max(0).min(n))
            }
            Some(other) => Err(format!(
                "slice: expected number indices, got {}",
                other.type_name()
            )),
        }
    };
    let start = idx(args.first())?;
    let end = idx(args.get(1))?;
    let end = end.max(start);
    Ok(Value::Array(Arc::new(Mutex::new(
        items[start as usize..end as usize].to_vec(),
    ))))
}

fn array_index_of(a: &Value, args: &[Value]) -> Result<Value, String> {
    let Value::Array(arr) = a else {
        unreachable!("bound method receiver")
    };
    let Some(target) = args.first() else {
        return Ok(Value::Integer(-1));
    };
    let from = match args.get(1) {
        Some(Value::Float(n)) => n.round().max(0.0) as usize,
        Some(Value::Integer(n)) => (*n).max(0) as usize,
        Some(other) => {
            return Err(format!(
                "indexOf: expected a number fromIndex, got {}",
                other.type_name()
            ));
        }
        None => 0,
    };
    let lock = arr.lock().expect("lock poisoned");
    for (i, item) in lock.iter().enumerate().skip(from) {
        if item == target {
            return Ok(Value::Integer(i as i64));
        }
    }
    Ok(Value::Integer(-1))
}

fn array_includes(a: &Value, args: &[Value]) -> Result<Value, String> {
    let Value::Array(arr) = a else {
        unreachable!("bound method receiver")
    };
    let Some(target) = args.first() else {
        return Ok(Value::Bool(false));
    };
    Ok(Value::Bool(
        arr.lock()
            .expect("lock poisoned")
            .iter()
            .any(|item| item == target),
    ))
}

// ---- Callback-taking array methods (await script functions) ----

fn array_map(arr: Arc<Mutex<Vec<Value>>>) -> NativeFn {
    Arc::new(move |args| {
        let arr = arr.clone();
        Box::pin(async move {
            let Some(cb) = args.first().cloned() else {
                return Ok(method_error("map: expected a function".to_string()));
            };
            let items = arr.lock().expect("lock poisoned").clone();
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.into_iter().enumerate() {
                match call_value(cb.clone(), vec![item, Value::Integer(i as i64)]).await {
                    Ok(v) => out.push(v),
                    Err(e) => return Ok(method_error(format!("map: {e}"))),
                }
            }
            Ok(Value::Array(Arc::new(Mutex::new(out))))
        })
    })
}

fn array_filter(arr: Arc<Mutex<Vec<Value>>>) -> NativeFn {
    Arc::new(move |args| {
        let arr = arr.clone();
        Box::pin(async move {
            let Some(cb) = args.first().cloned() else {
                return Ok(method_error("filter: expected a function".to_string()));
            };
            let items = arr.lock().expect("lock poisoned").clone();
            let mut out = Vec::new();
            for (i, item) in items.into_iter().enumerate() {
                match call_value(cb.clone(), vec![item.clone(), Value::Integer(i as i64)]).await {
                    Ok(v) if v.is_truthy() => out.push(item),
                    Ok(_) => {}
                    Err(e) => return Ok(method_error(format!("filter: {e}"))),
                }
            }
            Ok(Value::Array(Arc::new(Mutex::new(out))))
        })
    })
}

fn array_for_each(arr: Arc<Mutex<Vec<Value>>>) -> NativeFn {
    Arc::new(move |args| {
        let arr = arr.clone();
        Box::pin(async move {
            let Some(cb) = args.first().cloned() else {
                return Ok(method_error("forEach: expected a function".to_string()));
            };
            let items = arr.lock().expect("lock poisoned").clone();
            for (i, item) in items.into_iter().enumerate() {
                if let Err(e) = call_value(cb.clone(), vec![item, Value::Integer(i as i64)]).await {
                    return Ok(method_error(format!("forEach: {e}")));
                }
            }
            Ok(Value::Null)
        })
    })
}

fn array_reduce(arr: Arc<Mutex<Vec<Value>>>) -> NativeFn {
    Arc::new(move |args| {
        let arr = arr.clone();
        Box::pin(async move {
            let Some(cb) = args.first().cloned() else {
                return Ok(method_error("reduce: expected a function".to_string()));
            };
            let items = arr.lock().expect("lock poisoned").clone();
            let (mut acc, start) = match args.get(1) {
                Some(init) => (init.clone(), 0),
                None => match items.first() {
                    Some(first) => (first.clone(), 1),
                    None => {
                        return Ok(method_error(
                            "reduce: empty array with no initial value".to_string(),
                        ));
                    }
                },
            };
            for (i, item) in items.iter().enumerate().skip(start) {
                match call_value(
                    cb.clone(),
                    vec![acc, item.clone(), Value::Integer(i as i64)],
                )
                .await
                {
                    Ok(v) => acc = v,
                    Err(e) => return Ok(method_error(format!("reduce: {e}"))),
                }
            }
            Ok(acc)
        })
    })
}

fn array_sort(arr: Arc<Mutex<Vec<Value>>>) -> NativeFn {
    Arc::new(move |args| {
        let arr = arr.clone();
        Box::pin(async move {
            let mut items = arr.lock().expect("lock poisoned").clone();
            match args.first() {
                Some(Value::Function(cb)) => {
                    // Insertion sort so the async comparator can be awaited.
                    for i in 1..items.len() {
                        let mut j = i;
                        while j > 0 {
                            let cmp = match call_value(
                                Value::Function(cb.clone()),
                                vec![items[j].clone(), items[j - 1].clone()],
                            )
                            .await
                            {
                                Ok(Value::Float(n)) => n,
                                Ok(Value::Integer(n)) => n as f64,
                                Ok(_) => 0.0,
                                Err(e) => return Ok(method_error(format!("sort: {e}"))),
                            };
                            if cmp < 0.0 {
                                items.swap(j, j - 1);
                                j -= 1;
                            } else {
                                break;
                            }
                        }
                    }
                }
                None => items.sort_by(value_cmp),
                Some(other) => {
                    return Ok(method_error(format!(
                        "sort: expected a comparator function, got {}",
                        other.type_name()
                    )));
                }
            }
            *arr.lock().expect("lock poisoned") = items.clone();
            Ok(Value::Array(arr))
        })
    })
}

fn value_cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
        (Value::Integer(x), Value::Integer(y)) => x.cmp(y),
        (Value::Float(x), Value::Integer(y)) => x
            .partial_cmp(&(*y as f64))
            .unwrap_or(std::cmp::Ordering::Equal),
        (Value::Integer(x), Value::Float(y)) => (*x as f64)
            .partial_cmp(y)
            .unwrap_or(std::cmp::Ordering::Equal),
        _ => a.display().cmp(&b.display()),
    }
}

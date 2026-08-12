use crate::lexer::Spanned;

pub type SpannedExpr = Spanned<Expr>;
pub type SpannedStmt = Spanned<Stmt>;

// ---- Expressions ----

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    // Literals
    Number(String),
    StringLit(String),
    Bool(bool),
    Null,

    // Identifier
    Ident(String),

    // Unary: !x, -x, ++x, --x, x++, x--
    Prefix { op: PrefixOp, expr: Box<SpannedExpr> },
    Postfix { op: PostfixOp, expr: Box<SpannedExpr> },

    // Binary: a + b, a === b, etc.
    Binary { op: BinOp, left: Box<SpannedExpr>, right: Box<SpannedExpr> },

    // Assignment: a = b, a += b, etc.
    Assign { op: AssignOp, target: Box<SpannedExpr>, value: Box<SpannedExpr> },

    // Ternary: cond ? then : else
    Ternary {
        cond:      Box<SpannedExpr>,
        then:      Box<SpannedExpr>,
        otherwise: Box<SpannedExpr>,
    },

    // Member access: a.b
    Member { object: Box<SpannedExpr>, property: String },

    // Index access: a[b]
    Index { object: Box<SpannedExpr>, index: Box<SpannedExpr> },

    // Call: f(a, b)
    Call { callee: Box<SpannedExpr>, args: Vec<SpannedExpr> },

    // Array literal: [a, b, c]
    Array(Vec<SpannedExpr>),

    // Object literal: { a: 1, b: "x" }
    Object(Vec<(String, SpannedExpr)>),

    // Arrow function: (a, b) => expr  |  (a, b) => { stmts }
    Arrow { params: Vec<String>, body: ArrowBody },

    // <html> ... </html> block inside script
    HtmlBlock(Vec<SpannedStmt>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArrowBody {
    Expr(Box<SpannedExpr>),
    Block(Vec<SpannedStmt>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PrefixOp  { Neg, Not, PlusPlus, MinusMinus }

#[derive(Debug, Clone, PartialEq)]
pub enum PostfixOp { PlusPlus, MinusMinus }

#[derive(Debug, Clone, PartialEq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod,
    Eq, Neq, StrictEq, StrictNeq,
    Lt, Lte, Gt, Gte,
    And, Or,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AssignOp { Assign, Add, Sub, Mul, Div }

// ---- Statements ----

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    // let/const/var x = expr;
    VarDecl { kind: VarKind, name: String, init: Option<SpannedExpr> },

    // function f(a, b) { ... }
    FunctionDecl { name: String, params: Vec<String>, body: Vec<SpannedStmt> },

    // if (cond) { ... } else { ... }
    If { cond: SpannedExpr, then: Vec<SpannedStmt>, otherwise: Option<Vec<SpannedStmt>> },

    // while (cond) { ... }
    While { cond: SpannedExpr, body: Vec<SpannedStmt> },

    // for (init; cond; update) { ... }
    For {
        init:   Option<Box<SpannedStmt>>,
        cond:   Option<SpannedExpr>,
        update: Option<SpannedExpr>,
        body:   Vec<SpannedStmt>,
    },

    Return(Option<SpannedExpr>),
    Break,
    Continue,

    // Any expression used as a statement: f(), x++, etc.
    Expr(SpannedExpr),

    // Placeholder inserted when a statement fails to parse
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VarKind { Let, Const, Var }
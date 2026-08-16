use crate::lexer::Spanned;

pub type Expr = Spanned<RawExpr>;
pub type Stmt = Spanned<RawStmt>;

// ---- Expressions ----

#[derive(Debug, Clone, PartialEq)]
pub enum RawExpr {
    // Literals
    Number(String),
    StringLit(String),
    Bool(bool),
    Null,

    // Identifier
    Ident(String),

    // Unary: !x, -x, ++x, --x, x++, x--
    Prefix {
        op: PrefixOp,
        expr: Box<Expr>,
    },
    Postfix {
        op: PostfixOp,
        expr: Box<Expr>,
    },

    // Binary: a + b, a === b, etc.
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },

    // Assignment: a = b, a += b, etc.
    Assign {
        op: AssignOp,
        target: Box<Expr>,
        value: Box<Expr>,
    },

    // Ternary: cond ? then : else
    Ternary {
        cond: Box<Expr>,
        then: Box<Expr>,
        otherwise: Box<Expr>,
    },

    // Member access: a.b
    Member {
        object: Box<Expr>,
        property: String,
    },

    // Index access: a[b]
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },

    // Call: f(a, b)
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },

    // Array literal: [a, b, c]
    Array(Vec<Expr>),

    // Object literal: { a: 1, b: "x" }
    Object(Vec<(String, Expr)>),

    // Arrow function: (a, b) => expr  |  (a, b) => { stmts }
    Arrow {
        params: Vec<String>,
        body: ArrowBody,
    },

    // <html> ... </html> block inside script
    HtmlBlock(Vec<Stmt>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArrowBody {
    Expr(Box<Expr>),
    Block(Vec<Stmt>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PrefixOp {
    Neg,
    Not,
    PlusPlus,
    MinusMinus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PostfixOp {
    PlusPlus,
    MinusMinus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Neq,
    StrictEq,
    StrictNeq,
    Lt,
    Lte,
    Gt,
    Gte,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AssignOp {
    Assign,
    Add,
    Sub,
    Mul,
    Div,
}

// ---- Statements ----

#[derive(Debug, Clone, PartialEq)]
pub enum RawStmt {
    // let/const/var x = expr;
    VarDecl {
        kind: VarKind,
        name: String,
        init: Option<Expr>,
    },

    // function f(a, b) { ... }
    FunctionDecl {
        name: String,
        params: Vec<String>,
        body: Vec<Stmt>,
    },

    // if (cond) { ... } else { ... }
    If {
        cond: Expr,
        then: Vec<Stmt>,
        otherwise: Option<Vec<Stmt>>,
    },

    // while (cond) { ... }
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },

    // for (init; cond; update) { ... }
    For {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        update: Option<Expr>,
        body: Vec<Stmt>,
    },

    // for (x in y) { ... }
    ForIn {
        var: String,
        iterable: Expr,
        body: Vec<Stmt>,
    },

    Return(Option<Expr>),
    Try(Expr),
    Break,
    Continue,

    // Any expression used as a statement: f(), x++, etc.
    Expr(Expr),

    // Placeholder inserted when a statement fails to parse
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VarKind {
    Let,
    Const,
    Var,
}

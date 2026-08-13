use logos::Logos;

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Lex error at {}..{}: {:?}", self.span.start, self.span.end, self.message)
    }
}

pub type Span = std::ops::Range<usize>;

#[derive(Debug, Clone, PartialEq)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub fn new(node: T, span: Span) -> Self {
        Self { node, span }
    }
}

pub fn merge_spans(a: &Span, b: &Span) -> Span {
    a.start.min(b.start)..a.end.max(b.end)
}

pub type SpannedToken = Spanned<Token>;

impl std::error::Error for ParseError {}

#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\r\n]+")]
#[logos(skip r"//[^\n]")]          // single-line comments. TODO: Check if that * was needed
#[logos(skip r"/\*([^*]|\*[^/])*\*/")] // block comments
pub enum Token {

    // --- Keywords ---
    #[token("let")]      Let,
    #[token("const")]    Const,
    #[token("var")]      Var,
    #[token("for")]      For,
    #[token("while")]    While,
    #[token("return")]   Return,
    #[token("try")]   Try,
    #[token("continue")] Continue,
    #[token("break")]    Break,
    #[token("if")]       If,
    #[token("else")]     Else,
    #[token("function")] Function,
    #[token("null")]     Null,
    #[token("true")]     True,
    #[token("false")]    False,

    // --- HTML snippet ---
    #[token("<html>")]   HtmlOpen,
    #[token("</html>")]  HtmlClose,

    // --- Identifiers ---
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice().to_string())]
    Ident(String),

    // --- Literals ---
    #[regex(r"[0-9]+(\.[0-9]+)?", |lex| lex.slice().to_string())]
    Number(String),

    #[regex(r#""([^"\\]|\\.)*""#, |lex| lex.slice().to_string())]
    StringDouble(String),

    #[regex(r#"'([^'\\]|\\.)*'"#, |lex| lex.slice().to_string())]
    StringSingle(String),

    // --- Operators (longer tokens first) ---
    #[token("=>")] Arrow,
    #[token("===")] StrictEq,
    #[token("!==")] StrictNeq,
    #[token("++")] PlusPlus,
    #[token("--")] MinusMinus,
    #[token("==")] Eq,
    #[token("!=")] Neq,
    #[token("<=")] Lte,
    #[token(">=")] Gte,
    #[token("&&")] And,
    #[token("||")] Or,
    #[token("+=")] PlusAssign,
    #[token("-=")] MinusAssign,
    #[token("*=")] MulAssign,
    #[token("/=")] DivAssign,
    #[token("<")]  Lt,
    #[token(">")]  Gt,
    #[token("!")]  Not,
    #[token("+")]  Plus,
    #[token("-")]  Minus,
    #[token("*")]  Star,
    #[token("/")]  Slash,
    #[token("%")]  Percent,
    #[token("=")]  Assign,
    #[token(".")]  Dot,

    // --- Brackets ---
    #[token("(")] LParen,
    #[token(")")] RParen,
    #[token("{")] LBrace,
    #[token("}")] RBrace,
    #[token("[")] LBracket,
    #[token("]")] RBracket,

    // --- Punctuation ---
    #[token(";")] Semicolon,
    #[token(",")] Comma,
    #[token(":")] Colon,
    #[token("?")] Question,
}

pub fn lex_code(src: &str) -> Result<Vec<Spanned<Token>>, Vec<ParseError>> {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();

    let mut lexer = Token::lexer(src);
    while let Some(result) = lexer.next() {
        match result {
            Ok(token) => tokens.push(Spanned { node: token, span: lexer.span() }),
            Err(_) => errors.push(ParseError {
                message: "Unexpected token".to_string(),
                span: lexer.span()
            }),
        }
    }

    if errors.is_empty() {
        Ok(tokens)
    } else {
        Err(errors)
    }
}
use crate::ast::*;
use crate::lexer::Span;
use crate::lexer::{ParseError, Spanned, SpannedToken, Token, merge_spans};

pub struct Parser<'a> {
    tokens: Vec<SpannedToken>,
    pos: usize,
    errors: Vec<ParseError>,
    source: &'a str,
}

impl<'a> Parser<'a> {
    pub fn new(tokens: Vec<SpannedToken>, source: &'a str) -> Self {
        Self {
            tokens,
            pos: 0,
            errors: Vec::new(),
            source,
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|t| &t.node)
    }

    #[allow(dead_code)]
    fn peek_spanned(&self) -> Option<&SpannedToken> {
        self.tokens.get(self.pos)
    }

    fn current_span(&self) -> Span {
        self.tokens
            .get(self.pos)
            .or_else(|| self.tokens.last())
            .map(|t| t.span.clone())
            .unwrap_or(0..0)
    }

    // End of the most recently consumed token (0 when nothing was consumed).
    fn last_consumed_end(&self) -> usize {
        self.tokens
            .get(self.pos.saturating_sub(1))
            .map(|t| t.span.end)
            .unwrap_or(0)
    }

    fn advance(&mut self) -> Option<&SpannedToken> {
        let t = self.tokens.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn is_at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    // Consume token if it matches, else emit error and return None
    fn expect(&mut self, expected: &Token, msg: &str) -> Option<Span> {
        if self.peek() == Some(expected) {
            let span = self.current_span();
            self.advance();
            Some(span)
        } else {
            let span = self.current_span();
            self.errors.push(ParseError {
                message: msg.to_string(),
                span,
            });
            None
        }
    }

    fn eat(&mut self, token: &Token) -> bool {
        if self.peek() == Some(token) {
            self.advance();
            true
        } else {
            false
        }
    }

    // Eat tokens until we find a safe restart point
    fn synchronize(&mut self) {
        loop {
            match self.peek() {
                None => break,
                Some(Token::Semicolon) => {
                    self.advance();
                    break;
                }
                Some(Token::RBrace) => break, // don't consume, parent needs it
                Some(
                    Token::Let
                    | Token::Const
                    | Token::Var
                    | Token::If
                    | Token::For
                    | Token::While
                    | Token::Return
                    | Token::Break
                    | Token::Continue
                    | Token::Function,
                ) => break,
                _ => {
                    self.advance();
                }
            }
        }
    }

    // fn error(&mut self, msg: &str) -> Span {
    //     let span = self.current_span();
    //     self.errors.push(ParseError { message: msg.to_string(), span: span.clone() });
    //     span
    // }

    pub fn parse(tokens: Vec<SpannedToken>, source: &str) -> (Vec<Stmt>, Vec<ParseError>) {
        let mut p = Parser::new(tokens, source);
        let stmts = p.parse_block_contents();
        (stmts, p.errors)
    }

    fn parse_block_contents(&mut self) -> Vec<Stmt> {
        let mut stmts = Vec::new();
        while !self.is_at_end() && self.peek() != Some(&Token::RBrace) {
            stmts.push(self.parse_stmt());
        }
        stmts
    }

    fn parse_block(&mut self) -> Vec<Stmt> {
        self.expect(&Token::LBrace, "expected `{`");
        let stmts = self.parse_block_contents();
        self.expect(&Token::RBrace, "expected `}`");
        stmts
    }

    fn parse_stmt(&mut self) -> Stmt {
        let start = self.current_span().start;

        let result = match self.peek() {
            Some(Token::Let) => self.parse_var_decl(VarKind::Let),
            Some(Token::Const) => self.parse_var_decl(VarKind::Const),
            Some(Token::Var) => self.parse_var_decl(VarKind::Var),
            Some(Token::Function) => self.parse_function_decl(),
            Some(Token::If) => self.parse_if(),
            Some(Token::While) => self.parse_while(),
            Some(Token::For) => self.parse_for(),
            Some(Token::Return) => self.parse_return(),
            Some(Token::Try) => self.parse_try(),
            Some(Token::Break) => {
                self.advance();
                self.eat(&Token::Semicolon);
                Ok(RawStmt::Break)
            }
            Some(Token::Continue) => {
                self.advance();
                self.eat(&Token::Semicolon);
                Ok(RawStmt::Continue)
            }
            _ => self.parse_expr_stmt(),
        };

        match result {
            Ok(node) => {
                let end = self
                    .tokens
                    .get(self.pos.saturating_sub(1))
                    .map(|t| t.span.end)
                    .unwrap_or(start);
                Spanned::new(node, start..end)
            }
            Err(e) => {
                self.errors.push(e);
                self.synchronize();
                let end = self.current_span().end;
                Spanned::new(RawStmt::Error, start..end)
            }
        }
    }

    fn parse_var_decl(&mut self, kind: VarKind) -> Result<RawStmt, ParseError> {
        self.advance(); // eat let/const/var
        let name = self.expect_ident("expected variable name")?;
        let init = if self.eat(&Token::Assign) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.eat(&Token::Semicolon);
        Ok(RawStmt::VarDecl { kind, name, init })
    }

    fn parse_function_decl(&mut self) -> Result<RawStmt, ParseError> {
        self.advance(); // eat `function`
        let name = self.expect_ident("expected function name")?;
        let params = self.parse_params()?;
        let body = self.parse_block();
        Ok(RawStmt::FunctionDecl { name, params, body })
    }

    fn parse_if(&mut self) -> Result<RawStmt, ParseError> {
        self.advance(); // eat `if`
        self.expect(&Token::LParen, "expected `(` after `if`");
        let cond = self.parse_expr()?;
        self.expect(&Token::RParen, "expected `)` after condition");
        let then = self.parse_block();
        let otherwise = if self.eat(&Token::Else) {
            // else if chains
            if self.peek() == Some(&Token::If) {
                let s = self.parse_stmt();
                Some(vec![s])
            } else {
                Some(self.parse_block())
            }
        } else {
            None
        };
        Ok(RawStmt::If {
            cond,
            then,
            otherwise,
        })
    }

    fn parse_while(&mut self) -> Result<RawStmt, ParseError> {
        self.advance(); // eat `while`
        self.expect(&Token::LParen, "expected `(` after `while`");
        let cond = self.parse_expr()?;
        self.expect(&Token::RParen, "expected `)` after condition");
        let body = self.parse_block();
        Ok(RawStmt::While { cond, body })
    }

    fn parse_for(&mut self) -> Result<RawStmt, ParseError> {
        self.advance(); // eat `for`
        self.expect(&Token::LParen, "expected `(` after `for`");

        // `for (x in y) { ... }` (the `in` lexes as an identifier)
        if let Some(for_in) = self.try_parse_for_in()? {
            return Ok(for_in);
        }

        // init
        let init = if self.peek() == Some(&Token::Semicolon) {
            self.advance();
            None
        } else {
            let s = self.parse_stmt();
            Some(Box::new(s))
        };

        // cond
        let cond = if self.peek() == Some(&Token::Semicolon) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(&Token::Semicolon, "expected `;`");

        // update
        let update = if self.peek() == Some(&Token::RParen) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(&Token::RParen, "expected `)`");

        let body = self.parse_block();
        Ok(RawStmt::For {
            init,
            cond,
            update,
            body,
        })
    }

    // Attempt to parse `for (x in y) { ... }`. Returns Ok(None) when the
    // tokens don't match the shape, in which case nothing was consumed and
    // the caller falls back to the C-style `for`.
    fn try_parse_for_in(&mut self) -> Result<Option<RawStmt>, ParseError> {
        let saved_pos = self.pos;
        let saved_errors = self.errors.len();

        // Optional `let`/`const`/`var` before the loop variable.
        match self.peek() {
            Some(Token::Let) | Some(Token::Const) | Some(Token::Var) => {
                self.advance();
            }
            _ => {}
        }

        let Some(Token::Ident(var)) = self.peek().cloned() else {
            self.pos = saved_pos;
            self.errors.truncate(saved_errors);
            return Ok(None);
        };
        self.advance();

        // `in` lexes as an identifier.
        if self.peek() != Some(&Token::Ident("in".to_string())) {
            self.pos = saved_pos;
            self.errors.truncate(saved_errors);
            return Ok(None);
        }
        self.advance();

        // Committed: this is a for-in loop.
        let iterable = self.parse_assign()?;
        self.expect(&Token::RParen, "expected `)` after `for (x in y)`");
        let body = self.parse_block();
        Ok(Some(RawStmt::ForIn {
            var,
            iterable,
            body,
        }))
    }

    fn parse_return(&mut self) -> Result<RawStmt, ParseError> {
        self.advance(); // eat `return`
        let value = if self.peek() == Some(&Token::Semicolon) || self.is_at_end() {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.eat(&Token::Semicolon);
        Ok(RawStmt::Return(value))
    }

    fn parse_try(&mut self) -> Result<RawStmt, ParseError> {
        self.advance(); // eat 'try'
        let stmt = self.parse_stmt();
        Ok(RawStmt::Try(Box::new(stmt)))
    }

    fn parse_expr_stmt(&mut self) -> Result<RawStmt, ParseError> {
        let expr = self.parse_expr()?;
        self.eat(&Token::Semicolon);
        Ok(RawStmt::Expr(expr))
    }

    // ---- Helpers ----

    fn expect_ident(&mut self, msg: &str) -> Result<String, ParseError> {
        match self.peek() {
            Some(Token::Ident(_)) => {
                let t = self.advance().unwrap();
                if let Token::Ident(s) = &t.node {
                    Ok(s.clone())
                } else {
                    unreachable!()
                }
            }
            _ => {
                let span = self.current_span();
                Err(ParseError {
                    message: msg.to_string(),
                    span,
                })
            }
        }
    }

    fn parse_params(&mut self) -> Result<Vec<String>, ParseError> {
        self.expect(&Token::LParen, "expected `(`");
        let mut params = Vec::new();
        while self.peek() != Some(&Token::RParen) && !self.is_at_end() {
            params.push(self.expect_ident("expected parameter name")?);
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect(&Token::RParen, "expected `)`");
        Ok(params)
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_assign()
    }

    // Assignment is right-associative so handled separately from Pratt
    fn parse_assign(&mut self) -> Result<Expr, ParseError> {
        let left = self.parse_pratt(0)?;

        let op = match self.peek() {
            Some(Token::Assign) => Some(AssignOp::Assign),
            Some(Token::PlusAssign) => Some(AssignOp::Add),
            Some(Token::MinusAssign) => Some(AssignOp::Sub),
            Some(Token::MulAssign) => Some(AssignOp::Mul),
            Some(Token::DivAssign) => Some(AssignOp::Div),
            Some(Token::ModAssign) => Some(AssignOp::Mod),
            Some(Token::BitAndAssign) => Some(AssignOp::BitAnd),
            Some(Token::BitOrAssign) => Some(AssignOp::BitOr),
            Some(Token::BitXorAssign) => Some(AssignOp::BitXor),
            Some(Token::ShlAssign) => Some(AssignOp::Shl),
            Some(Token::ShrAssign) => Some(AssignOp::Shr),
            _ => None,
        };

        if let Some(op) = op {
            self.advance();
            let right = self.parse_assign()?; // right-associative
            let span = merge_spans(&left.span, &right.span);
            return Ok(Spanned::new(
                RawExpr::Assign {
                    op,
                    target: Box::new(left),
                    value: Box::new(right),
                },
                span,
            ));
        }

        Ok(left)
    }

    fn parse_pratt(&mut self, min_bp: u8) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;

        loop {
            // Ternary
            if self.peek() == Some(&Token::Question) {
                if min_bp > 0 {
                    break;
                } // ternary has very low precedence
                self.advance();
                let then = self.parse_pratt(0)?;
                self.expect(&Token::Colon, "expected `:` in ternary");
                let otherwise = self.parse_pratt(0)?;
                let span = merge_spans(&left.span, &otherwise.span);
                left = Spanned::new(
                    RawExpr::Ternary {
                        cond: Box::new(left),
                        then: Box::new(then),
                        otherwise: Box::new(otherwise),
                    },
                    span,
                );
                continue;
            }

            let Some(op) = self.peek_binop() else { break };
            let (l_bp, r_bp) = infix_binding_power(&op);
            if l_bp < min_bp {
                break;
            }

            self.advance();
            let right = self.parse_pratt(r_bp)?;
            let span = merge_spans(&left.span, &right.span);
            left = Spanned::new(
                RawExpr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }

        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        let start = self.current_span().start;

        let prefix = match self.peek() {
            Some(Token::Not) => Some(PrefixOp::Not),
            Some(Token::Minus) => Some(PrefixOp::Neg),
            Some(Token::Typeof) => Some(PrefixOp::Typeof),
            Some(Token::PlusPlus) => Some(PrefixOp::PlusPlus),
            Some(Token::MinusMinus) => Some(PrefixOp::MinusMinus),
            _ => None,
        };

        if let Some(op) = prefix {
            self.advance();
            let expr = self.parse_unary()?;
            let end = expr.span.end;
            return Ok(Spanned::new(
                RawExpr::Prefix {
                    op,
                    expr: Box::new(expr),
                },
                start..end,
            ));
        }

        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_call()?;

        loop {
            match self.peek() {
                Some(Token::PlusPlus) => {
                    let end = self.current_span().end;
                    self.advance();
                    let span = expr.span.start..end;
                    expr = Spanned::new(
                        RawExpr::Postfix {
                            op: PostfixOp::PlusPlus,
                            expr: Box::new(expr),
                        },
                        span,
                    );
                }
                Some(Token::MinusMinus) => {
                    let end = self.current_span().end;
                    self.advance();
                    let span = expr.span.start..end;
                    expr = Spanned::new(
                        RawExpr::Postfix {
                            op: PostfixOp::MinusMinus,
                            expr: Box::new(expr),
                        },
                        span,
                    );
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_call(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;

        loop {
            match self.peek() {
                Some(Token::LParen) => {
                    self.advance();
                    let mut args = Vec::new();
                    while self.peek() != Some(&Token::RParen) && !self.is_at_end() {
                        args.push(self.parse_assign()?);
                        if !self.eat(&Token::Comma) {
                            break;
                        }
                    }
                    let end = self.current_span().end;
                    self.expect(&Token::RParen, "expected `)` after arguments");
                    let span = expr.span.start..end;
                    expr = Spanned::new(
                        RawExpr::Call {
                            callee: Box::new(expr),
                            args,
                        },
                        span,
                    );
                }
                Some(Token::Dot) => {
                    self.advance();
                    let prop = self.expect_ident("expected property name after `.`")?;
                    let end = self
                        .tokens
                        .get(self.pos.saturating_sub(1))
                        .map(|t| t.span.end)
                        .unwrap_or(0);
                    let span = expr.span.start..end;
                    expr = Spanned::new(
                        RawExpr::Member {
                            object: Box::new(expr),
                            property: prop,
                        },
                        span,
                    );
                }
                Some(Token::LBracket) => {
                    self.advance();
                    let index = self.parse_assign()?;
                    let end = self.current_span().end;
                    self.expect(&Token::RBracket, "expected `]`");
                    let span = expr.span.start..end;
                    expr = Spanned::new(
                        RawExpr::Index {
                            object: Box::new(expr),
                            index: Box::new(index),
                        },
                        span,
                    );
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let span = self.current_span();

        match self.peek().cloned() {
            Some(Token::Number(n)) => {
                self.advance();
                Ok(Spanned::new(RawExpr::Number(n), span))
            }
            Some(Token::StringDouble(s) | Token::StringSingle(s)) => {
                self.advance();
                Ok(Spanned::new(RawExpr::StringLit(s), span))
            }
            Some(Token::True) => {
                self.advance();
                Ok(Spanned::new(RawExpr::Bool(true), span))
            }
            Some(Token::False) => {
                self.advance();
                Ok(Spanned::new(RawExpr::Bool(false), span))
            }
            Some(Token::Null) => {
                self.advance();
                Ok(Spanned::new(RawExpr::Null, span))
            }

            Some(Token::Ident(_)) => self.parse_ident_or_arrow(),

            Some(Token::LParen) => self.parse_paren_or_arrow(),

            Some(Token::LBracket) => {
                self.advance();
                let mut items = Vec::new();
                while self.peek() != Some(&Token::RBracket) && !self.is_at_end() {
                    items.push(self.parse_assign()?);
                    if !self.eat(&Token::Comma) {
                        break;
                    }
                }
                let end = self.current_span().end;
                self.expect(&Token::RBracket, "expected `]`");
                Ok(Spanned::new(RawExpr::Array(items), span.start..end))
            }

            Some(Token::LBrace) => {
                self.advance();
                let mut pairs = Vec::new();
                while self.peek() != Some(&Token::RBrace) && !self.is_at_end() {
                    let key = match self.peek().cloned() {
                        Some(Token::Ident(name)) => {
                            self.advance();
                            name
                        }
                        Some(Token::StringDouble(s) | Token::StringSingle(s)) => {
                            self.advance();
                            s
                        }
                        _ => {
                            return Err(ParseError {
                                message: "expected object key".to_string(),
                                span: self.current_span(),
                            });
                        }
                    };
                    self.expect(&Token::Colon, "expected `:` after object key");
                    let value = self.parse_assign()?;
                    pairs.push((key, value));
                    if !self.eat(&Token::Comma) {
                        break;
                    }
                }
                let end = self.current_span().end;
                self.expect(&Token::RBrace, "expected `}`");
                Ok(Spanned::new(RawExpr::Object(pairs), span.start..end))
            }

            Some(Token::Lt) => self.parse_html_template(),

            _ => {
                let span = self.current_span();
                Err(ParseError {
                    message: format!(
                        "unexpected token `{:?}`",
                        self.peek().cloned().unwrap_or(Token::Semicolon)
                    ),
                    span,
                })
            }
        }
    }

    // ---- HTML templates: <><p>{expr}</p></> ----
    //
    // The parser works on the token stream, so text between tags and `{}`
    // slots is reconstructed by slicing the original source between token
    // spans (whitespace is skipped by the lexer but preserved this way).

    // Entry point, called from parse_primary with the current token being `<`.
    fn parse_html_template(&mut self) -> Result<Expr, ParseError> {
        let start = self.current_span().start;
        let nodes = match self.tokens.get(self.pos + 1).map(|t| &t.node) {
            // `<> ... </>`
            Some(Token::Gt) => {
                self.advance(); // `<`
                self.advance(); // `>`
                self.parse_template_children(None)?
            }
            // `<tag> ... </tag>` or `<tag/>`
            Some(Token::Ident(tag)) => {
                let tag = tag.clone();
                self.advance(); // `<`
                self.advance(); // tag
                let attrs = self.parse_template_attrs()?;
                if self.peek() == Some(&Token::Slash) {
                    self.advance();
                    self.expect(&Token::Gt, "expected `>` after self-closing tag");
                    vec![TemplateNode::Element {
                        tag,
                        attrs,
                        children: vec![],
                    }]
                } else {
                    self.expect(&Token::Gt, "expected `>` after tag name");
                    let children = self.parse_template_children(Some(&tag))?;
                    vec![TemplateNode::Element {
                        tag,
                        attrs,
                        children,
                    }]
                }
            }
            _ => {
                return Err(ParseError {
                    message: "expected `<>` or `<tag>` after `<`".to_string(),
                    span: self.current_span(),
                });
            }
        };
        let end = self.current_span().end;
        Ok(Spanned::new(RawExpr::HtmlTemplate(nodes), start..end))
    }

    // Parse template children until the matching close tag. `expect_close`
    // is the tag name that must close this level (None means a fragment,
    // which must close with `</>`).
    fn parse_template_children(
        &mut self,
        expect_close: Option<&str>,
    ) -> Result<Vec<TemplateNode>, ParseError> {
        let mut nodes = Vec::new();
        // Text starts after the open tag's closing `>`, so whitespace that
        // the lexer skipped between tokens still lands inside the text.
        let mut text_start = self.last_consumed_end();

        loop {
            let token_start = self.current_span().start;
            match self.peek() {
                None => {
                    return Err(ParseError {
                        message: "unclosed template tag".to_string(),
                        span: self.current_span(),
                    });
                }
                Some(Token::Lt) => match self.tokens.get(self.pos + 1).map(|t| &t.node) {
                    Some(Token::Gt) => {
                        // `<> ... </>` nested fragment — flatten into this level
                        if token_start > text_start {
                            nodes.push(TemplateNode::Text(
                                self.source[text_start..token_start].to_string(),
                            ));
                        }
                        self.advance(); // `<`
                        self.advance(); // `>`
                        let inner = self.parse_template_children(None)?;
                        nodes.extend(inner);
                        text_start = self.last_consumed_end();
                    }
                    Some(Token::Slash) => {
                        // Close tag `</...>` or `</>`
                        if token_start > text_start {
                            nodes.push(TemplateNode::Text(
                                self.source[text_start..token_start].to_string(),
                            ));
                        }
                        self.advance(); // `<`
                        self.advance(); // `/`
                        match self.peek() {
                            Some(Token::Gt) => {
                                self.advance(); // `>`
                                if expect_close.is_some() {
                                    return Err(ParseError {
                                        message: "expected a closing tag, found `</>`".to_string(),
                                        span: self.current_span(),
                                    });
                                }
                                return Ok(nodes);
                            }
                            Some(Token::Ident(tag)) => {
                                let tag = tag.clone();
                                self.advance();
                                self.expect(&Token::Gt, "expected `>` after closing tag");
                                return match expect_close {
                                    Some(expected) if expected == tag => Ok(nodes),
                                    Some(expected) => Err(ParseError {
                                        message: format!(
                                            "expected `</{}>` but found `</{}>`",
                                            expected, tag
                                        ),
                                        span: self.current_span(),
                                    }),
                                    None => Err(ParseError {
                                        message: format!("unexpected closing tag `</{}>`", tag),
                                        span: self.current_span(),
                                    }),
                                };
                            }
                            _ => {
                                return Err(ParseError {
                                    message: "expected a tag name after `</`".to_string(),
                                    span: self.current_span(),
                                });
                            }
                        }
                    }
                    Some(Token::Ident(tag)) => {
                        // Element `<tag> ... </tag>` or `<tag/>`
                        if token_start > text_start {
                            nodes.push(TemplateNode::Text(
                                self.source[text_start..token_start].to_string(),
                            ));
                        }
                        let tag = tag.clone();
                        self.advance(); // `<`
                        self.advance(); // tag
                        let attrs = self.parse_template_attrs()?;
                        if self.peek() == Some(&Token::Slash) {
                            self.advance();
                            self.expect(&Token::Gt, "expected `>` after self-closing tag");
                            nodes.push(TemplateNode::Element {
                                tag,
                                attrs,
                                children: vec![],
                            });
                        } else {
                            self.expect(&Token::Gt, "expected `>` after tag name");
                            let children = self.parse_template_children(Some(&tag))?;
                            nodes.push(TemplateNode::Element {
                                tag,
                                attrs,
                                children,
                            });
                        }
                        text_start = self.last_consumed_end();
                    }
                    _ => {
                        return Err(ParseError {
                            message: "expected a template tag after `<`".to_string(),
                            span: self.current_span(),
                        });
                    }
                },
                Some(Token::LBrace) => {
                    // `{ expr }` slot
                    if token_start > text_start {
                        nodes.push(TemplateNode::Text(
                            self.source[text_start..token_start].to_string(),
                        ));
                    }
                    self.advance(); // `{`
                    let expr = self.parse_assign()?;
                    self.expect(&Token::RBrace, "expected `}` after template expression");
                    nodes.push(TemplateNode::Expr(Box::new(expr)));
                    text_start = self.last_consumed_end();
                }
                Some(_) => {
                    // Ordinary token — part of the surrounding text.
                    self.advance();
                }
            }
        }
    }

    // Parse the attributes between a template tag name and its closing `>`:
    //   class="primary"                        static value (as written)
    //   class={primary ? "primary" : ""}       evaluated + escaped at render
    //   disabled                               boolean (emits just the name)
    fn parse_template_attrs(&mut self) -> Result<Vec<Attr>, ParseError> {
        let mut attrs = Vec::new();
        loop {
            match self.peek() {
                Some(Token::Gt) | Some(Token::Slash) => break,
                Some(Token::Ident(name)) => {
                    let name = name.clone();
                    self.advance();
                    if self.eat(&Token::Assign) {
                        match self.peek() {
                            Some(Token::LBrace) => {
                                self.advance(); // `{`
                                let expr = self.parse_assign()?;
                                self.expect(
                                    &Token::RBrace,
                                    "expected `}` after attribute expression",
                                );
                                attrs.push(Attr::Expr(name, Box::new(expr)));
                            }
                            Some(Token::StringDouble(value) | Token::StringSingle(value)) => {
                                let value = value.clone();
                                self.advance();
                                attrs.push(Attr::Static(name, value));
                            }
                            _ => {
                                return Err(ParseError {
                                    message: "expected `{expr}` or a quoted value after `=`"
                                        .to_string(),
                                    span: self.current_span(),
                                });
                            }
                        }
                    } else {
                        attrs.push(Attr::Bool(name));
                    }
                }
                _ => {
                    return Err(ParseError {
                        message: "expected an attribute name".to_string(),
                        span: self.current_span(),
                    });
                }
            }
        }
        Ok(attrs)
    }

    // Single bare ident, could be the start of an arrow: `x => ...`
    fn parse_ident_or_arrow(&mut self) -> Result<Expr, ParseError> {
        let span = self.current_span();
        let name = self.expect_ident("expected identifier")?;
        if self.peek() == Some(&Token::Arrow) {
            self.advance(); // eat =>
            let body = self.parse_arrow_body()?;
            let end = self
                .tokens
                .get(self.pos.saturating_sub(1))
                .map(|t| t.span.end)
                .unwrap_or(span.end);
            return Ok(Spanned::new(
                RawExpr::Arrow {
                    params: vec![name],
                    body,
                },
                span.start..end,
            ));
        }

        Ok(Spanned::new(RawExpr::Ident(name), span))
    }

    // `(...)`, either a grouped expr or arrow params
    fn parse_paren_or_arrow(&mut self) -> Result<Expr, ParseError> {
        let start = self.current_span().start;
        self.advance(); // eat `(`

        // Collect what look like param names, watching for a `)` followed by `=>`
        let mut names: Vec<String> = Vec::new();
        let mut is_arrow = false;

        // Empty parens: () =>
        if self.peek() == Some(&Token::RParen) {
            self.advance();
            if self.peek() == Some(&Token::Arrow) {
                self.advance();
                is_arrow = true;
            }
        } else {
            // Speculatively try to collect `ident, ident, ...`
            let saved_pos = self.pos;
            let saved_errors = self.errors.len();
            let mut all_idents = true;

            while !self.is_at_end() {
                match self.peek().cloned() {
                    Some(Token::Ident(n)) => {
                        self.advance();
                        names.push(n);
                        if self.peek() == Some(&Token::Comma) {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    _ => {
                        all_idents = false;
                        break;
                    }
                }
            }

            if all_idents && self.peek() == Some(&Token::RParen) {
                self.advance(); // eat `)`
                if self.peek() == Some(&Token::Arrow) {
                    self.advance(); // eat `=>`
                    is_arrow = true;
                } else {
                    // Not an arrow, backtrack and parse as grouped expr
                    self.pos = saved_pos;
                    self.errors.truncate(saved_errors);
                }
            } else {
                // Not all idents, backtrack
                self.pos = saved_pos;
                self.errors.truncate(saved_errors);
            }
        }

        if is_arrow {
            let body = self.parse_arrow_body()?;
            let end = self
                .tokens
                .get(self.pos.saturating_sub(1))
                .map(|t| t.span.end)
                .unwrap_or(0);
            return Ok(Spanned::new(
                RawExpr::Arrow {
                    params: names,
                    body,
                },
                start..end,
            ));
        }

        // Plain grouped expression
        let inner = self.parse_assign()?;
        let end = self.current_span().end;
        self.expect(&Token::RParen, "expected `)`");
        Ok(Spanned::new(inner.node, start..end))
    }

    fn parse_arrow_body(&mut self) -> Result<ArrowBody, ParseError> {
        if self.peek() == Some(&Token::LBrace) {
            Ok(ArrowBody::Block(self.parse_block()))
        } else {
            Ok(ArrowBody::Expr(Box::new(self.parse_assign()?)))
        }
    }

    fn peek_binop(&self) -> Option<BinOp> {
        match self.peek()? {
            Token::Plus => Some(BinOp::Add),
            Token::Minus => Some(BinOp::Sub),
            Token::Star => Some(BinOp::Mul),
            Token::Slash => Some(BinOp::Div),
            Token::Percent => Some(BinOp::Mod),
            Token::BitAnd => Some(BinOp::BitAnd),
            Token::BitOr => Some(BinOp::BitOr),
            Token::BitXor => Some(BinOp::BitXor),
            Token::Shl => Some(BinOp::Shl),
            Token::Shr => Some(BinOp::Shr),
            Token::StrictEq => Some(BinOp::StrictEq),
            Token::StrictNeq => Some(BinOp::StrictNeq),
            Token::Eq => Some(BinOp::Eq),
            Token::Neq => Some(BinOp::Neq),
            Token::Lt => Some(BinOp::Lt),
            Token::Lte => Some(BinOp::Lte),
            Token::Gt => Some(BinOp::Gt),
            Token::Gte => Some(BinOp::Gte),
            Token::And => Some(BinOp::And),
            Token::Or => Some(BinOp::Or),
            _ => None,
        }
    }
}

// Binding powers, higher = tighter binding
fn infix_binding_power(op: &BinOp) -> (u8, u8) {
    match op {
        BinOp::Or => (1, 2),
        BinOp::And => (3, 4),
        BinOp::BitOr => (5, 6),
        BinOp::BitXor => (7, 8),
        BinOp::BitAnd => (9, 10),
        BinOp::Eq | BinOp::Neq | BinOp::StrictEq | BinOp::StrictNeq => (11, 12),
        BinOp::Lt | BinOp::Lte | BinOp::Gt | BinOp::Gte => (13, 14),
        BinOp::Shl | BinOp::Shr => (15, 16),
        BinOp::Add | BinOp::Sub => (17, 18),
        BinOp::Mul | BinOp::Div | BinOp::Mod => (19, 20),
    }
}

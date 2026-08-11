mod parser;
mod lexer;
mod ast;
mod value;
mod eval;

pub fn parse() {
    let src = "1 + 2";
    let tokens = lexer::lex_code(src).unwrap();
    let (stmts, _) = parser::Parser::parse(tokens);
    let mut evalulator = eval::Evaluator::new();
    let env = value::Env::new_root();
    evalulator.eval_stmts(&stmts, env).unwrap();
    dbg!(evalulator.output);
}
mod parser;
mod lexer;
mod ast;
mod value;
mod eval;

pub fn evaluate(src: &str) -> String {
    let tokens = lexer::lex_code(src).unwrap();
    let (stmts, _) = parser::Parser::parse(tokens);
    let mut evalulator = eval::Evaluator::new();
    let env = value::Env::new_root();
    evalulator.eval_stmts(&stmts, env).unwrap();
    evalulator.output
}

#[cfg(test)]
#[path = "./lib_tests.rs"]
mod tests;
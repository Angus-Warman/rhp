use std::{cell::RefCell, collections::HashMap, rc::Rc};

use crate::value::{
    Env, Function,
    FunctionBody::{self},
    Value,
};

mod ast;
mod eval;
mod lexer;
mod parser;
mod value;

pub fn evaluate(src: &str) -> String {
    let tokens = lexer::lex_code(src).unwrap();
    let (stmts, _) = parser::Parser::parse(tokens);
    let mut evalulator = eval::Evaluator::new();
    let env = value::Env::new_root();

    { // Scopes env_mut
        let mut env_mut = env.borrow_mut();
        env_mut.define("VERSION", value::Value::String("0.0.1".to_string()));

        let log = Value::Function(Function {
            params: vec!["value".to_string()],
            body: FunctionBody::Native(Rc::new(|args| {
                let output = args
                    .iter()
                    .map(|v| v.display())
                    .collect::<Vec<_>>()
                    .join(" ");
                println!("{}", output);
                Ok(Value::Null)
            })),
            captured: Env::new_root(),
        });

        let console = Value::Object(Rc::new(RefCell::new({
            let mut map = HashMap::new();
            map.insert("log".to_string(), log);
            map
        })));

        env_mut.define("console", console);
    }

    evalulator.eval_stmts(&stmts, env).unwrap();
    evalulator.output
}

#[cfg(test)]
#[path = "./lib_tests.rs"]
mod tests;

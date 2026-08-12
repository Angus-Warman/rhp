use std::{cell::RefCell, collections::HashMap, rc::Rc};

use crate::{eval::Evaluator, lexer, parser::Parser, value::{
    self, Env, Function, FunctionBody::{self}, Value,
}};

#[allow(dead_code)]
pub fn process_src(src: &str) -> String {
    let env = setup_env();
    let mut output = "".to_string();

    let sections = split_src(src);

    for section in sections {
        match section {
            Section::Html(html) => output += &html,
            Section::Code(script) => {
                let result = process_script_section(env.clone(), &script); 
                output += &result;
            },
        }
    }

    return output;
}

#[derive(Debug, Clone, PartialEq)]
pub enum Section {
    Html(String),
    Code(String),
}

fn split_src(src: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut rest = src;

    while !rest.is_empty() {
        match rest.find("<rhp>") {
            None => {
                // No more code blocks — remainder is HTML
                sections.push(Section::Html(rest.to_string()));
                break;
            }
            Some(html_end) => {
                // Capture HTML before the tag
                if html_end > 0 {
                    sections.push(Section::Html(rest[..html_end].to_string()));
                }

                let after_open = &rest[html_end + "<rhp>".len()..];

                match after_open.find("</rhp>") {
                    None => {
                        // Unclosed tag — treat the rest as a code block anyway,
                        // or you could return an Err here
                        sections.push(Section::Code(after_open.to_string()));
                        break;
                    }
                    Some(code_end) => {
                        sections.push(Section::Code(after_open[..code_end].to_string()));
                        rest = &after_open[code_end + "</rhp>".len()..];
                    }
                }
            }
        }
    }

    sections
}

fn setup_env() -> Rc<RefCell<Env>> {
    let env = Env::new_root();

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

    env
}

fn process_script_section(env: Rc<RefCell<Env>>, script: &str) -> String {
    let tokens = lexer::lex_code(script).unwrap();
    let (stmts, _) = Parser::parse(tokens);
    let mut evalulator = Evaluator::new();
    

    evalulator.eval_stmts(&stmts, env).unwrap();
    evalulator.output
}

#[cfg(test)]
#[path = "./process_tests.rs"]
mod process_tests;

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use crate::{eval::Evaluator, lexer, parser::Parser, value::{
    self, Env, Function, FunctionBody::{self}, Value,
}};

#[allow(dead_code)]
pub fn process_src(src: &str, method: &str) -> String {
    let env = setup_env();
    let mut output = "".to_string();

    let sections = split_src(src);

    for section in sections {
        match section {
            Section::Html(html) => output += &html,
            Section::Code { code, method: None } => {
                let result = process_script_section(env.clone(), &code);
                output += &result;
            },
            Section::Code { code, method: Some(m) } if m.eq_ignore_ascii_case(method) => {
                let result = process_script_section(env.clone(), &code);
                output += &result;
            },
            Section::Code { .. } => {},
        }
    }

    return output;
}

#[derive(Debug, Clone, PartialEq)]
pub enum Section {
    Html(String),
    Code { code: String, method: Option<String> },
}

fn parse_method(attrs: &str) -> Option<String> {
    const PREFIX: &str = "method=";
    let i = attrs.find(PREFIX)?;
    let rest = attrs[i + PREFIX.len()..].trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &rest[1..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn split_src(src: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut rest = src;

    while !rest.is_empty() {
        match rest.find("<rhp") {
            None => {
                // No more code blocks — remainder is HTML
                sections.push(Section::Html(rest.to_string()));
                break;
            }
            Some(start) => {
                // Capture HTML before the tag
                if start > 0 {
                    sections.push(Section::Html(rest[..start].to_string()));
                }

                let after_open = &rest[start + "<rhp".len()..];
                let (method, body) = match after_open.find('>') {
                    Some(gt) => (parse_method(&after_open[..gt]), &after_open[gt + 1..]),
                    None => (None, after_open),
                };

                match body.find("</rhp>") {
                    None => {
                        // Unclosed tag — treat the rest as a code block anyway,
                        // or you could return an Err here
                        sections.push(Section::Code { code: body.to_string(), method });
                        break;
                    }
                    Some(code_end) => {
                        sections.push(Section::Code {
                            code: body[..code_end].to_string(),
                            method,
                        });
                        rest = &body[code_end + "</rhp>".len()..];
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

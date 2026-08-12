use super::*;

fn test_process(script: &str) -> String {
    let env = setup_env(&Context::default());
    process_script_section(env, script)
}

fn ctx(method: Method) -> Context {
    Context { method, query: HashMap::new() }
}

#[test]
fn test_basic_eval() {
    assert_eq!(test_process("return 1 + 2"), "3");
}

#[test]
fn test_pass_function() {
    assert_eq!(test_process(r"
        let inc = (a) => a + 1
        let apply_twice = (f, n) => f(f(n))
        return apply_twice(inc, 2) 
    "), "4");
}

#[test]
fn test_global_constants() {
    assert_eq!(test_process(r"
        return VERSION 
    "), "0.0.1");
}

#[test]
fn test_console_log() {
    assert_eq!(test_process(r"
        console.log('hello world') 
    "), "");
}

#[test]
fn test_method_filtered_sections() {
    let src = r#"<rhp method="PUT">return "put"</rhp><rhp method="POST">return "post"</rhp>"#;
    assert_eq!(process_src(src, &ctx(Method::Post)), "post");
    assert_eq!(process_src(src, &ctx(Method::Put)), "put");
    assert_eq!(process_src(src, &ctx(Method::Get)), "");
}

#[test]
fn test_unfiltered_section_runs_all_methods() {
    let src = r#"<rhp>return "always"</rhp>"#;
    assert_eq!(process_src(src, &ctx(Method::Get)), "always");
    assert_eq!(process_src(src, &ctx(Method::Post)), "always");
}

#[test]
fn test_split_src_parses_method_attr() {
    let src = r#"<rhp method="PUT">a</rhp><rhp>b</rhp><rhp method='POST'>c</rhp>"#;
    assert_eq!(
        split_src(src),
        vec![
            Section::Code { code: "a".into(), method: Method::Put },
            Section::Code { code: "b".into(), method: Method::All },
            Section::Code { code: "c".into(), method: Method::Post },
        ]
    );
}

#[test]
fn test_object_prop() {
    assert_eq!(test_process(r"
        let a = {}
        a.b = 1
        return a
    "), "{ b: 1 }");
}

#[test]
fn test_null_value() {
    assert_eq!(test_process("return null"), "null");
}

#[test]
fn test_bool_values() {
    assert_eq!(test_process("return true"), "true");
    assert_eq!(test_process("return false"), "false");
}

#[test]
fn test_array_value() {
    assert_eq!(test_process("return [1, 2, 3]"), "[1, 2, 3]");
}

#[test]
fn test_function_value_display() {
    assert_eq!(test_process("return (a) => a"), "[function]");
}

#[test]
fn test_context_from_request() {
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/foo.rhp?id=123&name=hello")
        .body(())
        .unwrap();
    let context = Context::from_request(&request);
    assert_eq!(context.method, Method::Post);
    assert_eq!(context.query.get("id").map(String::as_str), Some("123"));
    assert_eq!(context.query.get("name").map(String::as_str), Some("hello"));
    assert_eq!(context.query.len(), 2);
}

#[test]
fn test_context_from_request_without_query() {
    let request = axum::http::Request::builder()
        .method("GET")
        .uri("/foo.rhp")
        .body(())
        .unwrap();
    let context = Context::from_request(&request);
    assert_eq!(context.method, Method::Get);
    assert!(context.query.is_empty());
}

#[test]
fn test_query_global_empty() {
    assert_eq!(test_process("return QUERY"), "{}");
}

#[test]
fn test_query_global_populated() {
    let context = Context {
        method: Method::Get,
        query:  HashMap::from([("id".to_string(), "123".to_string())]),
    };
    let env = setup_env(&context);
    assert_eq!(process_script_section(env, "return QUERY.id"), "123");
}
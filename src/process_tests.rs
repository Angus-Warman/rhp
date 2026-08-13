use super::*;

fn test_process(script: &str) -> String {
    let env = setup_env(&Context::default());
    process_script_section(env, script)
}

fn ctx(method: Method) -> Context {
    Context { method, query: HashMap::new(), body: empty_object() }
}

fn request_with_body(method: &str, uri: &str, content_type: Option<&str>, body: &str) -> Request {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(ct) = content_type {
        builder = builder.header("content-type", ct);
    }
    builder.body(axum::body::Body::from(body.to_string())).unwrap()
}

fn eval_with_context(context: &Context, script: &str) -> String {
    let env = setup_env(context);
    process_script_section(env, script)
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

#[tokio::test]
async fn test_context_from_request() {
    let request = request_with_body("POST", "/foo.rhp?id=123&name=hello", None, "");
    let context = Context::from_request(request).await.unwrap();
    assert_eq!(context.method, Method::Post);
    assert_eq!(context.query.get("id").map(String::as_str), Some("123"));
    assert_eq!(context.query.get("name").map(String::as_str), Some("hello"));
    assert_eq!(context.query.len(), 2);
    assert_eq!(context.body.display(), "{}");
}

#[tokio::test]
async fn test_context_from_request_without_query() {
    let request = request_with_body("GET", "/foo.rhp", None, "");
    let context = Context::from_request(request).await.unwrap();
    assert_eq!(context.method, Method::Get);
    assert!(context.query.is_empty());
    assert_eq!(context.body.display(), "{}");
}

#[tokio::test]
async fn test_body_text() {
    let context = Context::from_request(
        request_with_body("POST", "/x", Some("text/plain"), "hello world"),
    ).await.unwrap();
    assert_eq!(eval_with_context(&context, "return BODY.text"), "hello world");
}

#[tokio::test]
async fn test_body_json_object() {
    let context = Context::from_request(
        request_with_body("POST", "/x", Some("application/json"), r#"{"name":"rhp","count":2}"#),
    ).await.unwrap();
    assert_eq!(eval_with_context(&context, "return BODY.name"), "rhp");
    assert_eq!(eval_with_context(&context, "return BODY.count"), "2");
}

#[tokio::test]
async fn test_body_json_nested() {
    let context = Context::from_request(
        request_with_body("POST", "/x", Some("application/json"), r#"{"user":{"age":3}}"#),
    ).await.unwrap();
    assert_eq!(eval_with_context(&context, "return BODY.user.age"), "3");
}

#[tokio::test]
async fn test_body_json_array() {
    let context = Context::from_request(
        request_with_body("POST", "/x", Some("application/json"), "[1, 2, 3]"),
    ).await.unwrap();
    assert_eq!(eval_with_context(&context, "return BODY"), "[1, 2, 3]");
}

#[tokio::test]
async fn test_body_json_primitive() {
    let context = Context::from_request(
        request_with_body("POST", "/x", Some("application/json"), "5"),
    ).await.unwrap();
    assert_eq!(eval_with_context(&context, "return BODY"), "5");
}

#[tokio::test]
async fn test_body_form() {
    let context = Context::from_request(
        request_with_body("POST", "/x", Some("application/x-www-form-urlencoded"), "a=1&b=hello"),
    ).await.unwrap();
    assert_eq!(eval_with_context(&context, "return BODY.a"), "1");
    assert_eq!(eval_with_context(&context, "return BODY.as"), "[1]");
    assert_eq!(eval_with_context(&context, "return BODY.b"), "hello");
    assert_eq!(eval_with_context(&context, "return BODY.bs"), "[hello]");
    assert_eq!(eval_with_context(&context, "return BODY.c"), "null");
}

#[tokio::test]
async fn test_body_form_duplicate_values() {
    let context = Context::from_request(
        request_with_body("POST", "/x", Some("application/x-www-form-urlencoded"), "color=red&color=blue"),
    ).await.unwrap();
    assert_eq!(eval_with_context(&context, "return BODY.color"), "red"); // Gets the first
    assert_eq!(eval_with_context(&context, "return BODY.colors"), "[red, blue]");
}

#[tokio::test]
async fn test_body_empty_for_get() {
    let context = Context::from_request(
        request_with_body("GET", "/x", Some("text/plain"), "ignored"),
    ).await.unwrap();
    assert_eq!(eval_with_context(&context, "return BODY"), "{}");
}

#[tokio::test]
async fn test_body_empty_body() {
    let context = Context::from_request(
        request_with_body("POST", "/x", Some("text/plain"), ""),
    ).await.unwrap();
    assert_eq!(eval_with_context(&context, "return BODY"), "{}");
}

#[tokio::test]
async fn test_body_invalid_json_errors() {
    let result = Context::from_request(
        request_with_body("POST", "/x", Some("application/json"), "{not json}"),
    ).await;
    assert!(matches!(result, Err(ContextError::Json(_))));
}

#[test]
fn test_query_global_empty() {
    assert_eq!(test_process("return QUERY"), "{}");
}

#[tokio::test]
async fn test_query_object() {
    let context = Context::from_request(
        request_with_body("GET", "/index.rhp?id=123&name=hello", None, ""),
    ).await.unwrap();
    let env = setup_env(&context);
    assert_eq!(process_script_section(env, "return QUERY.id"), "123");
}
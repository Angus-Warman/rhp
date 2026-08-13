use super::*;

async fn test_process(script: &str) -> String {
    let env = setup_env(&Context::default());
    process_script_section(env, script).await
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

async fn eval_with_context(context: &Context, script: &str) -> String {
    let env = setup_env(context);
    process_script_section(env, script).await
}

#[tokio::test]
async fn test_basic_eval() {
    assert_eq!(test_process("return 1 + 2").await, "3");
}

#[tokio::test]
async fn test_pass_function() {
    assert_eq!(test_process(r"
        let inc = (a) => a + 1
        let apply_twice = (f, n) => f(f(n))
        return apply_twice(inc, 2) 
    ").await, "4");
}

#[tokio::test]
async fn test_global_constants() {
    assert_eq!(test_process(r"
        return VERSION 
    ").await, "0.0.1");
}

#[tokio::test]
async fn test_console_log() {
    assert_eq!(test_process(r"
        console.log('hello world') 
    ").await, "");
}

#[tokio::test]
async fn test_method_filtered_sections() {
    let src = r#"<rhp method="PUT">return "put"</rhp><rhp method="POST">return "post"</rhp>"#;
    assert_eq!(process_src(src.to_string(), ctx(Method::Post)).await, "post");
    assert_eq!(process_src(src.to_string(), ctx(Method::Put)).await, "put");
    assert_eq!(process_src(src.to_string(), ctx(Method::Get)).await, "");
}

#[tokio::test]
async fn test_unfiltered_section_runs_all_methods() {
    let src = r#"<rhp>return "always"</rhp>"#;
    assert_eq!(process_src(src.to_string(), ctx(Method::Get)).await, "always");
    assert_eq!(process_src(src.to_string(), ctx(Method::Post)).await, "always");
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

#[tokio::test]
async fn test_object_prop() {
    assert_eq!(test_process(r"
        let a = {}
        a.b = 1
        return a
    ").await, "{ b: 1 }");
}

#[tokio::test]
async fn test_null_value() {
    assert_eq!(test_process("return null").await, "null");
}

#[tokio::test]
async fn test_bool_values() {
    assert_eq!(test_process("return true").await, "true");
    assert_eq!(test_process("return false").await, "false");
}

#[tokio::test]
async fn test_array_value() {
    assert_eq!(test_process("return [1, 2, 3]").await, "[1, 2, 3]");
}

#[tokio::test]
async fn test_function_value_display() {
    assert_eq!(test_process("return (a) => a").await, "[function]");
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
    assert_eq!(eval_with_context(&context, "return BODY.text").await, "hello world");
}

#[tokio::test]
async fn test_body_json_object() {
    let context = Context::from_request(
        request_with_body("POST", "/x", Some("application/json"), r#"{"name":"rhp","count":2}"#),
    ).await.unwrap();
    assert_eq!(eval_with_context(&context, "return BODY.name").await, "rhp");
    assert_eq!(eval_with_context(&context, "return BODY.count").await, "2");
}

#[tokio::test]
async fn test_body_json_nested() {
    let context = Context::from_request(
        request_with_body("POST", "/x", Some("application/json"), r#"{"user":{"age":3}}"#),
    ).await.unwrap();
    assert_eq!(eval_with_context(&context, "return BODY.user.age").await, "3");
}

#[tokio::test]
async fn test_body_json_array() {
    let context = Context::from_request(
        request_with_body("POST", "/x", Some("application/json"), "[1, 2, 3]"),
    ).await.unwrap();
    assert_eq!(eval_with_context(&context, "return BODY").await, "[1, 2, 3]");
}

#[tokio::test]
async fn test_body_json_primitive() {
    let context = Context::from_request(
        request_with_body("POST", "/x", Some("application/json"), "5"),
    ).await.unwrap();
    assert_eq!(eval_with_context(&context, "return BODY").await, "5");
}

#[tokio::test]
async fn test_body_form() {
    let context = Context::from_request(
        request_with_body("POST", "/x", Some("application/x-www-form-urlencoded"), "a=1&b=hello"),
    ).await.unwrap();
    assert_eq!(eval_with_context(&context, "return BODY.a").await, "1");
    assert_eq!(eval_with_context(&context, "return BODY.as").await, "[1]");
    assert_eq!(eval_with_context(&context, "return BODY.b").await, "hello");
    assert_eq!(eval_with_context(&context, "return BODY.bs").await, "[hello]");
    assert_eq!(eval_with_context(&context, "return BODY.c").await, "null");
}

#[tokio::test]
async fn test_body_form_duplicate_values() {
    let context = Context::from_request(
        request_with_body("POST", "/x", Some("application/x-www-form-urlencoded"), "color=red&color=blue"),
    ).await.unwrap();
    assert_eq!(eval_with_context(&context, "return BODY.color").await, "red"); // Gets the first
    assert_eq!(eval_with_context(&context, "return BODY.colors").await, "[red, blue]");
}

#[tokio::test]
async fn test_body_empty_for_get() {
    let context = Context::from_request(
        request_with_body("GET", "/x", Some("text/plain"), "ignored"),
    ).await.unwrap();
    assert_eq!(eval_with_context(&context, "return BODY").await, "{}");
}

#[tokio::test]
async fn test_body_empty_body() {
    let context = Context::from_request(
        request_with_body("POST", "/x", Some("text/plain"), ""),
    ).await.unwrap();
    assert_eq!(eval_with_context(&context, "return BODY").await, "{}");
}

#[tokio::test]
async fn test_body_invalid_json_errors() {
    let result = Context::from_request(
        request_with_body("POST", "/x", Some("application/json"), "{not json}"),
    ).await;
    assert!(matches!(result, Err(ContextError::Json(_))));
}

#[tokio::test]
async fn test_query_global_empty() {
    assert_eq!(test_process("return QUERY").await, "{}");
}

#[tokio::test]
async fn test_query_object() {
    let context = Context::from_request(
        request_with_body("GET", "/index.rhp?id=123&name=hello", None, ""),
    ).await.unwrap();
    let env = setup_env(&context);
    assert_eq!(process_script_section(env, "return QUERY.id").await, "123");
}

#[tokio::test]
async fn test_object_falsiness() {
    assert_eq!(test_process(r"
        let a = {}
        if (a) { return 1 }
        return 2
    ").await, "2");
}

#[tokio::test]
async fn test_object_truthiness() {
    assert_eq!(test_process(r"
        let a = { ok: true }
        if (a) { return 1 }
        return 2
    ").await, "1");
}

#[tokio::test]
async fn test_object_error_falsiness() {
    assert_eq!(test_process(r"
        let a = { ok: false, error: 'oh no' }
        if (a) { return 1 }
        return 2
    ").await, "2");
}

#[tokio::test]
async fn test_try_syntax() {
    assert_eq!(test_process(r"
        let a = { b: 1 }
        try a
        return 'task complete'
    ").await, "task complete");

    assert_eq!(test_process(r"
        let a = { b: 1 }
        a.error = 'oh no'
        try a
        return 'task complete'
    ").await, "{ b: 1, error: oh no }"); // TODO Should this have quotes?
}

#[tokio::test]
async fn test_db_ping() {
    let context = Context::from_request(
        request_with_body("GET", "/x", None, ""),
    ).await.unwrap();
    let env = setup_env(&context);
    assert_eq!(process_script_section(env, "return DB.PING()").await, "pong");
}

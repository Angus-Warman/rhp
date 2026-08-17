use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

static DB_ID: AtomicU64 = AtomicU64::new(0);

async fn test_conn() -> DbConn {
    // Use a unique named shared in-memory database per test to keep every
    // pooled connection on the same db.
    let id = DB_ID.fetch_add(1, Ordering::Relaxed);
    crate::db::connect(&format!(
        "sqlite://file%3Arhp_proc_test_{id}?mode=memory&cache=shared"
    ))
    .await
    .unwrap()
}

async fn test_process(script: &str) -> String {
    let env = setup_env(&Context::default(), test_conn().await);
    process_script_section(env, script).await
}

fn ctx(method: Method) -> Context {
    Context {
        method,
        query: HashMap::new(),
        headers: HashMap::new(),
        body: empty_object(),
        socket: None,
    }
}

fn request_with_body(method: &str, uri: &str, content_type: Option<&str>, body: &str) -> Request {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(ct) = content_type {
        builder = builder.header("content-type", ct);
    }
    builder
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

async fn eval_with_context(context: &Context, script: &str) -> String {
    let env = setup_env(context, test_conn().await);
    process_script_section(env, script).await
}

#[tokio::test]
async fn test_basic_eval() {
    assert_eq!(test_process("return 1 + 2").await, "3");
}

#[tokio::test]
async fn test_json_parse() {
    assert_eq!(
        test_process(
            r#"
        let m = JSON.Parse('{"name":"alice","text":"hi"}')
        return m.name + ':' + m.text
    "#
        )
        .await,
        "alice:hi"
    );
    // Invalid input returns { ok: false, error: msg }
    assert_eq!(
        test_process(
            r#"
        let r = JSON.Parse('not json')
        return r.ok + ':' + (r.error != '')
    "#
        )
        .await,
        "false:true"
    );
    assert_eq!(test_process("return JSON.Parse(42).ok").await, "false");
    assert_eq!(test_process("return JSON.Parse().ok").await, "false");
}

#[tokio::test]
async fn test_json_stringify() {
    assert_eq!(test_process(r#"return JSON.Stringify(null)"#).await, "null");
    assert_eq!(test_process(r#"return JSON.Stringify(2.5)"#).await, "2.5");
    assert_eq!(
        test_process(r#"return JSON.Stringify([1, "a"])"#).await,
        r#"[1,"a"]"#
    );
    // Round trip: Stringify then Parse
    assert_eq!(
        test_process(
            r#"
        let m = JSON.Parse(JSON.Stringify({ a: 1, b: "x" }))
        return m.a + ':' + m.b
    "#
        )
        .await,
        "1:x"
    );
    assert_eq!(test_process("return JSON.Stringify().ok").await, "false");
}

#[tokio::test]
async fn test_pass_function() {
    assert_eq!(
        test_process(
            r"
        let inc = (a) => a * 2
        let apply_twice = (f, n) => f(f(n))
        return apply_twice(inc, 3) 
    "
        )
        .await,
        "12"
    );
}

#[tokio::test]
async fn test_global_constants() {
    assert_eq!(
        test_process(
            r"
        return VERSION 
    "
        )
        .await,
        "0.0.1"
    );
}

#[tokio::test]
async fn test_console_log() {
    assert_eq!(
        test_process(
            r"
        console.log('hello world') 
    "
        )
        .await,
        ""
    );
}

#[tokio::test]
async fn test_method_filtered_sections() {
    let src = r#"<rhp method="PUT">return "put"</rhp><rhp method="POST">return "post"</rhp>"#;
    assert_eq!(
        process_src(src.to_string(), ctx(Method::Post), test_conn().await)
            .await
            .0,
        "post"
    );
    assert_eq!(
        process_src(src.to_string(), ctx(Method::Put), test_conn().await)
            .await
            .0,
        "put"
    );
    assert_eq!(
        process_src(src.to_string(), ctx(Method::Get), test_conn().await)
            .await
            .0,
        ""
    );
}

#[tokio::test]
async fn test_unfiltered_section_runs_all_methods() {
    let src = r#"<rhp>return "always"</rhp>"#;
    assert_eq!(
        process_src(src.to_string(), ctx(Method::Get), test_conn().await)
            .await
            .0,
        "always"
    );
    assert_eq!(
        process_src(src.to_string(), ctx(Method::Post), test_conn().await)
            .await
            .0,
        "always"
    );
}

#[test]
fn test_split_src_parses_method_attr() {
    let src = r#"<rhp method="PUT">a</rhp><rhp>b</rhp><rhp method='POST'>c</rhp>"#;
    assert_eq!(
        split_src(src),
        vec![
            Section::Code {
                code: "a".into(),
                method: Method::Put
            },
            Section::Code {
                code: "b".into(),
                method: Method::All
            },
            Section::Code {
                code: "c".into(),
                method: Method::Post
            },
        ]
    );
}

#[tokio::test]
async fn test_object_prop() {
    assert_eq!(
        test_process(
            r"
        let a = {}
        a.b = 1
        return a
    "
        )
        .await,
        "{ b: 1 }"
    );
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
    let context = Context::from_request(request_with_body(
        "POST",
        "/x",
        Some("text/plain"),
        "hello world",
    ))
    .await
    .unwrap();
    assert_eq!(
        eval_with_context(&context, "return BODY.text").await,
        "hello world"
    );
}

#[tokio::test]
async fn test_body_json_object() {
    let context = Context::from_request(request_with_body(
        "POST",
        "/x",
        Some("application/json"),
        r#"{"name":"rhp","count":2}"#,
    ))
    .await
    .unwrap();
    assert_eq!(eval_with_context(&context, "return BODY.name").await, "rhp");
    assert_eq!(eval_with_context(&context, "return BODY.count").await, "2");
}

#[tokio::test]
async fn test_body_json_nested() {
    let context = Context::from_request(request_with_body(
        "POST",
        "/x",
        Some("application/json"),
        r#"{"user":{"age":3}}"#,
    ))
    .await
    .unwrap();
    assert_eq!(
        eval_with_context(&context, "return BODY.user.age").await,
        "3"
    );
}

#[tokio::test]
async fn test_body_json_array() {
    let context = Context::from_request(request_with_body(
        "POST",
        "/x",
        Some("application/json"),
        "[1, 2, 3]",
    ))
    .await
    .unwrap();
    assert_eq!(
        eval_with_context(&context, "return BODY").await,
        "[1, 2, 3]"
    );
}

#[tokio::test]
async fn test_body_json_primitive() {
    let context = Context::from_request(request_with_body(
        "POST",
        "/x",
        Some("application/json"),
        "5",
    ))
    .await
    .unwrap();
    assert_eq!(eval_with_context(&context, "return BODY").await, "5");
}

#[tokio::test]
async fn test_body_form() {
    let context = Context::from_request(request_with_body(
        "POST",
        "/x",
        Some("application/x-www-form-urlencoded"),
        "a=1&b=hello",
    ))
    .await
    .unwrap();
    assert_eq!(eval_with_context(&context, "return BODY.a").await, "1");
    assert_eq!(
        eval_with_context(&context, "return BODY.as").await,
        "[\"1\"]"
    );
    assert_eq!(eval_with_context(&context, "return BODY.b").await, "hello");
    assert_eq!(
        eval_with_context(&context, "return BODY.bs").await,
        "[\"hello\"]"
    );
    assert_eq!(eval_with_context(&context, "return BODY.c").await, "null");
}

#[tokio::test]
async fn test_body_form_duplicate_values() {
    let context = Context::from_request(request_with_body(
        "POST",
        "/x",
        Some("application/x-www-form-urlencoded"),
        "color=red&color=blue",
    ))
    .await
    .unwrap();
    assert_eq!(
        eval_with_context(&context, "return BODY.color").await,
        "red"
    ); // Gets the first
    assert_eq!(
        eval_with_context(&context, "return BODY.colors").await,
        "[\"red\", \"blue\"]"
    );
}

#[tokio::test]
async fn test_body_empty_for_get() {
    let context = Context::from_request(request_with_body(
        "GET",
        "/x",
        Some("text/plain"),
        "ignored",
    ))
    .await
    .unwrap();
    assert_eq!(eval_with_context(&context, "return BODY").await, "{}");
}

#[tokio::test]
async fn test_body_empty_body() {
    let context = Context::from_request(request_with_body("POST", "/x", Some("text/plain"), ""))
        .await
        .unwrap();
    assert_eq!(eval_with_context(&context, "return BODY").await, "{}");
}

#[tokio::test]
async fn test_body_invalid_json_errors() {
    let result = Context::from_request(request_with_body(
        "POST",
        "/x",
        Some("application/json"),
        "{not json}",
    ))
    .await;
    assert!(matches!(result, Err(ContextError::Json(_))));
}

#[tokio::test]
async fn test_query_global_empty() {
    assert_eq!(test_process("return QUERY").await, "{}");
}

#[tokio::test]
async fn test_query_object() {
    let context = Context::from_request(request_with_body(
        "GET",
        "/index.rhp?id=123&name=hello",
        None,
        "",
    ))
    .await
    .unwrap();
    let env = setup_env(&context, test_conn().await);
    assert_eq!(process_script_section(env, "return QUERY.id").await, "123");
}

#[tokio::test]
async fn test_object_falsiness() {
    assert_eq!(
        test_process(
            r"
        let a = {}
        if (a) { return 1 }
        return 2
    "
        )
        .await,
        "2"
    );
}

#[tokio::test]
async fn test_object_truthiness() {
    assert_eq!(
        test_process(
            r"
        let a = { ok: true }
        if (a) { return 1 }
        return 2
    "
        )
        .await,
        "1"
    );
}

#[tokio::test]
async fn test_object_error_falsiness() {
    assert_eq!(
        test_process(
            r"
        let a = { ok: false, error: 'oh no' }
        if (a) { return 1 }
        return 2
    "
        )
        .await,
        "2"
    );
}

#[tokio::test]
async fn test_try_syntax() {
    assert_eq!(
        test_process(
            r"
        let a = { b: 1 }
        try a
        return 'task complete'
    "
        )
        .await,
        "task complete"
    );

    assert_eq!(
        test_process(
            r"
        let a = { b: 1 }
        a.error = 'oh no'
        try a
        return 'task complete'
    "
        )
        .await,
        "{ b: 1, error: \"oh no\" }"
    );
}

#[tokio::test]
async fn test_try_var_decl_returns_value() {
    // Truthy assignment: the variable is defined and execution continues.
    assert_eq!(
        test_process(
            r#"
        try let m = JSON.Parse('{"name":"alice","text":"hi"}')
        return 'ok:' + m.name
    "#
        )
        .await,
        "ok:alice"
    );

    // Falsy value: `let x = 0` returns 0 → try early-returns it.
    assert_eq!(
        test_process(
            r"
        try let x = 0
        return 'unreached'
    "
        )
        .await,
        "0"
    );

    // Failing JSON.Parse returns { ok: false } → try early-returns the error.
    assert_eq!(
        test_process(
            r#"
        try let m = JSON.Parse('not json')
        return 'unreached'
    "#
        )
        .await,
        "{ error: \"JSON.Parse: expected ident at line 1 column 2\", ok: false }"
    );
}

#[tokio::test]
async fn test_const_enforcement() {
    async fn eval(src: &str) -> Result<String, String> {
        let env = setup_env(&Context::default(), test_conn().await);
        let tokens = lexer::lex_code(src).unwrap();
        let (stmts, _) = Parser::parse(tokens, src);
        let mut ev = Evaluator::new();
        match ev.eval_stmts(&stmts, env).await {
            Ok(()) => Ok(ev.output),
            Err(e) => Err(e.message),
        }
    }

    // Assigning to a const errors.
    let err = eval("const x = 1\nx = 2").await.unwrap_err();
    assert!(err.contains("constant"), "got: {err}");

    // Assigning to a const in an enclosing scope errors too.
    assert!(eval("const x = 1\nif (true) { x = 2 }").await.is_err());

    // let can be reassigned.
    assert_eq!(
        eval("let x = 1\nx = 2\nreturn x").await,
        Ok("2".to_string())
    );

    // Mutating the contents of a const value is still allowed.
    assert_eq!(
        eval("const a = [1]\nlet n = a.push(2)\nreturn n + ':' + a.join('-')").await,
        Ok("2:1-2".to_string())
    );

    // A const declared with `let` again in a child scope shadows, not errors.
    assert_eq!(
        eval("const x = 1\nif (true) { let x = 2 }\nreturn x").await,
        Ok("1".to_string())
    );
}

#[tokio::test]
async fn test_typeof_operator() {
    assert_eq!(
        test_process(
            r#"
        let a_string = "hi"
        let an_int = 42
        let a_float = 42.0
        let a_bool = true
        let a_null = null
        let an_array = [1, 2]
        let an_object = { x: 1 }
        let a_function = () => 1
        return [typeof a_string, typeof an_int, typeof a_float, typeof a_bool, typeof a_null, typeof an_array, typeof an_object, typeof a_function].join(":")
    "#
        )
        .await,
        "string:number:number:bool:null:array:object:function"
    );

    // typeof binds its argument; `!typeof x` works.
    assert_eq!(test_process(r#"return !typeof "x""#).await, "false");

    // `.type` property on non-object values.
    assert_eq!(
        test_process(r#"return "hi".type + ':' + (42).type + ':' + [1].type"#).await,
        "string:number:array"
    );

    // Objects keep their own `type` field.
    assert_eq!(
        test_process(r#"return { type: "person" }.type"#).await,
        "person"
    );
}

#[tokio::test]
async fn test_worked_try_bubbles_errors() {
    // A signup pipeline where every step returns `{ ok: true, ... }` on
    // success or `{ ok: false, error: msg }` on failure. Each `try` returns
    // the failing step's error object, bubbling it up through the call stack
    // until the top-level `return` renders it.
    let script = r#"
        function validate_name(name) {
            if (name == '') { return { ok: false, error: 'name is required' } }
            return { ok: true, name: name }
        }

        function validate_age(age) {
            if (age < 0) { return { ok: false, error: 'age cannot be negative' } }
            if (age < 18) { return { ok: false, error: 'must be 18 or older' } }
            return { ok: true, age: age }
        }

        function create_account(name, age) {
            try let n = validate_name(name)
            try let a = validate_age(age)
            return { ok: true, name: n.name, age: a.age }
        }

        function run_signup(name, age) {
            try let account = create_account(name, age)
            return 'welcome ' + account.name + ' (' + account.age + ')'
        }
    "#;

    // Happy path: every try continues, the pipeline completes.
    assert_eq!(
        test_process(&format!("{script}\nreturn run_signup('ada', 30)")).await,
        "welcome ada (30)"
    );

    // Each failure short-circuits at the offending step and bubbles up the
    // exact error object — no intermediate step rewrites or swallows it.
    assert_eq!(
        test_process(&format!("{script}\nreturn run_signup('ada', 17)")).await,
        "{ error: \"must be 18 or older\", ok: false }"
    );
    assert_eq!(
        test_process(&format!("{script}\nreturn run_signup('ada', -3)")).await,
        "{ error: \"age cannot be negative\", ok: false }"
    );
    assert_eq!(
        test_process(&format!("{script}\nreturn run_signup('', 30)")).await,
        "{ error: \"name is required\", ok: false }"
    );

    // The first error wins when multiple steps would fail.
    assert_eq!(
        test_process(&format!("{script}\nreturn run_signup('', -1)")).await,
        "{ error: \"name is required\", ok: false }"
    );
}

#[tokio::test]
async fn test_db_ping() {
    let context = Context::from_request(request_with_body("GET", "/x", None, ""))
        .await
        .unwrap();
    let env = setup_env(&context, test_conn().await);
    assert_eq!(
        process_script_section(env, "return DB.Ping()").await,
        "pong"
    );
}

#[tokio::test]
async fn test_db_query_all() {
    assert_eq!(
        test_process(
            r#"
        return DB.Query("SELECT 2").All()
    "#
        )
        .await,
        r#"[{ 2: null }]"#
    );
}

#[tokio::test]
async fn test_db_query_one() {
    assert_eq!(
        test_process(
            r#"
        return DB.Query("SELECT 2").One()
    "#
        )
        .await,
        r#"{ 2: null }"#
    );
}

#[tokio::test]
async fn test_db_query_returns_stmt_object() {
    assert_eq!(
        test_process(
            r#"
        return DB.Query("SELECT 2")
    "#
        )
        .await,
        "{ All: [function], Bind: [function], One: [function] }"
    );
}

#[tokio::test]
async fn test_db_query_bind_param() {
    assert_eq!(
        test_process(
            r#"
        DB.Exec("CREATE TABLE t (id INTEGER, name TEXT)").Run()
        DB.Exec("INSERT INTO t (id, name) VALUES (1, 'alice'), (2, 'bob')").Run()
        return DB.Query("SELECT name FROM t WHERE id = ?").Bind(2).One().name
    "#
        )
        .await,
        "bob"
    );
}

#[tokio::test]
async fn test_db_query_all_invalid_returns_error_object() {
    assert_eq!(
        test_process(
            r#"
        let res = DB.Query("SELECT FROM WHERE").All()[0]
        if (!res.ok && res.error) { return "error object" }
        return "fail"
    "#
        )
        .await,
        "error object"
    );
}

#[tokio::test]
async fn test_db_exec_run() {
    assert_eq!(
        test_process(
            r#"
        return DB.Exec("CREATE TABLE t (id INTEGER)").Run()
    "#
        )
        .await,
        "{ ok: true, rowsAffected: 0 }"
    );
}

#[tokio::test]
async fn test_db_exec_returns_stmt_object() {
    assert_eq!(
        test_process(
            r#"
        return DB.Exec("CREATE TABLE t (id INTEGER)")
    "#
        )
        .await,
        "{ Run: [function] }"
    );
}

#[tokio::test]
async fn test_db_exec_run_invalid_returns_error_object() {
    assert_eq!(
        test_process(
            r#"
        let res = DB.Exec("INSEERT INTO nope").Run()
        if (!res.ok && res.error) { return "error object" }
        return "fail"
    "#
        )
        .await,
        "error object"
    );
}

#[tokio::test]
async fn test_db_table_returns_stmt_object() {
    assert_eq!(
        test_process(
            r#"
        DB.Exec("CREATE TABLE users (id INTEGER, name TEXT)").Run()
        let t = DB.Table("users")
        if (t.All && t.One && t.Count && t.Columns && t.Insert && t.Update) { return "has methods" }
        return "fail"
    "#
        )
        .await,
        "has methods"
    );
}

#[tokio::test]
async fn test_db_table_all_count() {
    assert_eq!(
        test_process(
            r#"
        DB.Exec("CREATE TABLE users (id INTEGER, name TEXT)").Run()
        DB.Exec("INSERT INTO users (id, name) VALUES (1, 'alice'), (2, 'bob')").Run()
        let t = DB.Table("users")
        return t.Count() + ":" + t.All()
    "#
        )
        .await,
        r#"2:[{ id: 1, name: "alice" }, { id: 2, name: "bob" }]"#
    );
}

#[tokio::test]
async fn test_db_table_one() {
    assert_eq!(
        test_process(
            r#"
        DB.Exec("CREATE TABLE users (id INTEGER, name TEXT)").Run()
        DB.Exec("INSERT INTO users (id, name) VALUES (1, 'alice')").Run()
        return DB.Table("users").One()
    "#
        )
        .await,
        r#"{ id: 1, name: "alice" }"#
    );
}

#[tokio::test]
async fn test_db_table_columns() {
    assert_eq!(
        test_process(
            r#"
        DB.Exec("CREATE TABLE users (id INTEGER, name TEXT)").Run()
        return DB.Table("users").Columns()
    "#
        )
        .await,
        r#"[{ name: "id", type: "INTEGER" }, { name: "name", type: "TEXT" }]"#
    );
}

#[tokio::test]
async fn test_db_table_insert_update() {
    assert_eq!(
        test_process(
            r#"
        DB.Exec("CREATE TABLE users (id INTEGER, name TEXT)").Run()
        let t = DB.Table("users")
        t.Insert({ id: 1, name: "alice" }).Run()
        t.Insert({ id: 2, name: "bob" }).Run()
        let updated = t.Update({ name: "renamed" }).Run()
        let rows = t.All()
        return updated.rowsAffected + ":" + rows
    "#
        )
        .await,
        r#"2:[{ id: 1, name: "renamed" }, { id: 2, name: "renamed" }]"#
    );
}

#[tokio::test]
async fn test_db_table_insert_bad_args() {
    assert_eq!(
        test_process(
            r#"
        DB.Exec("CREATE TABLE users (id INTEGER)").Run()
        let res = DB.Table("users").Insert("nope")
        if (!res.ok && res.error == "TableStmt.Insert: expected an object") { return "error object" }
        return "fail"
    "#
        )
        .await,
        "error object"
    );
}

#[tokio::test]
async fn test_db_bad_args_return_error_objects() {
    assert_eq!(
        test_process(
            r#"
        let a = DB.Query(42)
        let b = DB.Exec()
        let c = DB.Table(42)
        let d = DB.Table("users").Update("nope")
        let e = DB.Table("users").Where(7)
        return a.ok + ':' + b.ok + ':' + c.ok + ':' + d.ok + ':' + e.ok
    "#
        )
        .await,
        "false:false:false:false:false"
    );
}

#[tokio::test]
async fn test_db_table_where_delete_from_script() {
    assert_eq!(
        test_process(
            r#"
        DB.Exec("CREATE TABLE users (id INTEGER, name TEXT)").Run()
        DB.Table("users").Insert({ id: 1, name: "alice" }).Run()
        DB.Table("users").Insert({ id: 2, name: "bob" }).Run()
        let t = DB.Table("users").Where({ id: 2 })
        let deleted = t.Delete().Run()
        return deleted.rowsAffected + ":" + t.Count() + ":" + DB.Table("users").Count()
    "#
        )
        .await,
        "1:0:1"
    );
}

#[tokio::test]
async fn test_for_in_array() {
    assert_eq!(
        test_process(
            r"
        let sum = 0
        for (x in [1, 2, 3]) { sum += x }
        return sum
    "
        )
        .await,
        "6"
    );
}

#[tokio::test]
async fn test_for_in_number_counts_up_from_zero() {
    assert_eq!(
        test_process(
            r"
        let sum = 0
        for (i in 4) { sum += i }
        return sum
    "
        )
        .await,
        "6"
    );
}

#[tokio::test]
async fn test_for_in_string_yields_letters() {
    assert_eq!(
        test_process(
            r"
        let out = ''
        for (c in 'abc') { out += c }
        return out
    "
        )
        .await,
        "abc"
    );
}

#[tokio::test]
async fn test_for_in_object_yields_keys() {
    assert_eq!(
        test_process(
            r"
        let o = { a: 1, b: 2, c: 3 }
        let count = 0
        for (k in o) { count += 1 }
        return count
    "
        )
        .await,
        "3"
    );
}

#[tokio::test]
async fn test_for_in_object_membership() {
    assert_eq!(
        test_process(
            r"
        let o = { a: 1, b: 2, c: 3 }
        let found = false
        for (k in o) { if (k === 'b') { found = true } }
        return found
    "
        )
        .await,
        "true"
    );
}

#[tokio::test]
async fn test_for_in_break() {
    assert_eq!(
        test_process(
            r"
        let sum = 0
        for (x in [1, 2, 3]) { if (x === 2) { break } sum += x }
        return sum
    "
        )
        .await,
        "1"
    );
}

#[tokio::test]
async fn test_for_in_continue() {
    assert_eq!(
        test_process(
            r"
        let sum = 0
        for (x in [1, 2, 3, 4]) { if (x === 2) { continue } sum += x }
        return sum
    "
        )
        .await,
        "8"
    );
}

#[tokio::test]
async fn test_for_in_nested() {
    assert_eq!(
        test_process(
            r"
        let sum = 0
        for (x in [1, 2]) { for (y in [10, 20]) { sum += x * y } }
        return sum
    "
        )
        .await,
        "90"
    );
}

#[tokio::test]
async fn test_for_in_variable_stays_scoped_to_loop() {
    assert_eq!(
        test_process(
            r"
        let x = 'outer'
        for (x in [1, 2, 3]) { }
        return x
    "
        )
        .await,
        "outer"
    );
}

#[tokio::test]
async fn test_for_in_loop_variable_not_visible_after_loop() {
    assert_eq!(
        test_process(
            r"
        let seen = 0
        for (x in [7, 8, 9]) { seen = x }
        return seen
    "
        )
        .await,
        "9"
    );
}

#[tokio::test]
async fn test_for_in_zero_iterations() {
    assert_eq!(
        test_process(
            r"
        let count = 0
        for (x in []) { count += 1 }
        for (i in 0) { count += 1 }
        for (c in '') { count += 1 }
        for (k in {}) { count += 1 }
        return count
    "
        )
        .await,
        "0"
    );
}

#[tokio::test]
async fn test_for_in_with_let_keyword() {
    assert_eq!(
        test_process(
            r"
        let sum = 0
        for (let x in [1, 2, 3]) { sum += x }
        return sum
    "
        )
        .await,
        "6"
    );
}

#[tokio::test]
async fn test_for_in_inside_function() {
    assert_eq!(
        test_process(
            r"
        let sum = (arr) => {
            let total = 0
            for (x in arr) { total += x }
            return total
        }
        return sum([1, 2, 3])
    "
        )
        .await,
        "6"
    );
}

#[tokio::test]
async fn test_c_style_for_still_parses_after_for_in() {
    assert_eq!(
        test_process(
            r"
        let sum = 0
        for (let i = 0; i < 4; i++) { sum += i }
        return sum
    "
        )
        .await,
        "6"
    );
}

#[tokio::test]
async fn test_c_style_for_basic_count() {
    assert_eq!(
        test_process(
            r"
        let sum = 0
        for (let i = 0; i < 5; i++) { sum += i }
        return sum
    "
        )
        .await,
        "10"
    );
}

#[tokio::test]
async fn test_c_style_for_existing_var_no_let() {
    assert_eq!(
        test_process(
            r"
        let sum = 0
        for (i = 0; i < 3; i++) { sum += i }
        return sum
    "
        )
        .await,
        "3"
    );
}

#[tokio::test]
async fn test_c_style_for_counting_down() {
    assert_eq!(
        test_process(
            r"
        let out = ''
        for (let i = 3; i >= 0; i--) { out += i }
        return out
    "
        )
        .await,
        "3210"
    );
}

#[tokio::test]
async fn test_c_style_for_step_by_two() {
    assert_eq!(
        test_process(
            r"
        let out = ''
        for (let i = 0; i < 10; i += 2) { out += i }
        return out
    "
        )
        .await,
        "02468"
    );
}

#[tokio::test]
async fn test_c_style_for_empty_init() {
    assert_eq!(
        test_process(
            r"
        let i = 0
        let sum = 0
        for (; i < 3; i++) { sum += i }
        return sum
    "
        )
        .await,
        "3"
    );
}

#[tokio::test]
async fn test_c_style_for_empty_cond_with_break() {
    assert_eq!(
        test_process(
            r"
        let sum = 0
        for (let i = 0; ; i++) { if (i >= 4) { break } sum += i }
        return sum
    "
        )
        .await,
        "6"
    );
}

#[tokio::test]
async fn test_c_style_for_empty_update_with_increment_in_body() {
    assert_eq!(
        test_process(
            r"
        let sum = 0
        for (let i = 0; i < 4;) { sum += i; i++ }
        return sum
    "
        )
        .await,
        "6"
    );
}

#[tokio::test]
async fn test_c_style_for_continue_skips_update() {
    assert_eq!(
        test_process(
            r"
        let sum = 0
        for (let i = 0; i < 5; i++) { if (i === 2) { continue } sum += i }
        return sum
    "
        )
        .await,
        "8"
    );
}

#[tokio::test]
async fn test_c_style_for_break_stops_early() {
    assert_eq!(
        test_process(
            r"
        let sum = 0
        for (let i = 0; i < 100; i++) { if (i === 4) { break } sum += i }
        return sum
    "
        )
        .await,
        "6"
    );
}

#[tokio::test]
async fn test_c_style_for_nested() {
    assert_eq!(
        test_process(
            r"
        let out = ''
        for (let i = 0; i < 3; i++) {
            for (let j = 0; j < 2; j++) { out += i * 10 + j }
        }
        return out
    "
        )
        .await,
        "0110112021"
    );
}

#[tokio::test]
async fn test_c_style_for_zero_iterations() {
    assert_eq!(
        test_process(
            r"
        let count = 0
        for (let i = 0; i > 5; i++) { count += 1 }
        return count
    "
        )
        .await,
        "0"
    );
}

#[tokio::test]
async fn test_c_style_for_does_not_leak_loop_var() {
    assert_eq!(
        test_process(
            r"
        let i = 'outer'
        for (let i = 0; i < 3; i++) { }
        return i
    "
        )
        .await,
        "outer"
    );
}

#[tokio::test]
async fn test_c_style_for_and_for_in_coexist() {
    assert_eq!(
        test_process(
            r"
        let sum = 0
        for (let i = 0; i < 3; i++) {
            for (x in [10, 20]) { sum += i + x }
        }
        for (y in [100, 200]) { for (let j = 0; j < 2; j++) { sum += y + j } }
        return sum
    "
        )
        .await,
        "698"
    );
}

#[tokio::test]
async fn test_fizzbuzz_while() {
    assert_eq!(
        test_process(
            r"
        let out = ''
        let i = 1
        while (i <= 15) {
            if (i % 15 === 0) { out += 'FizzBuzz' }
            else if (i % 3 === 0) { out += 'Fizz' }
            else if (i % 5 === 0) { out += 'Buzz' }
            else { out += i }
            out += ','
            i++
        }
        return out
    "
        )
        .await,
        "1,2,Fizz,4,Buzz,Fizz,7,8,Fizz,Buzz,11,Fizz,13,14,FizzBuzz,"
    );
}

#[tokio::test]
async fn test_price_calculator() {
    assert_eq!(
        test_process(
            r"
        let price = 120
        let discount = 20
        let total = price - discount
        total *= 2
        total /= 4
        total -= 5
        total += total % 10
        return total + ' for ' + total / 2 + ' each'
    "
        )
        .await,
        "50 for 25 each"
    );
}

#[tokio::test]
async fn test_inventory_object_index() {
    assert_eq!(
        test_process(
            r"
        const stock = { apples: 3, oranges: 2 }
        var total = 0
        for (k in stock) { total += stock[k] }
        if (total != 5) { return 'wrong count' }
        if (total !== 5) { return 'strict mismatch' }
        stock['plums'] = 4
        return stock['plums'] + stock['apples']
    "
        )
        .await,
        "7"
    );
}

#[tokio::test]
async fn test_prefix_increment_and_negation() {
    assert_eq!(
        test_process(
            r"
        let i = 3
        let a = ++i
        let b = a--
        let c = --a
        let neg = -c
        let back = -neg
        return a + ':' + b + ':' + c + ':' + neg + ':' + back
    "
        )
        .await,
        "2:4:2:-2:2"
    );
}

#[tokio::test]
async fn test_pagination_defaults() {
    assert_eq!(
        test_process(
            r"
        let page = 0
        let size = 0
        size = size || 20
        let offset = page * size
        let hasMore = page <= 2
        if (hasMore == true) { page++ }
        if (page != 1) { return 'no' }
        if (page === 1 && size !== 10) { return 'ok:' + size }
        return 'fail'
    "
        )
        .await,
        "ok:20"
    );
}

#[tokio::test]
async fn test_comments_and_string_escapes() {
    assert_eq!(
        test_process(
            r#"
        // single-line comment
        let a = 1 /* inline block */ + 2 // trailing
        /* multi-line
           comment */
        let path = 'a\\b'
        let quote = "he said \"hi\""
        let joined = path + '|' + quote
        return a + ':' + joined
    "#
        )
        .await,
        r#"3:a\b|he said "hi""#
    );
}

#[tokio::test]
async fn test_response_object() {
    // Status + JSON body + content type.
    let (_html, response) = process_src(
        r#"<rhp>
        RES.SetStatus(404)
        RES.Json({ error: "nope" })
        return "ignored"
    </rhp>"#
            .to_string(),
        ctx(Method::Get),
        test_conn().await,
    )
    .await;
    assert_eq!(response.status, Some(404));
    assert_eq!(response.body.as_deref(), Some(r#"{"error":"nope"}"#));
    assert_eq!(response.content_type.as_deref(), Some("application/json"));
    assert_eq!(response.redirect, None);

    // Redirect sets a default 302 and a Location header; later sections stop.
    let (_, response) = process_src(
        r#"<rhp method="GET">
        RES.Redirect("/login")
    </rhp>
    <rhp method="GET">return "unreached"</rhp>"#
            .to_string(),
        ctx(Method::Get),
        test_conn().await,
    )
    .await;
    assert_eq!(response.redirect.as_deref(), Some("/login"));
    assert_eq!(response.status, Some(302));

    // Custom status + redirect.
    let (_, response) = process_src(
        r#"<rhp>
        RES.SetStatus(301)
        RES.Redirect("/moved")
    </rhp>"#
            .to_string(),
        ctx(Method::Get),
        test_conn().await,
    )
    .await;
    assert_eq!(response.status, Some(301));

    // SetCookie produces a set-cookie header.
    let (_, response) = process_src(
        r#"<rhp>
        RES.SetCookie("session", "abc123", { Path: "/", HttpOnly: true, SameSite: "Lax" })
    </rhp>"#
            .to_string(),
        ctx(Method::Get),
        test_conn().await,
    )
    .await;
    assert_eq!(
        response.headers,
        vec![(
            "set-cookie".to_string(),
            "session=abc123; Path=/; HttpOnly; SameSite=Lax".to_string()
        )]
    );

    // SetHeader + bad args.
    let (_, response) = process_src(
        r#"<rhp>
        RES.SetHeader("X-Powered-By", "rhp")
        let bad = RES.SetStatus("nope")
    </rhp>"#
            .to_string(),
        ctx(Method::Get),
        test_conn().await,
    )
    .await;
    assert_eq!(
        response.headers,
        vec![("X-Powered-By".to_string(), "rhp".to_string())]
    );
}

#[tokio::test]
async fn test_cookie_and_header_globals() {
    let context = Context {
        headers: {
            let mut map = HashMap::new();
            map.insert("cookie".to_string(), "session=abc; theme=dark".to_string());
            map.insert("user-agent".to_string(), "test-agent".to_string());
            map
        },
        ..ctx(Method::Get)
    };
    let env = setup_env(&context, test_conn().await);
    assert_eq!(
        process_script_section(env, r#"return COOKIE.session + ':' + COOKIE.theme"#).await,
        "abc:dark"
    );

    let env = setup_env(&context, test_conn().await);
    assert_eq!(
        process_script_section(env, r#"return HEADER["user-agent"]"#).await,
        "test-agent"
    );
}

#[tokio::test]
async fn test_html_block() {
    // Raw HTML outside the <rhp> block passes through untouched; code inside
    // the block runs and renders into the output.
    let src = r#"<header>Site</header><rhp>let n = 2
return <p>{n}</p></rhp><footer>bye</footer>"#;
    assert_eq!(
        process_src(src.to_string(), ctx(Method::Get), test_conn().await)
            .await
            .0,
        "<header>Site</header><p>2</p><footer>bye</footer>"
    );
}

#[tokio::test]
async fn test_html_template_basic() {
    assert_eq!(
        test_process(
            r#"
        let x = "foo"
        return <><p>{x}</p></>
    "#
        )
        .await,
        "<p>foo</p>"
    );
}

#[tokio::test]
async fn test_html_template_escapes_malicious_input() {
    assert_eq!(
        test_process(
            r#"
        let user_comment = "<script>console.log('oh no')</script>"
        return <><p>{user_comment}</p></>
    "#
        )
        .await,
        "<p>&lt;script&gt;console.log(&#x27;oh no&#x27;)&lt;&#x2F;script&gt;</p>"
    );
}

#[tokio::test]
async fn test_html_template_full_escape_map() {
    assert_eq!(
        test_process(
            r#"
        let evil = "a\"b'c`d/e=f<g>h&i"
        return <><p>{evil}</p></>
    "#
        )
        .await,
        "<p>a&quot;b&#x27;c&grave;d&#x2F;e&#x3D;f&lt;g&gt;h&amp;i</p>"
    );
}

#[tokio::test]
async fn test_html_template_text_and_nested_elements() {
    assert_eq!(
        test_process(
            r#"
        let first = "Ada"
        let last = "Lovelace"
        return <><h1>Hi {first}</h1><p>{first} {last}</p></>
    "#
        )
        .await,
        "<h1>Hi Ada</h1><p>Ada Lovelace</p>"
    );
}

#[tokio::test]
async fn test_html_template_element_root_and_number_slot() {
    assert_eq!(
        test_process(
            r#"
        let count = 3
        return <p>items: {count}</p>
    "#
        )
        .await,
        "<p>items: 3</p>"
    );
}

#[tokio::test]
async fn test_html_template_expression_in_slot() {
    assert_eq!(
        test_process(
            r#"
        let a = 2
        let b = 3
        return <><p>{a} + {b} = {a + b}</p></>
    "#
        )
        .await,
        "<p>2 + 3 = 5</p>"
    );
}

#[tokio::test]
async fn test_html_template_ternary() {
    assert_eq!(
        test_process(
            r#"
        let primary = true
        return <><button class={primary ? "primary" : ""}>Post</button></>
    "#
        )
        .await,
        r#"<button class="primary">Post</button>"#
    );
}

#[tokio::test]
async fn test_time_intrinsics() {
    assert_eq!(
        test_process(
            r#"
        let s = TIME.Unix_Sec()
        let ms = TIME.Unix_Ms()
        let ns = TIME.Unix_Ns()
        return (ns > ms * 1000) + ':' + (ms >= s * 1000) + ':' + (s > 1700000000)
    "#
        )
        .await,
        "true:true:true"
    );
}

#[tokio::test]
async fn test_math_intrinsics() {
    assert_eq!(
        test_process(r#"return MATH.Random() >= 0 && MATH.Random() < 1"#).await,
        "true"
    );
    assert_eq!(
        test_process(r#"return MATH.Ceil(2.1) + ':' + MATH.Floor(2.9)"#).await,
        "3:2"
    );
    assert_eq!(
        test_process(r#"return MATH.Sum(1, 2, 3) + ':' + MATH.Sum([1, 2, 3]) + ':' + MATH.Sum()"#)
            .await,
        "6:6:0"
    );
    assert_eq!(
        test_process(
            r#"return MATH.Avg(1, 2, 3) + ':' + MATH.Min(3, 1, 2) + ':' + MATH.Max(3, 1, 2)"#
        )
        .await,
        "2:1:3"
    );
    // Type errors surface as { ok: false, error }
    assert_eq!(test_process(r#"return MATH.Ceil("a").ok"#).await, "false");
    assert_eq!(test_process(r#"return MATH.Sum("a").ok"#).await, "false");
    assert_eq!(test_process(r#"return MATH.Avg().ok"#).await, "false");
    assert_eq!(test_process(r#"return MATH.Min().ok"#).await, "false");
}

#[tokio::test]
async fn test_string_methods() {
    assert_eq!(
        test_process(r#"return "foo bar baz".split(" ")"#).await,
        r#"["foo", "bar", "baz"]"#
    );
    assert_eq!(
        test_process(r#"return "a b c".split()"#).await,
        r#"["a", "b", "c"]"#
    );
    assert_eq!(
        test_process(r#"return "hello".split("")"#).await,
        r#"["h", "e", "l", "l", "o"]"#
    );
    assert_eq!(test_process(r#"return "  hi  ".trim()"#).await, "hi");
    assert_eq!(
        test_process(r#"return "aBc".toUpper() + ':' + "AbC".toLower()"#).await,
        "ABC:abc"
    );
    assert_eq!(
        test_process(r#"return "a1b2".replace("1", "X") + ':' + "hello".contains("ell")"#).await,
        "aXb2:true"
    );
    assert_eq!(test_process(r#"return "x".split(42).ok"#).await, "false");
    assert_eq!(test_process(r#"return "x".contains().ok"#).await, "false");
}

#[tokio::test]
async fn test_array_methods() {
    assert_eq!(
        test_process(
            r#"
        let a = [1, 2]
        let n = a.push(3)
        return n + ':' + a.join('-')
    "#
        )
        .await,
        "3:1-2-3"
    );
    assert_eq!(
        test_process(r#"return [1, 2, 3].length + ':' + [1, 2, 3].join()"#).await,
        "3:1,2,3"
    );
}

#[tokio::test]
async fn test_array_map_filter_reduce() {
    assert_eq!(
        test_process(
            r#"
        let doubled = [1, 2, 3].map(n => n * 2)
        return doubled.join(',')
    "#
        )
        .await,
        "2,4,6"
    );
    assert_eq!(
        test_process(
            r#"
        let evens = [1, 2, 3, 4].filter(n => n % 2 == 0)
        return evens.join(',')
    "#
        )
        .await,
        "2,4"
    );
    assert_eq!(
        test_process(
            r#"
        let sum = [1, 2, 3, 4].reduce((acc, n) => acc + n, 0)
        return sum
    "#
        )
        .await,
        "10"
    );
    // reduce without an initial value seeds from the first element.
    assert_eq!(
        test_process(r#"return [1, 2, 3].reduce((acc, n) => acc + n)"#).await,
        "6"
    );
    // Callbacks receive the index.
    assert_eq!(
        test_process(
            r#"
        let labeled = ["a", "b"].map((item, i) => item + i)
        return labeled.join(',')
    "#
        )
        .await,
        "a0,b1"
    );
}

#[tokio::test]
async fn test_array_sort_slice_indexof() {
    assert_eq!(
        test_process(r#"return [3, 1, 2].sort().join(',')"#).await,
        "1,2,3"
    );
    assert_eq!(
        test_process(
            r#"
        let by_len = ["bb", "a", "ccc"].sort((a, b) => a.length - b.length)
        return by_len.join(',')
    "#
        )
        .await,
        "a,bb,ccc"
    );
    // sort mutates the original array.
    assert_eq!(
        test_process(
            r#"
        let a = [3, 1, 2]
        let b = a.sort()
        return a.join(',') + ':' + b.join(',')
    "#
        )
        .await,
        "1,2,3:1,2,3"
    );
    assert_eq!(
        test_process(r#"return [1, 2, 3, 4].slice(1, 3).join(',')"#).await,
        "2,3"
    );
    assert_eq!(
        test_process(r#"return [1, 2, 3, 4].slice(-2).join(',')"#).await,
        "3,4"
    );
    assert_eq!(
        test_process(r#"return [10, 20, 30].indexOf(20) + ':' + [10, 20, 30].indexOf(99)"#).await,
        "1:-1"
    );
    assert_eq!(
        test_process(r#"return [1, 2].includes(2) + ':' + [1, 2].includes(3)"#).await,
        "true:false"
    );
    assert_eq!(
        test_process(r#"return [1, 2].map("nope").ok"#).await,
        "false"
    );
}

#[tokio::test]
async fn test_compound_assign_mod() {
    assert_eq!(
        test_process(
            r"
        let i = 17
        i %= 5
        return i
    "
        )
        .await,
        "2"
    );
}

#[tokio::test]
async fn test_compound_assign_bitwise_ops() {
    assert_eq!(
        test_process(
            r"
        let a = 12
        a &= 10
        return a
    "
        )
        .await,
        "8"
    );
    assert_eq!(
        test_process(
            r"
        let b = 12
        b |= 3
        return b
    "
        )
        .await,
        "15"
    );
    assert_eq!(
        test_process(
            r"
        let c = 10
        c ^= 12
        return c
    "
        )
        .await,
        "6"
    );
}

#[tokio::test]
async fn test_compound_assign_shift() {
    assert_eq!(
        test_process(
            r"
        let x = 4
        x <<= 3
        return x
    "
        )
        .await,
        "32"
    );
    assert_eq!(
        test_process(
            r"
        let y = 32
        y >>= 2
        return y
    "
        )
        .await,
        "8"
    );
}

#[tokio::test]
async fn test_bitwise_binary_ops() {
    assert_eq!(test_process(r"return 12 & 10").await, "8");
    assert_eq!(test_process(r"return 12 | 5").await, "13");
    assert_eq!(test_process(r"return 12 ^ 10").await, "6");
    assert_eq!(test_process(r"return 1 << 5").await, "32");
    assert_eq!(test_process(r"return 32 >> 3").await, "4");
}

#[tokio::test]
async fn test_compound_assign_string_concat() {
    assert_eq!(
        test_process(
            r#"
        let x = "foo"
        x += "bar"
        return x
    "#
        )
        .await,
        "foobar"
    );
}

#[tokio::test]
async fn test_compound_assign_chained() {
    assert_eq!(
        test_process(
            r"
        let i = 1
        i += 2
        i *= 3
        i -= 1
        i %= 4
        return i
    "
        )
        .await,
        "0"
    );
}

#[tokio::test]
async fn test_array_for_each() {
    assert_eq!(
        test_process(
            r#"
        let total = 0
        let r = [1, 2, 3].forEach(n => total = total + n)
        return total
    "#
        )
        .await,
        "6"
    );
}

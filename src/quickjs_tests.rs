use crate::db::DbConn;
use crate::process::Context;
use crate::quickjs::Engine;

async fn test_conn() -> DbConn {
    crate::db::connect("sqlite://file%3Arhp_qjs_test?mode=memory&cache=shared")
        .await
        .expect("in memory db")
}

fn test_context() -> Context {
    Context::default()
}

#[tokio::test]
async fn test_engine_creates() {
    let conn = test_conn().await;
    let _engine = Engine::new(conn).await.expect("engine");
}

#[tokio::test]
async fn test_run_section_basic() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (text, _val) = engine.run_section("write('hello')").await.unwrap();
    assert_eq!(text, "hello");
}

#[tokio::test]
async fn test_run_section_write_raw() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (text, _) = engine.run_section("writeRaw('<b>bold</b>')").await.unwrap();
    assert_eq!(text, "<b>bold</b>");
}

#[tokio::test]
async fn test_run_section_write_escapes_html() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (text, _) = engine.run_section("write('<script>')").await.unwrap();
    assert_eq!(text, "&lt;script&gt;");
}

#[tokio::test]
async fn test_run_section_multiple_writes() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (text, _) = engine
        .run_section("write('a'); write('b'); writeRaw('c')")
        .await
        .unwrap();
    assert_eq!(text, "abc");
}

#[tokio::test]
async fn test_run_section_completion_value() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine.run_section("return 42").await.unwrap();
    assert_eq!(val, serde_json::json!(42));
}

#[tokio::test]
async fn test_run_section_console_log() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    engine
        .run_section("console.log('test', 123)")
        .await
        .unwrap();
}

#[tokio::test]
async fn test_run_section_query_global() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    let mut ctx = test_context();
    ctx.query.insert("id".into(), "42".into());
    engine.setup(&ctx).await.unwrap();
    let (text, _) = engine.run_section("write(QUERY.id)").await.unwrap();
    assert_eq!(text, "42");
}

#[tokio::test]
async fn test_run_section_body_global() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    let mut ctx = test_context();
    ctx.body = serde_json::json!({"name": "test"});
    engine.setup(&ctx).await.unwrap();
    let (text, _) = engine
        .run_section("writeRaw(JSON.stringify(BODY))")
        .await
        .unwrap();
    assert_eq!(text, r#"{"name":"test"}"#);
}

#[tokio::test]
async fn test_run_section_req_headers() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    let mut ctx = test_context();
    ctx.headers.insert("x-test".into(), "hello".into());
    engine.setup(&ctx).await.unwrap();
    let (text, _) = engine
        .run_section("write(REQ.Headers['x-test'])")
        .await
        .unwrap();
    assert_eq!(text, "hello");
}

#[tokio::test]
async fn test_run_section_version() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (text, _) = engine.run_section("write(VERSION)").await.unwrap();
    assert_eq!(text, "0.0.2");
}

#[tokio::test]
async fn test_read_response_status() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    engine.run_section("RES.Status = 404").await.unwrap();
    let state = engine.read_response().await;
    assert_eq!(state.status, Some(404));
}

#[tokio::test]
async fn test_read_response_json() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    engine.run_section("RES.Json({ ok: true })").await.unwrap();
    let state = engine.read_response().await;
    assert_eq!(state.body.as_deref(), Some(r#"{"ok":true}"#));
    assert_eq!(state.content_type.as_deref(), Some("application/json"));
}

#[tokio::test]
async fn test_read_response_html() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    engine.run_section("RES.Html('<h1>Hi</h1>')").await.unwrap();
    let state = engine.read_response().await;
    assert_eq!(state.body.as_deref(), Some("<h1>Hi</h1>"));
    assert_eq!(state.content_type.as_deref(), Some("text/html"));
}

#[tokio::test]
async fn test_read_response_redirect() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    engine.run_section("RES.Redirect('/other')").await.unwrap();
    let state = engine.read_response().await;
    assert_eq!(state.redirect.as_deref(), Some("/other"));
    assert_eq!(state.status, Some(302));
    assert!(state.owns_response);
}

#[tokio::test]
async fn test_read_response_set_cookie() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    engine
        .run_section("RES.SetCookie('sid', 'abc', { Path: '/', HttpOnly: true })")
        .await
        .unwrap();
    let state = engine.read_response().await;
    assert_eq!(state.cookies, vec!["sid=abc; Path=/; HttpOnly"]);
}

#[tokio::test]
async fn test_read_response_status_does_not_own_response() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    engine.run_section("RES.Status = 410").await.unwrap();
    let state = engine.read_response().await;
    assert_eq!(state.status, Some(410));
    assert!(!state.owns_response);
}

#[tokio::test]
async fn test_run_section_script_error() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let err = engine.run_section("throw new Error('boom')").await;
    assert!(err.is_err());
}

#[tokio::test]
async fn test_run_section_undefined_var_is_not_error() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let result = engine.run_section("undefined").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_run_section_arithmetic() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine.run_section("return 2 + 3 * 4").await.unwrap();
    assert_eq!(val, serde_json::json!(14));
}

#[tokio::test]
async fn test_run_section_string_concat() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (text, _) = engine
        .run_section("write('hello ' + 'world')")
        .await
        .unwrap();
    assert_eq!(text, "hello world");
}

#[tokio::test]
async fn test_run_section_let_and_const() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section("const x = 10; let y = 20; return x + y")
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!(30));
}

#[tokio::test]
async fn test_run_section_if_else() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (text, _) = engine
        .run_section("if (true) { write('yes') } else { write('no') }")
        .await
        .unwrap();
    assert_eq!(text, "yes");
}

#[tokio::test]
async fn test_run_section_for_loop() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (text, _) = engine
        .run_section("for (let i = 0; i < 3; i++) { write(String(i)) }")
        .await
        .unwrap();
    assert_eq!(text, "012");
}

#[tokio::test]
async fn test_run_section_for_in() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section(
            "const obj = {a: 1, b: 2}; let sum = 0; for (const k in obj) { sum += obj[k] }; return sum",
        )
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!(3));
}

#[tokio::test]
async fn test_run_section_arrow_function() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section("const add = (a, b) => a + b; return add(3, 4)")
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!(7));
}

#[tokio::test]
async fn test_run_section_try_catch() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (text, _) = engine
        .run_section("try { throw new Error('fail') } catch(e) { write('caught') }")
        .await
        .unwrap();
    assert_eq!(text, "caught");
}

#[tokio::test]
async fn test_run_section_json_parse_stringify() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section("const o = JSON.parse('{\"x\":1}'); return o.x + 1")
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!(2));
}

#[tokio::test]
async fn test_run_section_json_stringify() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section("return JSON.stringify({ a: 1 })")
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!(r#"{"a":1}"#));
}

#[tokio::test]
async fn test_run_section_date_now() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section("return typeof Date.now()")
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!("number"));
}

#[tokio::test]
async fn test_run_section_math() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section("return Math.max(1, 2, 3)")
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!(3));
}

#[tokio::test]
async fn test_run_section_array_methods() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section("return [1,2,3].map(x => x * 2).filter(x => x > 3)")
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!([4, 6]));
}

#[tokio::test]
async fn test_run_section_object_spread() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (text, _) = engine
        .run_section("writeRaw(JSON.stringify({ ...{a:1}, b:2 }))")
        .await
        .unwrap();
    assert_eq!(text, r#"{"a":1,"b":2}"#);
}

#[tokio::test]
async fn test_run_section_string_methods() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section("return 'Hello World'.toLowerCase().split(' ')")
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!(["hello", "world"]));
}

#[tokio::test]
async fn test_run_section_number_methods() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section("return Number.isInteger(42) && !Number.isInteger(3.14)")
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!(true));
}

#[tokio::test]
async fn test_run_section_await_promise() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section("return await Promise.resolve(42)")
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!(42));
}

#[tokio::test]
async fn test_run_section_async_await() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section("async function double(n) { return n * 2 }; return await double(21)")
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!(42));
}

#[tokio::test]
async fn test_db_exec_and_table() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();

    engine
        .run_section(
            "const e = DB.Exec('CREATE TABLE IF NOT EXISTS t (id INTEGER PRIMARY KEY, val TEXT)'); await e.Run()",
        )
        .await
        .unwrap();

    engine
        .run_section("const t = DB.Table('t'); await t.Insert({ val: 'hello' }).Run()")
        .await
        .unwrap();

    let (_text, val) = engine
        .run_section("const t = DB.Table('t'); return JSON.stringify(await t.All())")
        .await
        .unwrap();
    assert!(val.to_string().contains("hello"));
}

#[tokio::test]
async fn test_db_query_bind() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();

    engine
        .run_section(
            "const e = DB.Exec('CREATE TABLE IF NOT EXISTS t2 (id INTEGER PRIMARY KEY, val TEXT)'); await e.Run()",
        )
        .await
        .unwrap();
    engine
        .run_section("const t = DB.Table('t2'); await t.Insert({ val: 'world' }).Run()")
        .await
        .unwrap();

    let (_text, val) = engine
        .run_section(
            "const s = DB.Query('SELECT * FROM t2 WHERE val = ?'); return JSON.stringify(await s.Bind('world').All())",
        )
        .await
        .unwrap();
    assert!(val.to_string().contains("world"));
}

#[tokio::test]
async fn test_db_table_where() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();

    engine
        .run_section(
            "const e = DB.Exec('CREATE TABLE IF NOT EXISTS t3 (id INTEGER PRIMARY KEY, val TEXT)'); await e.Run()",
        )
        .await
        .unwrap();
    engine
        .run_section("const t = DB.Table('t3'); await t.Insert({ val: 'findme' }).Run()")
        .await
        .unwrap();

    let (_text, val) = engine
        .run_section(
            "const t = DB.Table('t3'); return JSON.stringify(await t.Where({ val: 'findme' }).All())",
        )
        .await
        .unwrap();
    assert!(val.to_string().contains("findme"));
}

#[tokio::test]
async fn test_db_table_count() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();

    engine
        .run_section(
            "const e = DB.Exec('CREATE TABLE IF NOT EXISTS t4 (id INTEGER PRIMARY KEY, val TEXT)'); await e.Run()",
        )
        .await
        .unwrap();
    engine
        .run_section("const t = DB.Table('t4'); await t.Insert({ val: 'a' }).Run()")
        .await
        .unwrap();
    engine
        .run_section("const t = DB.Table('t4'); await t.Insert({ val: 'b' }).Run()")
        .await
        .unwrap();

    let (_text, val) = engine
        .run_section("const t = DB.Table('t4'); return await t.Count()")
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!(2));
}

#[tokio::test]
async fn test_db_table_delete() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();

    engine
        .run_section(
            "const e = DB.Exec('CREATE TABLE IF NOT EXISTS t5 (id INTEGER PRIMARY KEY, val TEXT)'); await e.Run()",
        )
        .await
        .unwrap();
    engine
        .run_section("const t = DB.Table('t5'); await t.Insert({ val: 'gone' }).Run()")
        .await
        .unwrap();

    let (_text, val) = engine
        .run_section(
            "const t = DB.Table('t5'); return JSON.stringify(await t.Where({ val: 'gone' }).Delete().Run())",
        )
        .await
        .unwrap();
    assert!(val.to_string().contains("rowsAffected"));
}

#[tokio::test]
async fn test_db_error_throws() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();

    let result = engine
        .run_section("const s = DB.Query('INVALID SQL'); await s.All()")
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_run_section_switch() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (text, _) = engine
        .run_section(
            r#"switch (2) {
                case 1: write("one"); break;
                case 2: write("two"); break;
                case 3: write("three"); break;
            }"#,
        )
        .await
        .unwrap();
    assert_eq!(text, "two");
}

#[tokio::test]
async fn test_run_section_ternary() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (text, _) = engine
        .run_section("write(true ? 'yes' : 'no')")
        .await
        .unwrap();
    assert_eq!(text, "yes");
}

#[tokio::test]
async fn test_run_section_bitwise() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine.run_section("return 5 & 3").await.unwrap();
    assert_eq!(val, serde_json::json!(1));
}

#[tokio::test]
async fn test_run_section_comparisons() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section("return '1' == 1 && '1' !== 1")
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!(true));
}

#[tokio::test]
async fn test_run_section_typeof() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section("return typeof null + ' ' + typeof undefined + ' ' + typeof 42 + ' ' + typeof 'hi' + ' ' + typeof true + ' ' + typeof {} + ' ' + typeof []")
        .await
        .unwrap();
    assert_eq!(
        val,
        serde_json::json!("object undefined number string boolean object object")
    );
}

#[tokio::test]
async fn test_run_section_closure() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section(
            "function makeCounter() { let n = 0; return () => ++n }; const c = makeCounter(); return c() + c() + c()",
        )
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!(6));
}

#[tokio::test]
async fn test_run_section_class() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section(
            "class Foo { constructor(x) { this.x = x } get() { return this.x * 2 } }; return new Foo(5).get()",
        )
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!(10));
}

#[tokio::test]
async fn test_run_section_destructuring() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section(
            "const { a, b } = { a: 10, b: 20 }; const [x, , z] = [1, 2, 3]; return a + b + x + z",
        )
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!(34));
}

#[tokio::test]
async fn test_run_section_template_literal() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (text, _) = engine
        .run_section("const name = 'World'; write(`Hello ${name}!`)")
        .await
        .unwrap();
    assert_eq!(text, "Hello World!");
}

#[tokio::test]
async fn test_run_section_nullish_coalescing() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section("return null ?? 'fallback'")
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!("fallback"));
}

#[tokio::test]
async fn test_run_section_optional_chaining() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();
    let (_text, val) = engine
        .run_section("const o = { a: { b: 42 } }; return o?.a?.b")
        .await
        .unwrap();
    assert_eq!(val, serde_json::json!(42));
}

#[tokio::test]
async fn test_db_statements_chain_sync() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();

    engine
        .run_section(
            "const e = DB.Exec('CREATE TABLE IF NOT EXISTS c1 (id INTEGER PRIMARY KEY, val TEXT)'); await e.Run()",
        )
        .await
        .unwrap();

    engine
        .run_section("const t = DB.Table('c1'); await t.Insert({ val: 'one' }).Run()")
        .await
        .unwrap();
    engine
        .run_section("const t = DB.Table('c1'); await t.Insert({ val: 'two' }).Run()")
        .await
        .unwrap();

    let (_text, val) = engine
        .run_section(
            "const t = DB.Table('c1'); return JSON.stringify(await t.Where({ val: 'one' }).All())",
        )
        .await
        .unwrap();
    let rows: serde_json::Value = serde_json::from_str(val.as_str().unwrap()).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["val"], serde_json::json!("one"));

    engine
        .run_section("const t = DB.Table('c1'); await t.Update({ val: 'updated' }).Run()")
        .await
        .unwrap();
    let (_text, val) = engine
        .run_section(
            "const t = DB.Table('c1'); return JSON.stringify(await t.Where({ val: 'updated' }).All())",
        )
        .await
        .unwrap();
    let rows: serde_json::Value = serde_json::from_str(val.as_str().unwrap()).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 2);
    assert_eq!(rows[0]["val"], serde_json::json!("updated"));
    assert_eq!(rows[1]["val"], serde_json::json!("updated"));

    let (_text, val) = engine
        .run_section(
            "const t = DB.Table('c1'); return JSON.stringify(await t.Where({ val: 'updated' }).Delete().Run())",
        )
        .await
        .unwrap();
    assert!(val.to_string().contains("rowsAffected"));

    let (_text, val) = engine
        .run_section(
            "const s = DB.Query('SELECT * FROM c1 WHERE val = ?'); return JSON.stringify(await s.Bind('updated').All())",
        )
        .await
        .unwrap();
    let rows: serde_json::Value = serde_json::from_str(val.as_str().unwrap()).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_transaction_commit_persists() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();

    engine
        .run_section("const e = DB.Exec('CREATE TABLE IF NOT EXISTS tx1 (id INTEGER PRIMARY KEY, val TEXT)'); await e.Run()")
        .await
        .unwrap();

    // Insert inside a transaction and commit.
    engine
        .run_section("await DB.StartTransaction()")
        .await
        .unwrap();
    let res = engine
        .run_section("const t = DB.Table('tx1'); return JSON.stringify(await t.Insert({ val: 'kept' }).Run())")
        .await
        .unwrap();
    assert!(res.1.to_string().contains("ok"));
    engine.run_section("await DB.Commit()").await.unwrap();

    // The row is visible afterwards.
    let (_t, val) = engine
        .run_section("const t = DB.Table('tx1'); return JSON.stringify(await t.All())")
        .await
        .unwrap();
    let rows: serde_json::Value = serde_json::from_str(val.as_str().unwrap()).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["val"], serde_json::json!("kept"));
}

#[tokio::test]
async fn test_transaction_rollback_discards() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();

    engine
        .run_section("const e = DB.Exec('CREATE TABLE IF NOT EXISTS tx2 (id INTEGER PRIMARY KEY, val TEXT)'); await e.Run()")
        .await
        .unwrap();

    engine
        .run_section("await DB.StartTransaction()")
        .await
        .unwrap();
    engine
        .run_section("const t = DB.Table('tx2'); await t.Insert({ val: 'dropped' }).Run()")
        .await
        .unwrap();
    engine.run_section("await DB.Rollback()").await.unwrap();

    let (_t, val) = engine
        .run_section("const t = DB.Table('tx2'); return JSON.stringify(await t.All())")
        .await
        .unwrap();
    let rows: serde_json::Value = serde_json::from_str(val.as_str().unwrap()).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_transaction_requires_commit_or_rollback_first() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();

    engine
        .run_section("await DB.StartTransaction()")
        .await
        .unwrap();
    // A second transaction while one is open must fail.
    let result = engine.run_section("await DB.StartTransaction()").await;
    assert!(result.is_err());
    // Clean up so the reserved connection is released.
    engine.run_section("await DB.Rollback()").await.unwrap();
    engine
        .run_section("await DB.StartTransaction()")
        .await
        .unwrap();
    engine.run_section("await DB.Commit()").await.unwrap();
}

#[tokio::test]
async fn test_delay_sleeps() {
    let conn = test_conn().await;
    let engine = Engine::new(conn).await.unwrap();
    engine.setup(&test_context()).await.unwrap();

    let start = std::time::Instant::now();
    engine.run_section("await delay(50)").await.unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed >= std::time::Duration::from_millis(40),
        "delay(50) returned too early: {elapsed:?}"
    );

    // delay can be awaited inline and chained with other output.
    let (text, _) = engine
        .run_section("write('a'); await delay(1); write('b')")
        .await
        .unwrap();
    assert_eq!(text, "ab");
}

use super::*;

async fn test_conn() -> DbConn {
    crate::db::connect("sqlite://file%3Arhp_proc_test?mode=memory&cache=shared")
        .await
        .expect("in memory db")
}

fn test_context() -> Context {
    Context::default()
}

#[tokio::test]
async fn test_syntax_error_inlines_in_html() {
    let conn = test_conn().await;
    let src = "<rhp>function(</rhp>".to_string();
    let (output, _) = process_src(src, test_context(), conn).await;
    assert!(
        output.contains("<pre>script error: function name expected"),
        "expected inline error, got: {output}"
    );
    assert!(output.contains("</pre>"), "missing closing pre tag");
}

#[tokio::test]
async fn test_runtime_throw_inlines_in_html() {
    let conn = test_conn().await;
    let src = "<rhp>throw new Error('boom')</rhp>".to_string();
    let (output, _) = process_src(src, test_context(), conn).await;
    assert!(
        output.contains("<pre>script error: boom"),
        "expected inline error, got: {output}"
    );
}

#[tokio::test]
async fn test_html_before_error_is_preserved() {
    let conn = test_conn().await;
    let src = "<h1>Hello</h1>\n<rhp>throw new Error('x')</rhp>".to_string();
    let (output, _) = process_src(src, test_context(), conn).await;
    assert!(output.contains("<h1>Hello</h1>"), "html lost: {output}");
    assert!(
        output.contains("<pre>script error:"),
        "error lost: {output}"
    );
    assert!(
        output.starts_with("<h1>Hello</h1>"),
        "html should come first: {output}"
    );
}

#[tokio::test]
async fn test_html_after_error_is_preserved() {
    let conn = test_conn().await;
    let src = "<rhp>throw new Error('y')</rhp>\n<p>after</p>".to_string();
    let (output, _) = process_src(src, test_context(), conn).await;
    assert!(
        output.contains("<pre>script error:"),
        "error lost: {output}"
    );
    assert!(
        output.contains("<p>after</p>"),
        "html after error lost: {output}"
    );
    assert!(
        output.find("<pre>script error:").unwrap() < output.find("<p>after</p>").unwrap(),
        "error should come before trailing html: {output}"
    );
}

#[tokio::test]
async fn test_multiple_errors_each_shown() {
    let conn = test_conn().await;
    let src = "<rhp>throw new Error('one')</rhp>\n<rhp>throw new Error('two')</rhp>".to_string();
    let (output, _) = process_src(src, test_context(), conn).await;
    let count = output.matches("<pre>script error:").count();
    assert_eq!(count, 2, "expected 2 error blocks, got {count}: {output}");
}

#[tokio::test]
async fn test_unclosed_rhp_tag_executes_as_code() {
    let conn = test_conn().await;
    let src = "<rhp>write('unclosed')".to_string();
    let (output, _) = process_src(src, test_context(), conn).await;
    assert!(
        output.contains("unclosed"),
        "code should still run: {output}"
    );
}

#[tokio::test]
async fn test_malformed_method_still_runs() {
    let conn = test_conn().await;
    let src = "<rhp method=broken>write('works')</rhp>".to_string();
    let (output, _) = process_src(src, test_context(), conn).await;
    assert!(
        output.contains("works"),
        "should run with All method: {output}"
    );
}

#[tokio::test]
async fn test_valid_script_no_error() {
    let conn = test_conn().await;
    let src = "<rhp>write('all good')</rhp>".to_string();
    let (output, _) = process_src(src, test_context(), conn).await;
    assert_eq!(output, "all good");
    assert!(
        !output.contains("<pre>"),
        "no error block expected: {output}"
    );
}

#[tokio::test]
async fn test_empty_script_section() {
    let conn = test_conn().await;
    let src = "<rhp></rhp>".to_string();
    let (output, _) = process_src(src, test_context(), conn).await;
    assert_eq!(output, "");
}

#[tokio::test]
async fn test_method_mismatch_skips_section() {
    let conn = test_conn().await;
    let context = Context {
        method: Method::Get,
        ..test_context()
    };
    let src =
        "<rhp method=\"POST\">throw new Error('should not run')</rhp>\n<p>visible</p>".to_string();
    let (output, _) = process_src(src, context, conn).await;
    assert!(output.contains("<p>visible</p>"), "html lost: {output}");
    assert!(
        !output.contains("<pre>"),
        "method-mismatch section should not execute: {output}"
    );
}

#[tokio::test]
async fn test_error_between_valid_sections() {
    let conn = test_conn().await;
    let src =
        "<rhp>write('before')</rhp>\n<rhp>throw new Error('oops')</rhp>\n<rhp>write('after')</rhp>"
            .to_string();
    let (output, _) = process_src(src, test_context(), conn).await;
    assert!(output.contains("before"), "first section lost: {output}");
    assert!(
        output.contains("<pre>script error:"),
        "error lost: {output}"
    );
    assert!(output.contains("after"), "third section lost: {output}");
}

#[tokio::test]
async fn test_inline_error_sets_status() {
    let conn = test_conn().await;
    let src = "<rhp>throw new Error('no status')</rhp>".to_string();
    let (_, response) = process_src(src, test_context(), conn).await;
    assert_eq!(
        response.status,
        Some(500),
        "script error should set HTTP status"
    );
}

#[tokio::test]
async fn test_raw_syntax_error_inlines_in_html() {
    let conn = test_conn().await;
    let src = "<rhp>if (</rhp>".to_string();
    let (output, _) = process_src(src, test_context(), conn).await;
    assert!(
        output.contains("<pre>"),
        "syntax error should produce pre tag: {output}"
    );
    assert!(
        output.contains("SyntaxError") || output.contains("error"),
        "should contain error detail: {output}"
    );
}

#[tokio::test]
async fn test_division_by_zero_throws() {
    let conn = test_conn().await;
    let src = "<rhp>const x = 1 / 0; throw new Error('bad')</rhp>".to_string();
    let (output, _) = process_src(src, test_context(), conn).await;
    assert!(
        output.contains("<pre>script error:"),
        "expected error: {output}"
    );
}

#[tokio::test]
async fn test_try_catch_prevents_error() {
    let conn = test_conn().await;
    let src =
        "<rhp>try { throw new Error('caught') } catch(e) { write('handled') }</rhp>".to_string();
    let (output, _) = process_src(src, test_context(), conn).await;
    assert_eq!(output, "handled");
    assert!(
        !output.contains("<pre>"),
        "catched error should not leak: {output}"
    );
}

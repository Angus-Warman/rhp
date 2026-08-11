use super::*;

#[test]
fn empty_src() {
    assert_eq!(parse(""), vec![]);
}

#[test]
fn test_basic_split() {
    let src = r#"<h1>Hello</h1><rhp>let x = 1</rhp><p>World</p>"#;
    let sections = parse(src);
    assert_eq!(sections, vec![
        Section::Html("<h1>Hello</h1>".to_string()),
        Section::Code("let x = 1".to_string()),
        Section::Html("<p>World</p>".to_string()),
    ]);
}

#[test]
fn test_multiple_code_blocks() {
    let src = "<rhp>a</rhp>mid<rhp>b</rhp>";
    let sections = parse(src);
    assert_eq!(sections, vec![
        Section::Code("a".to_string()),
        Section::Html("mid".to_string()),
        Section::Code("b".to_string()),
    ]);
}

#[test]
fn test_pure_html() {
    let src = "<p>just html</p>";
    assert_eq!(parse(src), vec![Section::Html(src.to_string())]);
}
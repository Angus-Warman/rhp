use super::*;

#[test]
fn test_basic_eval() {
    assert_eq!(evaluate("return 1 + 2"), "3");
}
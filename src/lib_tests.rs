use super::*;

#[test]
fn test_basic_eval() {
    assert_eq!(evaluate("return 1 + 2"), "3");
}


#[test]
fn test_pass_function() {
    assert_eq!(evaluate(r"
        let inc = (a) => a + 1
        let apply_twice = (f, n) => f(f(n))
        return apply_twice(inc, 2) 
    "), "4");
}
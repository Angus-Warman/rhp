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

#[test]
fn test_global_constants() {
    assert_eq!(evaluate(r"
        return VERSION 
    "), "0.0.1");
}

#[test]
fn test_console_log() {
    assert_eq!(evaluate(r"
        console.log('hello world') 
    "), "");
}

// #[test]
// fn test_object_prop() {
//     assert_eq!(evaluate(r"
//         let a = {}
//         a.b = 1
//         return a
//     "), "{ b: 1 }");
// }
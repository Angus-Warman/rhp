use rhp::parse;

fn main() {
    let res = parse("return 1 + 2");
    dbg!(res);
}

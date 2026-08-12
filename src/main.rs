use rhp::evaluate;

fn main() {
    let res = evaluate(r"
    console.log('hello world')
    ");
    dbg!(res);
}

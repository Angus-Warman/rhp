use rhp::evaluate;

fn main() {
    let res = evaluate(r"
    let x = (n) => n + 1;
    let y = (v) => v(2) * 2
    return y(x)
    ");
    dbg!(res);
}

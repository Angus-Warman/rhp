
#[tokio::main]
async fn main() {
    rhp::run_server().await;
}

// async fn rhp_handler(req: Request) -> Html<String> {
//     let path ="./public".to_owned() + req.uri().path();
//     let src = tokio::fs::read_to_string(path).await.unwrap();
//     Html(process_src(&src))
// }
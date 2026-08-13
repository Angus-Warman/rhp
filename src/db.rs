use anyhow::Result;

pub async fn ping() -> Result<String> {
    Ok("pong".to_string())
}
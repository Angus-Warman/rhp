use anyhow::Result;

pub async fn ping() -> Result<String> {
    return Ok("pong".to_string())
}
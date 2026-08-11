
#[derive(Debug, Clone, PartialEq)]
pub enum Section {
    Html(String),
    Code(String),
}

#[allow(dead_code)]
pub fn parse(src: &str) -> Vec<Section> {
    return vec![]
}

#[cfg(test)]
#[path = "./parser_tests.rs"]
mod tests;
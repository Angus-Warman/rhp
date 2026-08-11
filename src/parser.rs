#[derive(Debug, Clone, PartialEq)]
pub enum Section {
    Html(String),
    Code(String),
}

#[allow(dead_code)]
pub fn parse(src: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut rest = src;

    while !rest.is_empty() {
        match rest.find("<rhp>") {
            None => {
                // No more code blocks — remainder is HTML
                sections.push(Section::Html(rest.to_string()));
                break;
            }
            Some(html_end) => {
                // Capture HTML before the tag
                if html_end > 0 {
                    sections.push(Section::Html(rest[..html_end].to_string()));
                }

                let after_open = &rest[html_end + "<rhp>".len()..];

                match after_open.find("</rhp>") {
                    None => {
                        // Unclosed tag — treat the rest as a code block anyway,
                        // or you could return an Err here
                        sections.push(Section::Code(after_open.to_string()));
                        break;
                    }
                    Some(code_end) => {
                        sections.push(Section::Code(after_open[..code_end].to_string()));
                        rest = &after_open[code_end + "</rhp>".len()..];
                    }
                }
            }
        }
    }

    sections
}

#[cfg(test)]
#[path = "./parser_tests.rs"]
mod tests;
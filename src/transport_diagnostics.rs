//! Bounded transport diagnostics, excluding the request URL and all request data.
use serde_json::{Value, json};

pub(crate) fn transport_error(error: reqwest::Error) -> Value {
    let error = error.without_url();
    let mut causes = Vec::new();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        causes.push(safe_text(&cause.to_string()));
        if causes.len() >= 8 {
            break;
        }
        source = cause.source();
    }
    json!({
        "message": safe_text(&error.to_string()),
        "causes": causes,
        "timeout": error.is_timeout(),
        "connect": error.is_connect(),
        "body": error.is_body(),
        "decode": error.is_decode(),
        "request": error.is_request(),
        "status": error.status().map(|status| status.as_u16()),
    })
}

// Error descriptions must not preserve embedded URLs with query credentials.
fn safe_text(text: &str) -> String {
    text.split_whitespace()
        .take(100)
        .map(|word| {
            if word.contains("://") {
                "[URL omitida]"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(512)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn error_text_is_bounded_and_redacts_urls() {
        let text = safe_text("failed https://example.test/?access_token=secret connection reset");
        assert!(!text.contains("secret"));
        assert!(text.contains("connection reset"));
        assert_eq!(safe_text(&"x".repeat(2000)).len(), 512);
    }
    #[test]
    fn reqwest_url_is_not_included_in_diagnostics() {
        let error = reqwest::Client::new().get("http://[").build().unwrap_err();
        let details = transport_error(error);
        assert!(!details.to_string().contains("http://["));
        assert!(details["message"].is_string());
        assert!(details["causes"].is_array());
    }
}

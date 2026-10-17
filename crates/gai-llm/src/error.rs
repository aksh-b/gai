use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("rate limited: {message}")]
    RateLimited {
        retry_after: Option<Duration>,
        message: String,
    },
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("context window exceeded: {0}")]
    ContextLength(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("provider error {status}: {message}")]
    Provider { status: u16, message: String },
    #[error("network error: {0}")]
    Network(String),
    #[error("request timed out")]
    Timeout,
    #[error("could not decode provider response: {0}")]
    Decode(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("cancelled")]
    Cancelled,
}

impl LlmError {
    /// Worth retrying the same request against the same model.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::RateLimited { .. } | Self::Network(_) | Self::Timeout => true,
            Self::Provider { status, .. } => *status >= 500 || matches!(status, 408 | 409),
            _ => false,
        }
    }

    /// Worth trying a different model in a fallback chain. Broader than
    /// [`is_retryable`](Self::is_retryable): a missing key or unknown model on
    /// one provider says nothing about the next.
    pub fn is_fallbackable(&self) -> bool {
        self.is_retryable()
            || matches!(
                self,
                Self::Auth(_) | Self::NotFound(_) | Self::ContextLength(_) | Self::Config(_)
            )
    }

    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// Map an HTTP error status and body to an error, using the provider's
    /// message when the body carries one.
    pub fn from_status(status: u16, body: &str, retry_after: Option<Duration>) -> Self {
        let message = extract_message(body);
        let lower = message.to_lowercase();
        match status {
            401 | 403 => Self::Auth(message),
            404 => Self::NotFound(message),
            429 => Self::RateLimited {
                retry_after,
                message,
            },
            400 | 413 | 422
                if lower.contains("context_length")
                    || lower.contains("context length")
                    || lower.contains("prompt is too long")
                    || lower.contains("too many tokens")
                    || lower.contains("maximum context") =>
            {
                Self::ContextLength(message)
            }
            400 | 413 | 422 => Self::InvalidRequest(message),
            _ => Self::Provider { status, message },
        }
    }
}

impl From<reqwest::Error> for LlmError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            Self::Timeout
        } else if e.is_decode() {
            Self::Decode(e.to_string())
        } else {
            Self::Network(e.to_string())
        }
    }
}

/// Pull a human-readable message out of the error bodies providers return:
/// `{"error": {"message": ...}}`, `{"error": "..."}` or `{"message": ...}`.
fn extract_message(body: &str) -> String {
    let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();
    let from_json = parsed.as_ref().and_then(|v| {
        v.pointer("/error/message")
            .or_else(|| v.get("error").filter(|e| e.is_string()))
            .or_else(|| v.get("message"))
            .or_else(|| v.get("detail"))
            .and_then(|m| m.as_str())
            .map(str::to_owned)
    });
    from_json.unwrap_or_else(|| {
        let trimmed = body.trim();
        if trimmed.len() > 500 {
            format!("{}…", &trimmed[..trimmed.floor_char_boundary(500)])
        } else {
            trimmed.to_owned()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_statuses_and_messages() {
        let e = LlmError::from_status(
            400,
            r#"{"error":{"message":"prompt is too long: 300000 tokens"}}"#,
            None,
        );
        assert!(matches!(e, LlmError::ContextLength(_)));
        let e = LlmError::from_status(
            529,
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
            None,
        );
        assert!(e.is_retryable());
        assert_eq!(e.to_string(), "provider error 529: Overloaded");
        assert!(!LlmError::from_status(400, "bad", None).is_retryable());
        assert!(LlmError::from_status(401, "{}", None).is_fallbackable());
    }
}

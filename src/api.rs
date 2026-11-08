use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    Network(String),
    Auth { account: String, message: String },
    RateLimited,
    NotOwned(String),
    Other(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Network(m) => write!(f, "network error: {m}"),
            ApiError::Auth { account, message } => write!(f, "auth error for {account}: {message}"),
            ApiError::RateLimited => write!(f, "rate limited by Google"),
            ApiError::NotOwned(m) => write!(f, "calendar not owned by this account: {m}"),
            ApiError::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ApiError {}

pub trait AccessTokens {
    fn access_token(&mut self, account: &str) -> Result<String, ApiError>;
}

// Non-2xx responses come back as values so callers can read Google's error body
pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .into()
}

pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

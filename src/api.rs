use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    Network(String),
    Auth { account: String, message: String },
    RateLimited,
    NotOwned(String),
    Gone,
    Other(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Network(m) => write!(f, "network error: {m}"),
            ApiError::Auth { account, message } => write!(f, "auth error for {account}: {message}"),
            ApiError::RateLimited => write!(f, "rate limited by Google"),
            ApiError::NotOwned(m) => write!(f, "calendar not owned by this account: {m}"),
            ApiError::Gone => write!(f, "already deleted on Google"),
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

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarListEntry {
    pub id: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub access_role: String,
    #[serde(default)]
    pub primary: bool,
}

pub trait CalendarApi {
    fn list_events(
        &mut self,
        account: &str,
        calendar: &str,
        time_min: chrono::DateTime<chrono::Utc>,
        time_max: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<crate::event::GEvent>, ApiError>;
    fn list_copies(
        &mut self,
        account: &str,
        calendar: &str,
        time_min: chrono::DateTime<chrono::Utc>,
        time_max: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<crate::event::GEvent>, ApiError>;
    fn insert(
        &mut self,
        account: &str,
        calendar: &str,
        body: &serde_json::Value,
    ) -> Result<crate::event::GEvent, ApiError>;
    fn patch(
        &mut self,
        account: &str,
        calendar: &str,
        event_id: &str,
        body: &serde_json::Value,
    ) -> Result<crate::event::GEvent, ApiError>;
    fn delete(&mut self, account: &str, calendar: &str, event_id: &str) -> Result<(), ApiError>;
    fn list_calendars(&mut self, account: &str) -> Result<Vec<CalendarListEntry>, ApiError>;
}

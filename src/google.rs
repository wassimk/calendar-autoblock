use crate::api::{AccessTokens, ApiError, CalendarApi, CalendarListEntry, agent, percent_encode};
use crate::event::GEvent;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::time::Duration;

pub const BASE_URL: &str = "https://www.googleapis.com/calendar/v3";
const RETRY_DELAYS: [u64; 3] = [1, 2, 4];

pub struct GoogleCalendar<T: AccessTokens> {
    tokens: T,
    agent: ureq::Agent,
    base: String,
}

/// The Calendar API root; CALENDAR_AUTOBLOCK_API_BASE points it at a local stand-in for demos
pub fn api_base(override_url: Option<String>) -> String {
    override_url.map_or_else(
        || BASE_URL.to_string(),
        |u| u.trim_end_matches('/').to_string(),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    #[serde(default = "Vec::new")]
    items: Vec<T>,
    next_page_token: Option<String>,
}

enum Method {
    Get,
    Post,
    Patch,
    Delete,
}

impl<T: AccessTokens> GoogleCalendar<T> {
    pub fn new(tokens: T) -> Self {
        Self {
            tokens,
            agent: agent(),
            base: api_base(std::env::var("CALENDAR_AUTOBLOCK_API_BASE").ok()),
        }
    }

    fn send(
        &mut self,
        account: &str,
        method: Method,
        url: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<String, ApiError> {
        let mut attempt = 0;
        loop {
            let auth = format!("Bearer {}", self.tokens.access_token(account)?);
            let empty = serde_json::Value::Null;
            let result = match method {
                Method::Get => self.agent.get(url).header("Authorization", &auth).call(),
                Method::Delete => self.agent.delete(url).header("Authorization", &auth).call(),
                Method::Post => self
                    .agent
                    .post(url)
                    .header("Authorization", &auth)
                    .send_json(body.unwrap_or(&empty)),
                Method::Patch => self
                    .agent
                    .patch(url)
                    .header("Authorization", &auth)
                    .send_json(body.unwrap_or(&empty)),
            };
            let mut resp = result.map_err(|e| ApiError::Network(e.to_string()))?;
            let status = resp.status().as_u16();
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            if status < 300 {
                return Ok(text);
            }
            let err = classify(status, &text, account);
            let retryable = err == ApiError::RateLimited || status >= 500;
            match RETRY_DELAYS.get(attempt) {
                Some(secs) if retryable => {
                    std::thread::sleep(Duration::from_secs(*secs));
                    attempt += 1;
                }
                _ => return Err(err),
            }
        }
    }

    fn paginate<I: DeserializeOwned>(
        &mut self,
        account: &str,
        url_for: impl Fn(Option<&str>) -> String,
    ) -> Result<Vec<I>, ApiError> {
        let mut all = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page: Page<I> =
                parse_page(&self.send(account, Method::Get, &url_for(token.as_deref()), None)?)?;
            all.extend(page.items);
            match page.next_page_token {
                Some(n) => token = Some(n),
                None => return Ok(all),
            }
        }
    }

    fn list(
        &mut self,
        account: &str,
        calendar: &str,
        time_min: DateTime<Utc>,
        time_max: DateTime<Utc>,
        copies_only: bool,
    ) -> Result<Vec<GEvent>, ApiError> {
        let base = self.base.clone();
        self.paginate(account, |page| {
            events_url(&base, calendar, time_min, time_max, copies_only, page)
        })
        // A calendar this account can't see lists as 404, which means the same as 403 here
        .map_err(|e| {
            if e == ApiError::Gone {
                ApiError::NotOwned(calendar.to_string())
            } else {
                e
            }
        })
    }

    fn write(
        &mut self,
        account: &str,
        method: Method,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<GEvent, ApiError> {
        parse_event(&self.send(account, method, url, Some(body))?)
    }
}

fn event_url(base: &str, calendar: &str, event_id: Option<&str>) -> String {
    let mut url = format!("{base}/calendars/{}/events", percent_encode(calendar));
    if let Some(id) = event_id {
        url.push('/');
        url.push_str(&percent_encode(id));
    }
    url.push_str("?sendUpdates=none");
    url
}

impl<T: AccessTokens> CalendarApi for GoogleCalendar<T> {
    fn list_events(
        &mut self,
        a: &str,
        c: &str,
        min: DateTime<Utc>,
        max: DateTime<Utc>,
    ) -> Result<Vec<GEvent>, ApiError> {
        self.list(a, c, min, max, false)
    }

    fn list_copies(
        &mut self,
        a: &str,
        c: &str,
        min: DateTime<Utc>,
        max: DateTime<Utc>,
    ) -> Result<Vec<GEvent>, ApiError> {
        self.list(a, c, min, max, true)
    }

    fn insert(&mut self, a: &str, c: &str, body: &serde_json::Value) -> Result<GEvent, ApiError> {
        self.write(a, Method::Post, &event_url(&self.base, c, None), body)
    }

    fn patch(
        &mut self,
        a: &str,
        c: &str,
        id: &str,
        body: &serde_json::Value,
    ) -> Result<GEvent, ApiError> {
        self.write(a, Method::Patch, &event_url(&self.base, c, Some(id)), body)
    }

    fn delete(&mut self, a: &str, c: &str, id: &str) -> Result<(), ApiError> {
        match self.send(a, Method::Delete, &event_url(&self.base, c, Some(id)), None) {
            Ok(_) | Err(ApiError::Gone) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn list_calendars(&mut self, a: &str) -> Result<Vec<CalendarListEntry>, ApiError> {
        let base = self.base.clone();
        self.paginate(a, |page| {
            let mut url = format!("{base}/users/me/calendarList?maxResults=250");
            if let Some(t) = page {
                url.push_str(&format!("&pageToken={}", percent_encode(t)));
            }
            url
        })
    }
}

pub fn classify(status: u16, body: &str, account: &str) -> ApiError {
    let rate_limited =
        body.contains("\"rateLimitExceeded\"") || body.contains("\"userRateLimitExceeded\"");
    match status {
        401 => ApiError::Auth {
            account: account.to_string(),
            message: "Google rejected the access token".into(),
        },
        429 => ApiError::RateLimited,
        403 if rate_limited => ApiError::RateLimited,
        403 => ApiError::NotOwned(snippet(body)),
        404 | 410 => ApiError::Gone,
        _ => ApiError::Other(format!("Google returned {status}: {}", snippet(body))),
    }
}

fn snippet(body: &str) -> String {
    body.chars().take(200).collect()
}

pub fn events_url(
    base: &str,
    calendar: &str,
    time_min: DateTime<Utc>,
    time_max: DateTime<Utc>,
    copies_only: bool,
    page_token: Option<&str>,
) -> String {
    let fmt = |t: DateTime<Utc>| percent_encode(&t.to_rfc3339_opts(SecondsFormat::Secs, true));
    let mut url = format!(
        "{base}/calendars/{}/events?singleEvents=true&showDeleted=false&maxResults=2500&timeMin={}&timeMax={}",
        percent_encode(calendar),
        fmt(time_min),
        fmt(time_max),
    );
    if copies_only {
        url.push_str(&format!(
            "&privateExtendedProperty={}",
            percent_encode("cab_managed=1")
        ));
    }
    if let Some(t) = page_token {
        url.push_str(&format!("&pageToken={}", percent_encode(t)));
    }
    url
}

fn parse_page<I: DeserializeOwned>(body: &str) -> Result<Page<I>, ApiError> {
    serde_json::from_str(body)
        .map_err(|e| ApiError::Other(format!("unreadable Google response: {e}")))
}

fn parse_event(body: &str) -> Result<GEvent, ApiError> {
    serde_json::from_str(body)
        .map_err(|e| ApiError::Other(format!("unreadable Google response: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_base_defaults_to_google_and_honors_override() {
        assert_eq!(api_base(None), BASE_URL);
        assert_eq!(
            api_base(Some("http://127.0.0.1:8765/".into())),
            "http://127.0.0.1:8765"
        );
    }

    #[test]
    fn classify_maps_statuses() {
        assert!(
            matches!(classify(401, "{}", "work"), ApiError::Auth { account, .. } if account == "work")
        );
        let rate = r#"{"error":{"errors":[{"reason":"rateLimitExceeded"}],"code":403}}"#;
        assert_eq!(classify(403, rate, "work"), ApiError::RateLimited);
        let user_rate = r#"{"error":{"errors":[{"reason":"userRateLimitExceeded"}],"code":403}}"#;
        assert_eq!(classify(403, user_rate, "work"), ApiError::RateLimited);
        assert_eq!(classify(429, "{}", "work"), ApiError::RateLimited);
        let forbidden = r#"{"error":{"errors":[{"reason":"forbidden"}],"code":403}}"#;
        assert!(matches!(
            classify(403, forbidden, "work"),
            ApiError::NotOwned(_)
        ));
        assert_eq!(classify(404, "{}", "work"), ApiError::Gone);
        assert_eq!(classify(410, "{}", "work"), ApiError::Gone);
        assert!(matches!(classify(500, "{}", "work"), ApiError::Other(_)));
    }

    #[test]
    fn parses_paginated_events() {
        let a: Page<GEvent> =
            parse_page(include_str!("../tests/fixtures/events_page1.json")).unwrap();
        assert_eq!(a.next_page_token.as_deref(), Some("PAGE2"));
        let b: Page<GEvent> =
            parse_page(include_str!("../tests/fixtures/events_page2.json")).unwrap();
        assert_eq!(b.next_page_token, None);
        let ids: Vec<_> = a
            .items
            .iter()
            .chain(b.items.iter())
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn parses_calendar_list_roles() {
        let cals =
            parse_page::<CalendarListEntry>(include_str!("../tests/fixtures/calendar_list.json"))
                .unwrap()
                .items;
        assert_eq!(cals.len(), 3);
        assert!(cals[0].primary);
        assert_eq!(cals[1].summary, "Appointments");
        assert_eq!(cals[1].access_role, "owner");
        assert_eq!(cals[2].access_role, "writer");
    }

    #[test]
    fn events_url_has_required_params() {
        let min: DateTime<Utc> = "2026-11-09T12:00:00Z".parse().unwrap();
        let max: DateTime<Utc> = "2027-02-07T12:00:00Z".parse().unwrap();
        let url = events_url(
            BASE_URL,
            "fake123@group.calendar.google.com",
            min,
            max,
            true,
            Some("P2"),
        );
        assert!(url.starts_with(
            "https://www.googleapis.com/calendar/v3/calendars/fake123%40group.calendar.google.com/events?"
        ));
        for part in [
            "singleEvents=true",
            "showDeleted=false",
            "maxResults=2500",
            "timeMin=2026-11-09T12%3A00%3A00Z",
            "timeMax=2027-02-07T12%3A00%3A00Z",
            "privateExtendedProperty=cab_managed%3D1",
            "pageToken=P2",
        ] {
            assert!(url.contains(part), "missing {part} in {url}");
        }
        let plain = events_url(BASE_URL, "primary", min, max, false, None);
        assert!(!plain.contains("privateExtendedProperty"));
        assert!(!plain.contains("pageToken"));
    }
}

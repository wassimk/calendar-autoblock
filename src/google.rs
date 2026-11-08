use crate::api::{AccessTokens, ApiError, CalendarApi, CalendarListEntry, agent, percent_encode};
use crate::event::GEvent;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use std::time::Duration;

pub const BASE_URL: &str = "https://www.googleapis.com/calendar/v3";
const RETRY_DELAYS: [u64; 3] = [1, 2, 4];

pub struct GoogleCalendar<T: AccessTokens> {
    tokens: T,
    base_url: String,
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
            base_url: BASE_URL.to_string(),
        }
    }

    fn send(
        &mut self,
        account: &str,
        method: Method,
        url: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<(u16, String), ApiError> {
        let mut attempt = 0;
        loop {
            let token = self.tokens.access_token(account)?;
            let auth = format!("Bearer {token}");
            let agent = agent();
            let result = match (&method, body) {
                (Method::Get, _) => agent.get(url).header("Authorization", &auth).call(),
                (Method::Delete, _) => agent.delete(url).header("Authorization", &auth).call(),
                (Method::Post, b) => agent
                    .post(url)
                    .header("Authorization", &auth)
                    .send_json(b.cloned().unwrap_or_default()),
                (Method::Patch, b) => agent
                    .patch(url)
                    .header("Authorization", &auth)
                    .send_json(b.cloned().unwrap_or_default()),
            };
            let mut resp = result.map_err(|e| ApiError::Network(e.to_string()))?;
            let status = resp.status().as_u16();
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            if status < 300 {
                return Ok((status, text));
            }
            let err = classify(status, &text, account);
            let retryable = err == ApiError::RateLimited || status >= 500;
            match RETRY_DELAYS.get(attempt) {
                Some(secs) if retryable => {
                    std::thread::sleep(Duration::from_secs(*secs));
                    attempt += 1;
                }
                _ => {
                    return Err(if status == 404 || status == 410 {
                        ApiError::NotOwned(text)
                    } else {
                        err
                    });
                }
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
        let mut all = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let url = events_url(
                &self.base_url,
                calendar,
                time_min,
                time_max,
                copies_only,
                token.as_deref(),
            );
            let (_, text) = self.send(account, Method::Get, &url, None)?;
            let (events, next) = parse_events_page(&text)?;
            all.extend(events);
            match next {
                Some(n) => token = Some(n),
                None => return Ok(all),
            }
        }
    }

    fn event_url(&self, calendar: &str, event_id: Option<&str>) -> String {
        let mut url = format!(
            "{}/calendars/{}/events",
            self.base_url,
            percent_encode(calendar)
        );
        if let Some(id) = event_id {
            url.push('/');
            url.push_str(&percent_encode(id));
        }
        url.push_str("?sendUpdates=none");
        url
    }
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
        let url = self.event_url(c, None);
        let (_, text) = self.send(a, Method::Post, &url, Some(body))?;
        serde_json::from_str(&text)
            .map_err(|e| ApiError::Other(format!("unreadable insert response: {e}")))
    }

    fn patch(
        &mut self,
        a: &str,
        c: &str,
        id: &str,
        body: &serde_json::Value,
    ) -> Result<GEvent, ApiError> {
        let url = self.event_url(c, Some(id));
        let (_, text) = self.send(a, Method::Patch, &url, Some(body))?;
        serde_json::from_str(&text)
            .map_err(|e| ApiError::Other(format!("unreadable patch response: {e}")))
    }

    fn delete(&mut self, a: &str, c: &str, id: &str) -> Result<(), ApiError> {
        let url = self.event_url(c, Some(id));
        match self.send(a, Method::Delete, &url, None) {
            // Already gone counts as deleted
            Ok(_) | Err(ApiError::NotOwned(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn list_calendars(&mut self, a: &str) -> Result<Vec<CalendarListEntry>, ApiError> {
        let mut all = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut url = format!("{}/users/me/calendarList?maxResults=250", self.base_url);
            if let Some(t) = &token {
                url.push_str(&format!("&pageToken={}", percent_encode(t)));
            }
            let (_, text) = self.send(a, Method::Get, &url, None)?;
            let (cals, next) = parse_calendar_list(&text)?;
            all.extend(cals);
            match next {
                Some(n) => token = Some(n),
                None => return Ok(all),
            }
        }
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
        403 | 404 => ApiError::NotOwned(snippet(body)),
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

pub fn parse_events_page(body: &str) -> Result<(Vec<GEvent>, Option<String>), ApiError> {
    let page: Page<GEvent> = serde_json::from_str(body)
        .map_err(|e| ApiError::Other(format!("unreadable events page: {e}")))?;
    Ok((page.items, page.next_page_token))
}

pub fn parse_calendar_list(
    body: &str,
) -> Result<(Vec<CalendarListEntry>, Option<String>), ApiError> {
    let page: Page<CalendarListEntry> = serde_json::from_str(body)
        .map_err(|e| ApiError::Other(format!("unreadable calendar list: {e}")))?;
    Ok((page.items, page.next_page_token))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(matches!(classify(404, "{}", "work"), ApiError::NotOwned(_)));
        assert!(matches!(classify(500, "{}", "work"), ApiError::Other(_)));
    }

    #[test]
    fn parses_paginated_events() {
        let (a, next) =
            parse_events_page(include_str!("../tests/fixtures/events_page1.json")).unwrap();
        assert_eq!(next.as_deref(), Some("PAGE2"));
        let (b, last) =
            parse_events_page(include_str!("../tests/fixtures/events_page2.json")).unwrap();
        assert_eq!(last, None);
        let ids: Vec<_> = a.iter().chain(b.iter()).map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn parses_calendar_list_roles() {
        let (cals, _) =
            parse_calendar_list(include_str!("../tests/fixtures/calendar_list.json")).unwrap();
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

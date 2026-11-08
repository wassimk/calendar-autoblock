use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GEvent {
    pub id: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    pub start: EventTime,
    pub end: EventTime,
    #[serde(default)]
    pub transparency: Option<String>,
    #[serde(default)]
    pub visibility: Option<String>,
    #[serde(default)]
    pub event_type: Option<String>,
    #[serde(default)]
    pub attendees: Option<Vec<Attendee>>,
    #[serde(default)]
    pub recurring_event_id: Option<String>,
    #[serde(default, rename = "iCalUID")]
    pub i_cal_uid: Option<String>,
    #[serde(default)]
    pub original_start_time: Option<EventTime>,
    #[serde(default)]
    pub extended_properties: Option<ExtendedProperties>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventTime {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_time: Option<DateTime<FixedOffset>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attendee {
    #[serde(default, rename = "self")]
    pub is_self: Option<bool>,
    #[serde(default)]
    pub response_status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ExtendedProperties {
    #[serde(default)]
    pub private: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct CopyKey {
    pub ical_uid: String,
    pub original_start: Option<String>,
}

impl EventTime {
    fn to_utc(&self, tz: Tz) -> Option<DateTime<Utc>> {
        if let Some(dt) = self.date_time {
            return Some(dt.with_timezone(&Utc));
        }
        let midnight = self.date?.and_hms_opt(0, 0, 0)?;
        tz.from_local_datetime(&midnight)
            .earliest()
            .map(|d| d.with_timezone(&Utc))
    }

    fn key_string(&self) -> Option<String> {
        match (self.date_time, self.date) {
            (Some(dt), _) => Some(
                dt.with_timezone(&Utc)
                    .format("%Y-%m-%dT%H:%M:%SZ")
                    .to_string(),
            ),
            (None, Some(d)) => Some(d.to_string()),
            _ => None,
        }
    }
}

impl GEvent {
    pub fn bounds(&self, tz: Tz) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        Some((self.start.to_utc(tz)?, self.end.to_utc(tz)?))
    }

    pub fn is_all_day(&self) -> bool {
        self.start.date_time.is_none() && self.start.date.is_some()
    }

    pub fn tag(&self, name: &str) -> Option<&str> {
        self.extended_properties
            .as_ref()?
            .private
            .as_ref()?
            .get(name)
            .map(String::as_str)
    }

    pub fn is_copy(&self) -> bool {
        self.tag("cab_managed") == Some("1")
    }

    pub fn copy_key(&self) -> Option<CopyKey> {
        if self.is_copy() {
            let original = self.tag("cab_original_start").filter(|s| !s.is_empty());
            return Some(CopyKey {
                ical_uid: self.tag("cab_ical_uid")?.to_string(),
                original_start: original.map(str::to_string),
            });
        }
        Some(CopyKey {
            ical_uid: self.i_cal_uid.clone().unwrap_or_else(|| self.id.clone()),
            original_start: self
                .original_start_time
                .as_ref()
                .and_then(EventTime::key_string),
        })
    }
}

pub fn copy_body(
    source: &GEvent,
    source_account: &str,
    source_calendar: &str,
    label: &str,
) -> serde_json::Value {
    let key = source.copy_key().expect("source events always have a key");
    json!({
        "summary": label,
        "visibility": "public",
        "transparency": "opaque",
        "reminders": { "useDefault": false, "overrides": [] },
        "start": source.start,
        "end": source.end,
        "extendedProperties": { "private": {
            "cab_managed": "1",
            "cab_source_account": source_account,
            "cab_source_calendar": source_calendar,
            "cab_source_event": source.id,
            "cab_ical_uid": key.ical_uid,
            "cab_original_start": key.original_start.unwrap_or_default(),
            "cab_recurring_event_id": source.recurring_event_id.clone().unwrap_or_default(),
        }},
    })
}

#[cfg(test)]
pub mod test_support {
    use super::GEvent;
    use crate::config::WorkHours;
    use chrono::{NaiveTime, Weekday};

    pub fn event(name: &str) -> GEvent {
        let all: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/events_mixed.json")).unwrap();
        serde_json::from_value(all[name].clone()).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    pub fn wassim_hours() -> WorkHours {
        WorkHours {
            timezone: chrono_tz::America::Chicago,
            days: vec![
                Weekday::Mon,
                Weekday::Tue,
                Weekday::Wed,
                Weekday::Thu,
                Weekday::Fri,
            ],
            start: NaiveTime::from_hms_opt(8, 0, 0).unwrap(),
            end: NaiveTime::from_hms_opt(17, 0, 0).unwrap(),
            margin_before: chrono::Duration::hours(1),
            margin_after: chrono::Duration::hours(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::event;
    use super::*;

    #[test]
    fn copy_key_uses_original_start_for_instances() {
        let a = event("inst_orig").copy_key().unwrap();
        let b = event("inst_moved").copy_key().unwrap();
        assert_eq!(a, b);
        assert_eq!(a.ical_uid, "series1@google.com");
        assert_eq!(a.original_start.as_deref(), Some("2026-11-10T16:00:00Z"));
        assert_eq!(event("own").copy_key().unwrap().original_start, None);
    }

    #[test]
    fn copy_key_of_a_copy_comes_from_tags() {
        let key = event("copy").copy_key().unwrap();
        assert_eq!(
            key,
            CopyKey {
                ical_uid: "own@google.com".into(),
                original_start: None
            }
        );
    }

    #[test]
    fn all_day_bounds_use_local_midnight() {
        let (s, e) = event("allday_busy")
            .bounds(chrono_tz::America::Chicago)
            .unwrap();
        assert_eq!(s, "2026-11-09T06:00:00Z".parse::<DateTime<Utc>>().unwrap());
        assert_eq!(e, "2026-11-10T06:00:00Z".parse::<DateTime<Utc>>().unwrap());
    }

    #[test]
    fn copy_body_has_exact_fields() {
        let src = event("accepted");
        let body = copy_body(&src, "personal", "cal123", "Personal Commitment");
        assert_eq!(body["summary"], "Personal Commitment");
        assert_eq!(body["visibility"], "public");
        assert_eq!(body["transparency"], "opaque");
        assert_eq!(
            body["reminders"],
            serde_json::json!({"useDefault": false, "overrides": []})
        );
        assert_eq!(body["start"]["dateTime"], "2026-11-09T10:00:00-06:00");
        assert_eq!(body["start"]["timeZone"], "America/Chicago");
        let tags = &body["extendedProperties"]["private"];
        assert_eq!(tags["cab_managed"], "1");
        assert_eq!(tags["cab_source_account"], "personal");
        assert_eq!(tags["cab_source_calendar"], "cal123");
        assert_eq!(tags["cab_source_event"], "accepted");
        assert_eq!(tags["cab_ical_uid"], "accepted@google.com");
        assert_eq!(tags["cab_original_start"], "");
        assert_eq!(tags["cab_recurring_event_id"], "");
        for absent in ["description", "location", "attendees", "conferenceData"] {
            assert!(body.get(absent).is_none(), "{absent} must not be copied");
        }
    }
}

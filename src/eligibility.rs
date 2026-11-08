use crate::config::WorkHours;
use crate::event::GEvent;
use crate::window::{overlaps_any, windows_between};
use chrono::{DateTime, Utc};

pub fn is_eligible(ev: &GEvent, hours: &WorkHours, now: DateTime<Utc>, horizon_days: u32) -> bool {
    if ev.status.as_deref() == Some("cancelled") || ev.is_copy() {
        return false;
    }
    if !matches!(
        ev.event_type.as_deref(),
        None | Some("default") | Some("outOfOffice")
    ) {
        return false;
    }
    if ev.transparency.as_deref() == Some("transparent") {
        return false;
    }
    let my_response = ev
        .attendees
        .iter()
        .flatten()
        .find(|a| a.is_self == Some(true))
        .and_then(|a| a.response_status.as_deref());
    if !matches!(my_response, None | Some("accepted") | Some("tentative")) {
        return false;
    }
    let Some((start, end)) = ev.bounds(hours.timezone) else {
        return false;
    };
    let horizon_end = now + chrono::Duration::days(horizon_days.into());
    if end <= now || start >= horizon_end {
        return false;
    }
    overlaps_any(&windows_between(hours, start, end), start, end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::test_support::{event, wassim_hours};

    fn now() -> DateTime<Utc> {
        "2026-11-09T12:00:00Z".parse().unwrap()
    }

    fn ok(name: &str) -> bool {
        is_eligible(&event(name), &wassim_hours(), now(), 90)
    }

    #[test]
    fn accepted_timed_event_in_window_is_eligible() {
        assert!(ok("accepted"));
    }

    #[test]
    fn own_event_without_attendees_is_eligible() {
        assert!(ok("own"));
    }

    #[test]
    fn tentative_is_eligible() {
        assert!(ok("tentative"));
    }

    #[test]
    fn declined_is_not_eligible() {
        assert!(!ok("declined"));
    }

    #[test]
    fn needs_action_is_not_eligible() {
        assert!(!ok("needs_action"));
    }

    #[test]
    fn transparent_is_not_eligible() {
        assert!(!ok("transparent"));
    }

    #[test]
    fn cancelled_is_not_eligible() {
        assert!(!ok("cancelled"));
    }

    #[test]
    fn busy_all_day_is_eligible() {
        assert!(ok("allday_busy"));
    }

    #[test]
    fn free_all_day_is_not_eligible() {
        assert!(!ok("allday_free"));
    }

    #[test]
    fn out_of_office_is_eligible() {
        assert!(ok("ooo"));
    }

    #[test]
    fn working_location_focus_time_birthday_from_gmail_are_not_eligible() {
        for name in ["working_location", "focus_time", "birthday", "from_gmail"] {
            assert!(!ok(name), "{name} should not be eligible");
        }
    }

    #[test]
    fn copy_is_never_eligible() {
        assert!(!ok("copy"));
    }

    #[test]
    fn outside_window_is_not_eligible() {
        assert!(!ok("early_morning"));
    }

    #[test]
    fn ended_event_is_not_eligible() {
        let after_it_ended = "2026-11-09T17:30:00Z".parse().unwrap();
        assert!(!is_eligible(
            &event("accepted"),
            &wassim_hours(),
            after_it_ended,
            90
        ));
    }

    #[test]
    fn in_progress_event_is_eligible() {
        let during = "2026-11-09T16:30:00Z".parse().unwrap();
        assert!(is_eligible(&event("accepted"), &wassim_hours(), during, 90));
    }

    #[test]
    fn beyond_horizon_is_not_eligible() {
        assert!(!ok("beyond_horizon"));
        assert!(is_eligible(
            &event("beyond_horizon"),
            &wassim_hours(),
            now(),
            120
        ));
    }
}

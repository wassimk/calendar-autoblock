use crate::config::WorkHours;
use chrono::{DateTime, Datelike, Days, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

pub fn windows_between(hours: &WorkHours, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<Window> {
    let tz = hours.timezone;
    // Margins can push a window across midnight, so look one local day beyond each end
    let first = from.with_timezone(&tz).date_naive() - Days::new(1);
    let last = to.with_timezone(&tz).date_naive() + Days::new(1);
    first
        .iter_days()
        .take_while(|d| *d <= last)
        .filter(|d| hours.days.contains(&d.weekday()))
        .map(|d| Window {
            start: local(tz, d, hours.start) - hours.margin_before,
            end: local(tz, d, hours.end) + hours.margin_after,
        })
        .filter(|w| w.start < to && w.end > from)
        .collect()
}

pub fn overlaps_any(windows: &[Window], start: DateTime<Utc>, end: DateTime<Utc>) -> bool {
    windows.iter().any(|w| start < w.end && end > w.start)
}

// A wall time that falls in a DST gap moves forward an hour, like a clock would
fn local(tz: Tz, date: NaiveDate, time: NaiveTime) -> DateTime<Utc> {
    let naive = date.and_time(time);
    tz.from_local_datetime(&naive)
        .earliest()
        .or_else(|| {
            tz.from_local_datetime(&(naive + chrono::Duration::hours(1)))
                .earliest()
        })
        .expect("valid local time")
        .with_timezone(&Utc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkHours;
    use chrono::{NaiveTime, TimeZone, Weekday};
    use chrono_tz::America::Chicago;

    fn hours() -> WorkHours {
        WorkHours {
            timezone: Chicago,
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

    fn utc(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn ct(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Chicago
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn monday_window_includes_margins() {
        let w = windows_between(
            &hours(),
            utc("2026-11-09T06:00:00Z"),
            utc("2026-11-10T06:00:00Z"),
        );
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].start, utc("2026-11-09T13:00:00Z"));
        assert_eq!(w[0].end, utc("2026-11-10T00:00:00Z"));
    }

    #[test]
    fn weekend_has_no_window() {
        let w = windows_between(&hours(), ct(2026, 11, 14, 0, 0), ct(2026, 11, 15, 23, 59));
        assert!(w.is_empty());
    }

    #[test]
    fn window_follows_dst_start() {
        let w = windows_between(&hours(), ct(2027, 3, 15, 0, 0), ct(2027, 3, 15, 23, 59));
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].start, utc("2027-03-15T12:00:00Z"));
        assert_eq!(w[0].end, utc("2027-03-15T23:00:00Z"));
    }

    #[test]
    fn appointment_after_work_inside_margin_overlaps() {
        let w = windows_between(&hours(), ct(2026, 11, 9, 0, 0), ct(2026, 11, 10, 0, 0));
        assert!(overlaps_any(
            &w,
            ct(2026, 11, 9, 17, 30),
            ct(2026, 11, 9, 18, 30)
        ));
    }

    #[test]
    fn appointment_starting_at_margin_end_does_not_overlap() {
        let w = windows_between(&hours(), ct(2026, 11, 9, 0, 0), ct(2026, 11, 10, 0, 0));
        assert!(!overlaps_any(
            &w,
            ct(2026, 11, 9, 18, 0),
            ct(2026, 11, 9, 19, 0)
        ));
    }

    #[test]
    fn event_crossing_midnight_into_window_overlaps() {
        let start = ct(2026, 11, 8, 23, 0);
        let end = ct(2026, 11, 9, 7, 30);
        let w = windows_between(&hours(), start, end);
        assert!(overlaps_any(&w, start, end));
    }
}

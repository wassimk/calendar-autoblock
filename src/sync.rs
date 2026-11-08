use crate::api::{ApiError, CalendarApi};
use crate::config::Config;
use crate::eligibility::is_eligible;
use crate::event::{CopyKey, GEvent, copy_body};
use crate::reconcile::{Action, Desired, Existing, approx_end, plan, prune_tombstones};
use crate::state::{Counts, KnownCopy, State};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default)]
pub struct PassReport {
    pub counts: Counts,
    pub actions: Vec<Action>,
    pub failures: Vec<(String, ApiError)>,
    pub succeeded: Vec<String>,
    pub all_reads_ok: bool,
}

pub fn run_pass(
    cfg: &Config,
    state: &mut State,
    api: &mut dyn CalendarApi,
    now: DateTime<Utc>,
    dry_run: bool,
) -> PassReport {
    if dry_run {
        let mut scratch = state.clone();
        return pass(cfg, &mut scratch, api, now, true);
    }
    pass(cfg, state, api, now, false)
}

fn pass(
    cfg: &Config,
    state: &mut State,
    api: &mut dyn CalendarApi,
    now: DateTime<Utc>,
    dry_run: bool,
) -> PassReport {
    let mut report = PassReport {
        all_reads_ok: true,
        ..Default::default()
    };
    let time_max = now + chrono::Duration::days(cfg.horizon_days.into());
    let mut seen: BTreeSet<CopyKey> = BTreeSet::new();

    let mut by_dest: BTreeMap<(String, String), Vec<&crate::config::SyncRule>> = BTreeMap::new();
    for rule in &cfg.sync {
        by_dest
            .entry((rule.to.account.clone(), rule.to.calendar.clone()))
            .or_default()
            .push(rule);
    }

    for ((dest_account, dest_calendar), rules) in by_dest {
        let mut desired = Vec::new();
        let mut deletable = BTreeSet::new();
        for rule in rules {
            let (acct, cal) = (&rule.from.account, &rule.from.calendar);
            match api.list_events(acct, cal, now, time_max) {
                Ok(events) => {
                    report.succeeded.push(format!("{acct}/{cal}"));
                    deletable.insert((acct.clone(), cal.clone()));
                    for ev in events.into_iter().filter(|e| !e.is_copy()) {
                        let Some(key) = ev.copy_key() else { continue };
                        seen.insert(key.clone());
                        // A remembered delete lives as long as its source event, wherever it moves
                        if let (Some(end), Some((_, src_end))) = (
                            state.tombstones.get_mut(&key),
                            ev.bounds(cfg.work_hours.timezone),
                        ) {
                            *end = src_end;
                        }
                        if is_eligible(&ev, &cfg.work_hours, now, cfg.horizon_days) {
                            desired.push(Desired {
                                key,
                                source: ev,
                                source_account: acct.clone(),
                                source_calendar: cal.clone(),
                            });
                        }
                    }
                }
                Err(e) => {
                    report.all_reads_ok = false;
                    report.failures.push((format!("{acct}/{cal}"), e));
                }
            }
        }

        let copies = match api.list_copies(&dest_account, &dest_calendar, now, time_max) {
            Ok(c) => {
                report
                    .succeeded
                    .push(format!("{dest_account}/{dest_calendar}"));
                c
            }
            Err(e) => {
                report.all_reads_ok = false;
                report
                    .failures
                    .push((format!("{dest_account}/{dest_calendar}"), e));
                continue;
            }
        };

        detect_manual_deletes(state, &dest_account, &dest_calendar, &copies, now);
        remember_copies(state, &dest_account, &dest_calendar, &copies);

        let existing: Vec<Existing> = copies
            .into_iter()
            .map(|event| Existing {
                dest_event_id: event.id.clone(),
                event,
            })
            .collect();
        let tombstones: BTreeSet<CopyKey> = state.tombstones.keys().cloned().collect();
        let actions = plan(desired, &existing, &deletable, &tombstones, &cfg.label, now);

        if !dry_run {
            for action in &actions {
                apply(
                    cfg,
                    state,
                    api,
                    &dest_account,
                    &dest_calendar,
                    action,
                    &mut report,
                );
            }
        }
        report.actions.extend(actions);
    }

    prune_tombstones(&mut state.tombstones, &seen, report.all_reads_ok, now);
    report
}

/// Every future copy on every destination, as (account, calendar, copy)
pub fn cleanup_actions(
    cfg: &Config,
    api: &mut dyn CalendarApi,
    now: DateTime<Utc>,
) -> Result<Vec<(String, String, GEvent)>, ApiError> {
    let dests: BTreeSet<(String, String)> = cfg
        .sync
        .iter()
        .map(|r| (r.to.account.clone(), r.to.calendar.clone()))
        .collect();
    // Look past the horizon in case it was shortened since the copies were made
    let until = now + chrono::Duration::days(i64::from(cfg.horizon_days).max(365));
    let mut items = Vec::new();
    for (account, calendar) in dests {
        for copy in api.list_copies(&account, &calendar, now, until)? {
            if copy.is_copy() && approx_end(&copy.end).is_some_and(|end| end > now) {
                items.push((account.clone(), calendar.clone(), copy));
            }
        }
    }
    Ok(items)
}

fn detect_manual_deletes(
    state: &mut State,
    account: &str,
    calendar: &str,
    copies: &[GEvent],
    now: DateTime<Utc>,
) {
    let present: BTreeSet<&str> = copies.iter().map(|c| c.id.as_str()).collect();
    let gone: Vec<String> = state
        .known_copies
        .iter()
        .filter(|(id, k)| {
            k.dest_account == account
                && k.dest_calendar == calendar
                && k.end > now
                && !present.contains(id.as_str())
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in gone {
        if let Some(k) = state.known_copies.remove(&id) {
            state.tombstones.insert(k.key, k.end);
        }
    }
}

fn remember_copies(state: &mut State, account: &str, calendar: &str, copies: &[GEvent]) {
    for c in copies {
        if let (Some(key), Some(end)) = (c.copy_key(), approx_end(&c.end)) {
            state
                .known_copies
                .insert(c.id.clone(), known(key, end, account, calendar));
        }
    }
}

fn known(key: CopyKey, end: DateTime<Utc>, account: &str, calendar: &str) -> KnownCopy {
    KnownCopy {
        key,
        end,
        dest_account: account.to_string(),
        dest_calendar: calendar.to_string(),
    }
}

fn apply(
    cfg: &Config,
    state: &mut State,
    api: &mut dyn CalendarApi,
    account: &str,
    calendar: &str,
    action: &Action,
    report: &mut PassReport,
) {
    let target = format!("{account}/{calendar}");
    match action {
        Action::Create { desired: d } | Action::Patch { desired: d, .. } => {
            let body = copy_body(&d.source, &d.source_account, &d.source_calendar, &cfg.label);
            let result = match action {
                Action::Patch { dest_event_id, .. } => {
                    api.patch(account, calendar, dest_event_id, &body)
                }
                _ => api.insert(account, calendar, &body),
            };
            match result {
                Ok(ev) => {
                    if let Some(end) = approx_end(&ev.end) {
                        state
                            .known_copies
                            .insert(ev.id.clone(), known(d.key.clone(), end, account, calendar));
                    }
                    if matches!(action, Action::Patch { .. }) {
                        report.counts.patched += 1;
                    } else {
                        report.counts.created += 1;
                    }
                }
                Err(e) => report.failures.push((target, e)),
            }
        }
        Action::Delete { dest_event_id, .. } => {
            match api.delete(account, calendar, dest_event_id) {
                Ok(()) => {
                    state.known_copies.remove(dest_event_id);
                    report.counts.deleted += 1;
                }
                Err(e) => report.failures.push((target, e)),
            }
        }
    }
}

#[cfg(test)]
pub mod fake {
    use crate::api::{ApiError, CalendarApi, CalendarListEntry};
    use crate::event::GEvent;
    use chrono::{DateTime, Utc};
    use std::collections::HashMap;

    #[derive(Default)]
    pub struct FakeApi {
        pub sources: HashMap<(String, String), Vec<GEvent>>,
        pub copies: HashMap<(String, String), Vec<GEvent>>,
        pub calendars: HashMap<String, Vec<CalendarListEntry>>,
        pub fail_list_events: Option<ApiError>,
        pub fail_list_copies: Option<ApiError>,
        pub next_id: u32,
    }

    fn k(a: &str, c: &str) -> (String, String) {
        (a.to_string(), c.to_string())
    }

    impl CalendarApi for FakeApi {
        fn list_events(
            &mut self,
            a: &str,
            c: &str,
            _: DateTime<Utc>,
            _: DateTime<Utc>,
        ) -> Result<Vec<GEvent>, ApiError> {
            if let Some(e) = &self.fail_list_events {
                return Err(e.clone());
            }
            Ok(self.sources.get(&k(a, c)).cloned().unwrap_or_default())
        }
        fn list_copies(
            &mut self,
            a: &str,
            c: &str,
            _: DateTime<Utc>,
            _: DateTime<Utc>,
        ) -> Result<Vec<GEvent>, ApiError> {
            if let Some(e) = &self.fail_list_copies {
                return Err(e.clone());
            }
            Ok(self.copies.get(&k(a, c)).cloned().unwrap_or_default())
        }
        fn insert(
            &mut self,
            a: &str,
            c: &str,
            body: &serde_json::Value,
        ) -> Result<GEvent, ApiError> {
            self.next_id += 1;
            let mut body = body.clone();
            body["id"] = serde_json::json!(format!("copy-{}", self.next_id));
            let ev: GEvent = serde_json::from_value(body).unwrap();
            self.copies.entry(k(a, c)).or_default().push(ev.clone());
            Ok(ev)
        }
        fn patch(
            &mut self,
            a: &str,
            c: &str,
            id: &str,
            body: &serde_json::Value,
        ) -> Result<GEvent, ApiError> {
            let mut body = body.clone();
            body["id"] = serde_json::json!(id);
            let ev: GEvent = serde_json::from_value(body).unwrap();
            let list = self.copies.entry(k(a, c)).or_default();
            list.retain(|e| e.id != id);
            list.push(ev.clone());
            Ok(ev)
        }
        fn delete(&mut self, a: &str, c: &str, id: &str) -> Result<(), ApiError> {
            self.copies
                .entry(k(a, c))
                .or_default()
                .retain(|e| e.id != id);
            Ok(())
        }
        fn list_calendars(&mut self, a: &str) -> Result<Vec<CalendarListEntry>, ApiError> {
            Ok(self.calendars.get(a).cloned().unwrap_or_default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeApi;
    use super::*;
    use crate::api::ApiError;
    use crate::state::State;
    use serde_json::json;

    const CONFIG: &str = r#"
oauth_client = "/tmp/client.json"
[work_hours]
timezone = "America/Chicago"
days = ["mon", "tue", "wed", "thu", "fri"]
start = "08:00"
end = "17:00"
[accounts.personal]
[accounts.work]
[[sync]]
from = { account = "personal", calendar = "appts" }
to = { account = "work", calendar = "primary" }
"#;

    fn cfg() -> Config {
        crate::config::parse(CONFIG).unwrap()
    }

    fn now() -> DateTime<Utc> {
        "2026-11-09T12:00:00Z".parse().unwrap()
    }

    fn ev(id: &str, start: &str, end: &str, series: Option<&str>) -> GEvent {
        let mut v = json!({
            "id": id, "status": "confirmed", "summary": "Secret",
            "start": {"dateTime": start}, "end": {"dateTime": end},
            "iCalUID": format!("{}@google.com", series.unwrap_or(id)),
        });
        if let Some(s) = series {
            v["recurringEventId"] = json!(s);
            v["originalStartTime"] = json!({"dateTime": start});
        }
        serde_json::from_value(v).unwrap()
    }

    fn api_with(events: Vec<GEvent>) -> FakeApi {
        let mut api = FakeApi::default();
        api.sources
            .insert(("personal".into(), "appts".into()), events);
        api
    }

    fn dest(api: &FakeApi) -> Vec<GEvent> {
        api.copies
            .get(&("work".to_string(), "primary".to_string()))
            .cloned()
            .unwrap_or_default()
    }

    fn dentist() -> GEvent {
        ev(
            "dentist",
            "2026-11-09T16:00:00Z",
            "2026-11-09T17:00:00Z",
            None,
        )
    }

    #[test]
    fn creates_copy_for_eligible_event() {
        let mut api = api_with(vec![dentist()]);
        let mut state = State::default();
        let report = run_pass(&cfg(), &mut state, &mut api, now(), false);
        assert_eq!(report.counts.created, 1);
        assert!(report.all_reads_ok);
        let copies = dest(&api);
        assert_eq!(copies.len(), 1);
        assert_eq!(copies[0].summary.as_deref(), Some("Personal Commitment"));
        assert_eq!(copies[0].tag("cab_source_event"), Some("dentist"));
        assert!(state.known_copies.contains_key(&copies[0].id));
    }

    #[test]
    fn failed_source_read_deletes_nothing() {
        let mut api = api_with(vec![dentist()]);
        let mut state = State::default();
        run_pass(&cfg(), &mut state, &mut api, now(), false);
        api.fail_list_events = Some(ApiError::Network("offline".into()));
        let report = run_pass(&cfg(), &mut state, &mut api, now(), false);
        assert_eq!(report.counts.deleted, 0);
        assert!(!report.all_reads_ok);
        assert_eq!(dest(&api).len(), 1);
    }

    #[test]
    fn failed_destination_read_skips_destination() {
        let mut api = api_with(vec![dentist()]);
        api.fail_list_copies = Some(ApiError::Network("offline".into()));
        let mut state = State::default();
        let report = run_pass(&cfg(), &mut state, &mut api, now(), false);
        assert!(report.actions.is_empty());
        assert!(dest(&api).is_empty());
        assert!(!report.all_reads_ok);
    }

    #[test]
    fn user_deleted_copy_becomes_tombstone_and_is_not_recreated() {
        let mut api = api_with(vec![dentist()]);
        let mut state = State::default();
        run_pass(&cfg(), &mut state, &mut api, now(), false);
        api.copies.clear();
        let report = run_pass(&cfg(), &mut state, &mut api, now(), false);
        assert_eq!(report.counts.created, 0);
        assert!(dest(&api).is_empty());
        assert!(
            state
                .tombstones
                .contains_key(&dentist().copy_key().unwrap())
        );
    }

    #[test]
    fn deleted_series_removes_future_copies() {
        let series = vec![
            ev(
                "s_1",
                "2026-11-09T16:00:00Z",
                "2026-11-09T17:00:00Z",
                Some("s"),
            ),
            ev(
                "s_2",
                "2026-11-10T16:00:00Z",
                "2026-11-10T17:00:00Z",
                Some("s"),
            ),
            ev(
                "s_3",
                "2026-11-11T16:00:00Z",
                "2026-11-11T17:00:00Z",
                Some("s"),
            ),
        ];
        let mut api = api_with(series);
        let mut state = State::default();
        run_pass(&cfg(), &mut state, &mut api, now(), false);
        assert_eq!(dest(&api).len(), 3);
        api.sources
            .insert(("personal".into(), "appts".into()), vec![]);
        let report = run_pass(&cfg(), &mut state, &mut api, now(), false);
        assert_eq!(report.counts.deleted, 3);
        assert!(dest(&api).is_empty());
        assert!(state.tombstones.is_empty());
    }

    #[test]
    fn tombstone_follows_source_moved_later() {
        let mut api = api_with(vec![dentist()]);
        let mut state = State::default();
        run_pass(&cfg(), &mut state, &mut api, now(), false);
        api.copies.clear();
        let moved = ev(
            "dentist",
            "2026-11-12T16:00:00Z",
            "2026-11-12T17:00:00Z",
            None,
        );
        api.sources
            .insert(("personal".into(), "appts".into()), vec![moved]);
        run_pass(&cfg(), &mut state, &mut api, now(), false);
        let after_old_end: DateTime<Utc> = "2026-11-10T12:00:00Z".parse().unwrap();
        run_pass(&cfg(), &mut state, &mut api, after_old_end, false);
        let report = run_pass(&cfg(), &mut state, &mut api, after_old_end, false);
        assert_eq!(
            report.counts.created, 0,
            "deleted copy came back after its source moved"
        );
        assert!(dest(&api).is_empty());
    }

    #[test]
    fn dry_run_changes_nothing() {
        let mut api = api_with(vec![dentist()]);
        let mut state = State::default();
        let report = run_pass(&cfg(), &mut state, &mut api, now(), true);
        assert_eq!(report.actions.len(), 1);
        assert!(dest(&api).is_empty());
        assert_eq!(state, State::default());
    }
}

#[cfg(test)]
mod cleanup_tests {
    use super::fake::FakeApi;
    use super::*;
    use serde_json::json;

    #[test]
    fn cleanup_lists_only_future_copies() {
        let cfg = crate::config::parse(
            r#"
oauth_client = "/tmp/c.json"
[work_hours]
timezone = "America/Chicago"
days = ["mon"]
start = "08:00"
end = "17:00"
[accounts.personal]
[accounts.work]
[[sync]]
from = { account = "personal", calendar = "appts" }
to = { account = "work", calendar = "primary" }
"#,
        )
        .unwrap();
        let copy = |id: &str, start: &str, end: &str| -> GEvent {
            serde_json::from_value(json!({
                "id": id, "summary": "Personal Commitment",
                "start": {"dateTime": start}, "end": {"dateTime": end},
                "extendedProperties": {"private": {"cab_managed": "1", "cab_ical_uid": id}},
            }))
            .unwrap()
        };
        let mut api = FakeApi::default();
        api.copies.insert(
            ("work".into(), "primary".into()),
            vec![
                copy("past", "2026-11-09T08:00:00Z", "2026-11-09T09:00:00Z"),
                copy("future", "2026-11-10T16:00:00Z", "2026-11-10T17:00:00Z"),
            ],
        );
        let now: DateTime<Utc> = "2026-11-09T12:00:00Z".parse().unwrap();
        let items = cleanup_actions(&cfg, &mut api, now).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].2.id, "future");
    }
}

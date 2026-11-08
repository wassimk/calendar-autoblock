use crate::event::{CopyKey, EventTime, GEvent};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq)]
pub struct Desired {
    pub key: CopyKey,
    pub source: GEvent,
    pub source_account: String,
    pub source_calendar: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Create {
        desired: Desired,
    },
    Patch {
        dest_event_id: String,
        desired: Desired,
    },
    Delete {
        dest_event_id: String,
        reason: DeleteReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteReason {
    SourceGone,
    Duplicate,
}

pub fn plan(
    desired: Vec<Desired>,
    existing: &[GEvent],
    deletable_sources: &BTreeSet<(String, String)>,
    tombstones: &BTreeSet<CopyKey>,
    label: &str,
    now: DateTime<Utc>,
    tz: Tz,
) -> Vec<Action> {
    let mut wanted: BTreeMap<CopyKey, Desired> = BTreeMap::new();
    for d in desired {
        wanted.entry(d.key.clone()).or_insert(d);
    }

    let mut actions = Vec::new();
    let mut matched: BTreeSet<CopyKey> = BTreeSet::new();
    for ex in existing {
        if ex.end.to_utc(tz).is_none_or(|end| end <= now) {
            continue;
        }
        let Some(key) = ex.copy_key() else {
            continue;
        };
        if matched.contains(&key) {
            actions.push(Action::Delete {
                dest_event_id: ex.id.clone(),
                reason: DeleteReason::Duplicate,
            });
            continue;
        }
        match wanted.get(&key) {
            Some(d) => {
                matched.insert(key);
                if !matches_source(ex, &d.source, label) {
                    actions.push(Action::Patch {
                        dest_event_id: ex.id.clone(),
                        desired: d.clone(),
                    });
                }
            }
            None => {
                let origin = (
                    ex.tag("cab_source_account").unwrap_or_default().to_string(),
                    ex.tag("cab_source_calendar")
                        .unwrap_or_default()
                        .to_string(),
                );
                if deletable_sources.contains(&origin) {
                    actions.push(Action::Delete {
                        dest_event_id: ex.id.clone(),
                        reason: DeleteReason::SourceGone,
                    });
                }
            }
        }
    }

    for (key, d) in wanted {
        if !matched.contains(&key) && !tombstones.contains(&key) {
            actions.push(Action::Create { desired: d });
        }
    }
    actions
}

/// `seen` maps every source event read this pass to its end. A remembered delete follows
/// its source event's end, wherever it moves, and is forgotten once that end passes.
pub fn prune_tombstones(
    tombstones: &mut BTreeMap<CopyKey, DateTime<Utc>>,
    seen: &BTreeMap<CopyKey, DateTime<Utc>>,
    read_sources_ok: bool,
    now: DateTime<Utc>,
) {
    for (key, end) in tombstones.iter_mut() {
        if let Some(src_end) = seen.get(key) {
            *end = *src_end;
        }
    }
    tombstones.retain(|key, end| *end > now && (!read_sources_ok || seen.contains_key(key)));
}

fn matches_source(copy: &GEvent, source: &GEvent, label: &str) -> bool {
    copy.summary.as_deref() == Some(label)
        && copy.visibility.as_deref() == Some("public")
        // Google omits transparency when it is the default, opaque
        && matches!(copy.transparency.as_deref(), None | Some("opaque"))
        && same_time(&copy.start, &source.start)
        && same_time(&copy.end, &source.end)
}

fn same_time(a: &EventTime, b: &EventTime) -> bool {
    match (a.date_time, b.date_time) {
        (Some(x), Some(y)) => x == y,
        (None, None) => a.date == b.date,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::copy_body;
    use serde_json::json;

    const LABEL: &str = "Personal Commitment";

    fn now() -> DateTime<Utc> {
        "2026-11-09T12:00:00Z".parse().unwrap()
    }

    fn source(id: &str, start: &str, end: &str) -> GEvent {
        serde_json::from_value(json!({
            "id": id, "status": "confirmed", "summary": "Secret title",
            "start": {"dateTime": start}, "end": {"dateTime": end},
            "iCalUID": format!("{id}@google.com"),
        }))
        .unwrap()
    }

    fn desired(ev: GEvent, calendar: &str) -> Desired {
        Desired {
            key: ev.copy_key().unwrap(),
            source: ev,
            source_account: "personal".into(),
            source_calendar: calendar.into(),
        }
    }

    fn existing_for(dest_id: &str, ev: &GEvent, calendar: &str) -> GEvent {
        let mut body = copy_body(ev, "personal", calendar, LABEL);
        body["id"] = json!(dest_id);
        serde_json::from_value(body).unwrap()
    }

    fn ok_sources() -> BTreeSet<(String, String)> {
        [
            ("personal".to_string(), "appts".to_string()),
            ("personal".to_string(), "family".to_string()),
        ]
        .into_iter()
        .collect()
    }

    fn run(
        desired: Vec<Desired>,
        existing: &[GEvent],
        tombstones: &BTreeSet<CopyKey>,
    ) -> Vec<Action> {
        plan(
            desired,
            existing,
            &ok_sources(),
            tombstones,
            LABEL,
            now(),
            chrono_tz::America::Chicago,
        )
    }

    fn dentist() -> GEvent {
        source("dentist", "2026-11-09T16:00:00Z", "2026-11-09T17:00:00Z")
    }

    #[test]
    fn new_desired_creates() {
        let actions = run(vec![desired(dentist(), "appts")], &[], &BTreeSet::new());
        assert!(matches!(actions.as_slice(), [Action::Create { .. }]));
    }

    #[test]
    fn matching_copy_is_left_alone() {
        let ex = existing_for("c1", &dentist(), "appts");
        assert!(run(vec![desired(dentist(), "appts")], &[ex], &BTreeSet::new()).is_empty());
    }

    #[test]
    fn moved_source_patches_copy() {
        let ex = existing_for("c1", &dentist(), "appts");
        let moved = source("dentist", "2026-11-09T18:00:00Z", "2026-11-09T19:00:00Z");
        let actions = run(vec![desired(moved, "appts")], &[ex], &BTreeSet::new());
        assert!(
            matches!(actions.as_slice(), [Action::Patch { dest_event_id, .. }] if dest_event_id == "c1")
        );
    }

    #[test]
    fn edited_copy_snaps_back() {
        let mut ex = existing_for("c1", &dentist(), "appts");
        ex.summary = Some("Lunch".into());
        let actions = run(vec![desired(dentist(), "appts")], &[ex], &BTreeSet::new());
        assert!(matches!(actions.as_slice(), [Action::Patch { .. }]));
    }

    #[test]
    fn source_gone_deletes_future_copy() {
        let ex = existing_for("c1", &dentist(), "appts");
        let actions = run(vec![], &[ex], &BTreeSet::new());
        assert!(matches!(
            actions.as_slice(),
            [Action::Delete { dest_event_id, reason: DeleteReason::SourceGone, .. }] if dest_event_id == "c1"
        ));
    }

    #[test]
    fn ended_copy_is_never_deleted() {
        let past = source("old", "2026-11-09T09:00:00Z", "2026-11-09T10:00:00Z");
        let ex = existing_for("c1", &past, "appts");
        assert!(run(vec![], &[ex], &BTreeSet::new()).is_empty());
    }

    #[test]
    fn tombstoned_key_is_not_created() {
        let tomb: BTreeSet<CopyKey> = [dentist().copy_key().unwrap()].into_iter().collect();
        assert!(run(vec![desired(dentist(), "appts")], &[], &tomb).is_empty());
    }

    #[test]
    fn tombstone_survives_source_move() {
        let tomb: BTreeSet<CopyKey> = [dentist().copy_key().unwrap()].into_iter().collect();
        let mut moved = source("dentist", "2026-11-10T16:00:00Z", "2026-11-10T17:00:00Z");
        moved.summary = Some("Renamed".into());
        assert!(run(vec![desired(moved, "appts")], &[], &tomb).is_empty());
    }

    #[test]
    fn same_event_on_two_calendars_creates_one_copy() {
        let actions = run(
            vec![desired(dentist(), "appts"), desired(dentist(), "family")],
            &[],
            &BTreeSet::new(),
        );
        assert_eq!(actions.len(), 1);
    }

    #[test]
    fn duplicate_copies_delete_extras() {
        let a = existing_for("c1", &dentist(), "appts");
        let b = existing_for("c2", &dentist(), "appts");
        let actions = run(vec![desired(dentist(), "appts")], &[a, b], &BTreeSet::new());
        assert!(matches!(
            actions.as_slice(),
            [Action::Delete { dest_event_id, reason: DeleteReason::Duplicate, .. }] if dest_event_id == "c2"
        ));
    }

    #[test]
    fn copy_from_failed_source_is_not_deleted() {
        let ex = existing_for("c1", &dentist(), "appts");
        let only_family: BTreeSet<(String, String)> =
            [("personal".to_string(), "family".to_string())]
                .into_iter()
                .collect();
        assert!(
            plan(
                vec![],
                &[ex],
                &only_family,
                &BTreeSet::new(),
                LABEL,
                now(),
                chrono_tz::America::Chicago
            )
            .is_empty()
        );
    }

    #[test]
    fn prune_drops_ended_and_unseen_tombstones() {
        let ended = CopyKey {
            ical_uid: "ended".into(),
            original_start: None,
        };
        let unseen = CopyKey {
            ical_uid: "unseen".into(),
            original_start: None,
        };
        let kept = CopyKey {
            ical_uid: "kept".into(),
            original_start: None,
        };
        let later: DateTime<Utc> = "2026-11-20T00:00:00Z".parse().unwrap();
        let mut t: BTreeMap<CopyKey, DateTime<Utc>> = [
            (ended.clone(), "2026-11-01T00:00:00Z".parse().unwrap()),
            (unseen.clone(), later),
            (kept.clone(), later),
        ]
        .into_iter()
        .collect();
        let seen: BTreeMap<CopyKey, DateTime<Utc>> = [(kept.clone(), later)].into_iter().collect();
        prune_tombstones(&mut t, &seen, true, now());
        assert_eq!(t.keys().cloned().collect::<Vec<_>>(), vec![kept]);
    }

    #[test]
    fn prune_keeps_unseen_when_reads_failed() {
        let unseen = CopyKey {
            ical_uid: "unseen".into(),
            original_start: None,
        };
        let mut t: BTreeMap<CopyKey, DateTime<Utc>> =
            [(unseen.clone(), "2026-11-20T00:00:00Z".parse().unwrap())]
                .into_iter()
                .collect();
        prune_tombstones(&mut t, &BTreeMap::new(), false, now());
        assert!(t.contains_key(&unseen));
    }
}

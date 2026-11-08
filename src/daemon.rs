use crate::api::ApiError;
use crate::config::{self, Config};
use crate::google::GoogleCalendar;
use crate::oauth::{self, TokenStore};
use crate::state::{self, State, Status};
use crate::sync::{PassReport, run_pass};
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn state_path() -> PathBuf {
    state::state_dir().join("state.json")
}

pub fn status_path() -> PathBuf {
    state::state_dir().join("status.json")
}

pub fn build_api(cfg: &Config) -> Result<GoogleCalendar<TokenStore>> {
    let client = oauth::load_client(&cfg.oauth_client)?;
    Ok(GoogleCalendar::new(TokenStore::new(client)))
}

pub fn update_status(status: &mut Status, report: &PassReport, now: DateTime<Utc>) {
    status.last_pass = Some(now);
    status.last_counts = report.counts;
    for key in &report.succeeded {
        status.last_success.insert(key.clone(), now);
        status.calendar_errors.remove(key);
        if let Some(account) = key.split('/').next() {
            status.auth_errors.remove(account);
        }
    }
    let mut network_failed = false;
    status.last_error = None;
    for (key, err) in &report.failures {
        match err {
            ApiError::Network(_) => network_failed = true,
            ApiError::Auth { account, message } => {
                status.auth_errors.insert(
                    account.clone(),
                    format!("{message}; run: calendar-autoblock auth {account}"),
                );
            }
            ApiError::NotOwned(_) => {
                let account = key.split('/').next().unwrap_or(key);
                status.calendar_errors.insert(
                    key.clone(),
                    format!("not owned by {account}; this calendar can't be synced with the current scopes"),
                );
            }
            other => status.last_error = Some(format!("{key}: {other}")),
        }
    }
    if network_failed {
        status.offline_since.get_or_insert(now);
    } else {
        status.offline_since = None;
    }
}

/// One pass against Google, saving state and status. Returns the report and the updated status.
pub fn pass_once(cfg: &Config, dry_run: bool) -> Result<(PassReport, Status)> {
    let mut api = build_api(cfg)?;
    let (mut state, state_note): (State, _) = state::load_json_or_reset(&state_path())?;
    let now = Utc::now();
    let report = run_pass(cfg, &mut state, &mut api, now, dry_run);
    let (mut status, status_note): (Status, _) = state::load_json_or_reset(&status_path())?;
    if !dry_run {
        state::save_json(&state_path(), &state)?;
        update_status(&mut status, &report, now);
        if let Some(note) = state_note.or(status_note) {
            status.last_error = Some(note);
        }
        state::save_json(&status_path(), &status)?;
    }
    Ok((report, status))
}

pub fn run_daemon(cfg_path: &Path, verbose: bool) -> Result<()> {
    log("starting");
    let mut previous = Status::default();
    loop {
        let sleep = match config::load(cfg_path) {
            Ok(cfg) => {
                match pass_once(&cfg, false) {
                    Ok((report, status)) => {
                        log_changes(&previous, &status, &report, verbose);
                        previous = status;
                    }
                    Err(e) => log(&format!("pass failed: {e:#}")),
                }
                cfg.poll_seconds
            }
            Err(e) => {
                log(&format!("config error: {e:#}"));
                60
            }
        };
        std::thread::sleep(Duration::from_secs(sleep));
    }
}

fn log_changes(before: &Status, after: &Status, report: &PassReport, verbose: bool) {
    match (before.offline_since, after.offline_since) {
        (None, Some(_)) => {
            let why = report
                .failures
                .iter()
                .find(|(_, e)| matches!(e, ApiError::Network(_)))
                .map(|(_, e)| e.to_string())
                .unwrap_or_default();
            log(&format!("offline: {why}"));
        }
        (Some(_), None) => log("back online"),
        _ => {}
    }
    for (account, msg) in &after.auth_errors {
        if before.auth_errors.get(account) != Some(msg) {
            log(&format!("auth error for {account}: {msg}"));
        }
    }
    for (cal, msg) in &after.calendar_errors {
        if before.calendar_errors.get(cal) != Some(msg) {
            log(&format!("{cal}: {msg}"));
        }
    }
    if let Some(e) = &after.last_error {
        log(&format!("error: {e}"));
    }
    let c = report.counts;
    if verbose || c.created + c.patched + c.deleted > 0 {
        log(&format!(
            "pass: {} created, {} patched, {} deleted",
            c.created, c.patched, c.deleted
        ));
    }
}

fn log(msg: &str) {
    println!("{} {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ApiError;
    use crate::sync::PassReport;

    fn t(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn report(failures: Vec<(&str, ApiError)>, succeeded: Vec<&str>) -> PassReport {
        PassReport {
            all_reads_ok: failures.is_empty(),
            failures: failures
                .into_iter()
                .map(|(k, e)| (k.to_string(), e))
                .collect(),
            succeeded: succeeded.into_iter().map(String::from).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn update_status_sets_offline_once() {
        let mut s = Status::default();
        let offline = || {
            report(
                vec![("personal/appts", ApiError::Network("dns".into()))],
                vec![],
            )
        };
        update_status(&mut s, &offline(), t("2026-11-09T12:00:00Z"));
        update_status(&mut s, &offline(), t("2026-11-09T12:05:00Z"));
        assert_eq!(s.offline_since, Some(t("2026-11-09T12:00:00Z")));
        assert_eq!(s.last_pass, Some(t("2026-11-09T12:05:00Z")));
    }

    #[test]
    fn update_status_clears_offline_on_success() {
        let mut s = Status {
            offline_since: Some(t("2026-11-09T12:00:00Z")),
            ..Default::default()
        };
        update_status(
            &mut s,
            &report(vec![], vec!["personal/appts", "work/primary"]),
            t("2026-11-09T12:10:00Z"),
        );
        assert_eq!(s.offline_since, None);
        assert_eq!(
            s.last_success.get("personal/appts"),
            Some(&t("2026-11-09T12:10:00Z"))
        );
    }

    #[test]
    fn update_status_records_auth_fix_command() {
        let mut s = Status::default();
        let err = ApiError::Auth {
            account: "work".into(),
            message: "invalid_grant: revoked".into(),
        };
        update_status(
            &mut s,
            &report(vec![("work/primary", err)], vec![]),
            t("2026-11-09T12:00:00Z"),
        );
        assert!(s.auth_errors["work"].contains("run: calendar-autoblock auth work"));
        update_status(
            &mut s,
            &report(vec![], vec!["work/primary"]),
            t("2026-11-09T12:05:00Z"),
        );
        assert!(s.auth_errors.is_empty());
    }

    #[test]
    fn update_status_reports_not_owned_calendar() {
        let mut s = Status::default();
        let err = ApiError::NotOwned("forbidden".into());
        update_status(
            &mut s,
            &report(vec![("personal/family", err)], vec![]),
            t("2026-11-09T12:00:00Z"),
        );
        assert_eq!(
            s.calendar_errors["personal/family"],
            "not owned by personal; this calendar can't be synced with the current scopes"
        );
    }
}

use crate::api::{CalendarApi, CalendarListEntry};
use crate::config::{
    self, AccountConfig, CalendarRef, Config, DEST_SCOPES, SOURCE_SCOPES, SyncRule, WorkHours,
};
use crate::daemon;
use crate::keychain;
use crate::oauth::{self, ClientSecrets};
use anyhow::{Context, Result, anyhow, bail};
use std::collections::BTreeMap;
use std::fmt;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, PartialEq)]
pub enum MatchError {
    NotFound,
    Ambiguous(Vec<String>),
    NotOwned(String),
}

impl fmt::Display for MatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MatchError::NotFound => write!(f, "no calendar with that name"),
            MatchError::Ambiguous(ids) => write!(
                f,
                "more than one calendar has that name; use an id: {}",
                ids.join(", ")
            ),
            MatchError::NotOwned(name) => write!(
                f,
                "{name} isn't owned by this account, so it can't be read with the current scopes"
            ),
        }
    }
}

impl std::error::Error for MatchError {}

pub fn match_calendar<'a>(
    entries: &'a [CalendarListEntry],
    query: &str,
) -> Result<&'a CalendarListEntry, MatchError> {
    let found = match entries.iter().find(|e| e.id == query) {
        Some(e) => e,
        None => {
            let named: Vec<&CalendarListEntry> = entries
                .iter()
                .filter(|e| e.summary.eq_ignore_ascii_case(query))
                .collect();
            match named.as_slice() {
                [] => return Err(MatchError::NotFound),
                [one] => *one,
                many => {
                    return Err(MatchError::Ambiguous(
                        many.iter().map(|e| e.id.clone()).collect(),
                    ));
                }
            }
        }
    };
    if found.access_role != "owner" {
        return Err(MatchError::NotOwned(found.summary.clone()));
    }
    Ok(found)
}

pub fn sign_in(
    client: &ClientSecrets,
    account: &str,
    scopes: &[&str],
    hint: Option<&str>,
) -> Result<()> {
    let grant = oauth::run_loopback_flow(client, scopes, hint)?;
    let missing = oauth::missing_scopes(&grant.scope, scopes);
    if !missing.is_empty() {
        bail!(
            "Google didn't grant: {}. Run auth again and leave every box checked.",
            missing.join(", ")
        );
    }
    keychain::save_refresh_token(account, &grant.refresh_token)?;
    println!("Signed in {account}");
    Ok(())
}

fn prompt(question: &str, default: &str) -> Result<String> {
    if default.is_empty() {
        print!("{question}: ");
    } else {
        print!("{question} [{default}]: ");
    }
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    let answer = answer.trim();
    Ok(if answer.is_empty() {
        default.to_string()
    } else {
        answer.to_string()
    })
}

fn source_account(cfg: &Config) -> Result<String> {
    cfg.sync
        .first()
        .map(|r| r.from.account.clone())
        .or_else(|| cfg.accounts.keys().next().cloned())
        .ok_or_else(|| anyhow!("no accounts in config; run calendar-autoblock setup"))
}

fn destination(cfg: &Config) -> Result<CalendarRef> {
    cfg.sync
        .first()
        .map(|r| CalendarRef {
            name: None,
            ..r.to.clone()
        })
        .ok_or_else(|| anyhow!("no sync rules in config; run calendar-autoblock setup"))
}

fn print_calendars(entries: &[CalendarListEntry], synced: &[String], numbered: bool) {
    for (i, e) in entries.iter().enumerate() {
        let mark = if synced.contains(&e.id) { "[x]" } else { "[ ]" };
        let note = if e.access_role == "owner" {
            ""
        } else {
            "  (not owned, unsupported)"
        };
        let num = if numbered {
            format!("{:>2}. ", i + 1)
        } else {
            String::new()
        };
        println!("{num}{mark} {}{note}", e.summary);
    }
}

pub fn calendars_list() -> Result<()> {
    let cfg = config::load(&config::config_path())?;
    let account = source_account(&cfg)?;
    let mut api = daemon::build_api(&cfg)?;
    let entries = api.list_calendars(&account)?;
    let synced: Vec<String> = cfg.sync.iter().map(|r| r.from.calendar.clone()).collect();
    println!("Calendars on {account}:");
    print_calendars(&entries, &synced, false);
    Ok(())
}

pub fn calendars_add(name: &str) -> Result<()> {
    let path = config::config_path();
    let mut cfg = config::load(&path)?;
    let account = source_account(&cfg)?;
    let dest = destination(&cfg)?;
    let mut api = daemon::build_api(&cfg)?;
    let entries = api.list_calendars(&account)?;
    let cal = match_calendar(&entries, name)?;
    if cfg
        .sync
        .iter()
        .any(|r| r.from.account == account && r.from.calendar == cal.id)
    {
        println!("{} is already synced.", cal.summary);
        return Ok(());
    }
    cfg.sync.push(SyncRule {
        from: CalendarRef {
            account,
            calendar: cal.id.clone(),
            name: Some(cal.summary.clone()),
        },
        to: dest,
    });
    config::save(&cfg, &path)?;
    println!(
        "Now syncing {}. The next pass will add its blocks.",
        cal.summary
    );
    Ok(())
}

pub fn calendars_remove(name: &str, yes: bool) -> Result<()> {
    let path = config::config_path();
    let mut cfg = config::load(&path)?;
    let pos = cfg
        .sync
        .iter()
        .position(|r| {
            r.from.calendar == name
                || r.from
                    .name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .ok_or_else(|| anyhow!("{name} isn't synced; see calendar-autoblock calendars list"))?;
    let rule = cfg.sync.remove(pos);
    let label = rule
        .from
        .name
        .clone()
        .unwrap_or_else(|| rule.from.calendar.clone());

    let mut api = daemon::build_api(&cfg)?;
    let now = chrono::Utc::now();
    let until = now + chrono::Duration::days(365);
    let copies: Vec<_> = api
        .list_copies(&rule.to.account, &rule.to.calendar, now, until)?
        .into_iter()
        .filter(|c| c.tag("cab_source_calendar") == Some(rule.from.calendar.as_str()))
        .collect();
    config::save(&cfg, &path)?;
    println!("Stopped syncing {label}.");
    if copies.is_empty() {
        return Ok(());
    }
    let answer = if yes {
        "y".to_string()
    } else {
        prompt(
            &format!("Delete its {} future blocks? [y/N]", copies.len()),
            "n",
        )?
    };
    if answer.eq_ignore_ascii_case("y") {
        for c in &copies {
            api.delete(&rule.to.account, &rule.to.calendar, &c.id)?;
        }
        println!("Deleted {} blocks.", copies.len());
    }
    Ok(())
}

pub fn run_setup() -> Result<()> {
    let path = config::config_path();
    println!("calendar-autoblock setup. Press Enter to accept the default in brackets.\n");

    let default_client = config::config_path().with_file_name("client_secret.json");
    let client_path = PathBuf::from(prompt(
        "Path to your Google OAuth client JSON",
        &default_client.to_string_lossy(),
    )?);
    let client = oauth::load_client(&client_path)?;

    let src = prompt("Name for your personal (source) account", "personal")?;
    let src_email = prompt(&format!("Email for {src}"), "")?;
    let dst = prompt("Name for your work (destination) account", "work")?;
    let dst_email = prompt(&format!("Email for {dst}"), "")?;

    println!(
        "\nSigning in to {src}. Choose {} in the browser.",
        if src_email.is_empty() {
            "that account"
        } else {
            &src_email
        }
    );
    sign_in(
        &client,
        &src,
        &SOURCE_SCOPES,
        Some(&src_email)
            .filter(|e| !e.is_empty())
            .map(String::as_str),
    )?;
    println!(
        "\nSigning in to {dst}. Choose {} in the browser.",
        if dst_email.is_empty() {
            "that account"
        } else {
            &dst_email
        }
    );
    sign_in(
        &client,
        &dst,
        &DEST_SCOPES,
        Some(&dst_email)
            .filter(|e| !e.is_empty())
            .map(String::as_str),
    )?;

    let tz = prompt("\nTime zone for your work hours", "America/Chicago")?;
    let days = prompt("Work days", "mon,tue,wed,thu,fri")?;
    let start = prompt("Work day starts", "08:00")?;
    let end = prompt("Work day ends", "17:00")?;
    let margin = prompt("Margin before and after work hours", "1h")?;

    let account = |email: &str| AccountConfig {
        email: Some(email.to_string()).filter(|e| !e.is_empty()),
    };
    let raw = format!(
        "oauth_client = {:?}\n[work_hours]\ntimezone = {tz:?}\ndays = [{}]\nstart = {start:?}\nend = {end:?}\nmargin_before = {margin:?}\nmargin_after = {margin:?}\n",
        client_path.to_string_lossy(),
        days.split(',')
            .map(|d| format!("{:?}", d.trim()))
            .collect::<Vec<_>>()
            .join(", "),
    );
    let parsed = config::parse(&raw).context("those work hours didn't parse")?;
    let mut cfg = Config {
        accounts: BTreeMap::from([
            (src.clone(), account(&src_email)),
            (dst.clone(), account(&dst_email)),
        ]),
        sync: Vec::new(),
        work_hours: WorkHours {
            ..parsed.work_hours
        },
        ..parsed
    };

    let mut api = daemon::build_api(&cfg)?;
    let entries = api.list_calendars(&src)?;
    println!("\nCalendars on {src}:");
    print_calendars(&entries, &[], true);
    let picks = prompt(
        "Which calendars should block your work time? Numbers, comma separated",
        "",
    )?;
    for pick in picks.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let n: usize = pick
            .parse()
            .with_context(|| format!("{pick:?} isn't a number"))?;
        let e = entries
            .get(n.wrapping_sub(1))
            .ok_or_else(|| anyhow!("no calendar number {n}"))?;
        if e.access_role != "owner" {
            println!("Skipping {}: not owned by {src}.", e.summary);
            continue;
        }
        cfg.sync.push(SyncRule {
            from: CalendarRef {
                account: src.clone(),
                calendar: e.id.clone(),
                name: Some(e.summary.clone()),
            },
            to: CalendarRef {
                account: dst.clone(),
                calendar: "primary".into(),
                name: None,
            },
        });
    }
    if cfg.sync.is_empty() {
        bail!("no calendars picked; nothing saved");
    }
    config::save(&cfg, &path)?;
    println!("\nSaved {}.", path.display());
    println!(
        "Next: `calendar-autoblock sync --dry-run` to preview, then `calendar-autoblock install`."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cal(id: &str, summary: &str, role: &str) -> CalendarListEntry {
        CalendarListEntry {
            id: id.into(),
            summary: summary.into(),
            access_role: role.into(),
            primary: false,
        }
    }

    fn entries() -> Vec<CalendarListEntry> {
        vec![
            cal("a1@group.calendar.google.com", "Appointments", "owner"),
            cal("f1@group.calendar.google.com", "Family", "writer"),
            cal("d1@group.calendar.google.com", "Dup", "owner"),
            cal("d2@group.calendar.google.com", "dup", "owner"),
        ]
    }

    #[test]
    fn match_is_case_insensitive() {
        let e = entries();
        assert_eq!(
            match_calendar(&e, "appointments").unwrap().id,
            "a1@group.calendar.google.com"
        );
    }

    #[test]
    fn match_reports_ambiguous_names() {
        let e = entries();
        match match_calendar(&e, "DUP") {
            Err(MatchError::Ambiguous(ids)) => assert_eq!(ids.len(), 2),
            other => panic!("expected ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn match_accepts_exact_id() {
        let e = entries();
        assert_eq!(
            match_calendar(&e, "d2@group.calendar.google.com")
                .unwrap()
                .summary,
            "dup"
        );
    }

    #[test]
    fn match_rejects_not_owned() {
        let e = entries();
        assert!(matches!(
            match_calendar(&e, "family"),
            Err(MatchError::NotOwned(_))
        ));
        assert!(matches!(
            match_calendar(&e, "nope"),
            Err(MatchError::NotFound)
        ));
    }
}

use anyhow::{Context, Result, anyhow, bail};
use chrono::{NaiveTime, Weekday};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const SOURCE_SCOPES: [&str; 2] = [
    "https://www.googleapis.com/auth/calendar.events.owned.readonly",
    "https://www.googleapis.com/auth/calendar.calendarlist.readonly",
];
pub const DEST_SCOPES: [&str; 1] = ["https://www.googleapis.com/auth/calendar.events.owned"];

pub const DEFAULT_LABEL: &str = "Personal Commitment";
pub const DEFAULT_HORIZON_DAYS: u32 = 90;
pub const DEFAULT_POLL_SECONDS: u64 = 300;
const DEFAULT_MARGIN: &str = "1h";

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub oauth_client: PathBuf,
    pub label: String,
    pub horizon_days: u32,
    pub poll_seconds: u64,
    pub work_hours: WorkHours,
    pub accounts: BTreeMap<String, AccountConfig>,
    pub sync: Vec<SyncRule>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkHours {
    pub timezone: chrono_tz::Tz,
    pub days: Vec<Weekday>,
    pub start: NaiveTime,
    pub end: NaiveTime,
    pub margin_before: chrono::Duration,
    pub margin_after: chrono::Duration,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AccountConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncRule {
    pub from: CalendarRef,
    pub to: CalendarRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalendarRef {
    pub account: String,
    pub calendar: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl CalendarRef {
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.calendar)
    }
}

#[derive(Serialize, Deserialize)]
struct RawConfig {
    oauth_client: String,
    label: Option<String>,
    horizon_days: Option<u32>,
    poll_seconds: Option<u64>,
    work_hours: RawWorkHours,
    #[serde(default)]
    accounts: BTreeMap<String, AccountConfig>,
    #[serde(default)]
    sync: Vec<SyncRule>,
}

#[derive(Serialize, Deserialize)]
struct RawWorkHours {
    timezone: String,
    days: Vec<String>,
    start: String,
    end: String,
    margin_before: Option<String>,
    margin_after: Option<String>,
}

/// Where the app keeps its files; CALENDAR_AUTOBLOCK_HOME moves them (and only them) for demos
pub fn home() -> PathBuf {
    home_from(std::env::var_os("CALENDAR_AUTOBLOCK_HOME"))
}

fn home_from(override_dir: Option<std::ffi::OsString>) -> PathBuf {
    override_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().expect("no home directory"))
}

pub fn config_path() -> PathBuf {
    home().join(".config/calendar-autoblock/config.toml")
}

pub fn load(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("couldn't read config at {}", path.display()))?;
    parse(&text).with_context(|| format!("invalid config at {}", path.display()))
}

pub fn save(cfg: &Config, path: &Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, toml::to_string_pretty(&to_raw(cfg))?)?;
    Ok(())
}

pub fn parse(text: &str) -> Result<Config> {
    let raw: RawConfig = toml::from_str(text)?;
    let wh = &raw.work_hours;
    let days: Vec<&str> = wh.days.iter().map(String::as_str).collect();
    let work_hours = work_hours_from(
        &wh.timezone,
        &days,
        &wh.start,
        &wh.end,
        wh.margin_before.as_deref().unwrap_or(DEFAULT_MARGIN),
        wh.margin_after.as_deref().unwrap_or(DEFAULT_MARGIN),
    )?;
    for rule in &raw.sync {
        for r in [&rule.from, &rule.to] {
            if !raw.accounts.contains_key(&r.account) {
                bail!("sync rule uses unknown account {:?}", r.account);
            }
        }
    }
    Ok(Config {
        oauth_client: expand_tilde(&raw.oauth_client),
        label: raw.label.unwrap_or_else(|| DEFAULT_LABEL.to_string()),
        horizon_days: raw.horizon_days.unwrap_or(DEFAULT_HORIZON_DAYS),
        poll_seconds: raw.poll_seconds.unwrap_or(DEFAULT_POLL_SECONDS),
        work_hours,
        accounts: raw.accounts,
        sync: raw.sync,
    })
}

pub fn work_hours_from(
    timezone: &str,
    days: &[&str],
    start: &str,
    end: &str,
    margin_before: &str,
    margin_after: &str,
) -> Result<WorkHours> {
    let timezone: chrono_tz::Tz = timezone
        .parse()
        .map_err(|_| anyhow!("unknown timezone {timezone:?}"))?;
    let days = days
        .iter()
        .map(|d| parse_day(d.trim()))
        .collect::<Result<Vec<_>>>()?;
    let (start_t, end_t) = (parse_time(start)?, parse_time(end)?);
    if end_t <= start_t {
        bail!("work_hours end ({end}) must be after start ({start})");
    }
    Ok(WorkHours {
        timezone,
        days,
        start: start_t,
        end: end_t,
        margin_before: parse_margin(margin_before)?,
        margin_after: parse_margin(margin_after)?,
    })
}

pub fn parse_margin(s: &str) -> Result<chrono::Duration> {
    let s = s.trim();
    if s == "0" {
        return Ok(chrono::Duration::zero());
    }
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: u32 = num
        .parse()
        .map_err(|_| anyhow!("invalid margin {s:?}; use e.g. 1h or 30m"))?;
    match unit {
        "h" => Ok(chrono::Duration::hours(n.into())),
        "m" => Ok(chrono::Duration::minutes(n.into())),
        _ => bail!("invalid margin {s:?}; use e.g. 1h or 30m"),
    }
}

pub fn required_scopes(cfg: &Config, account: &str) -> Vec<&'static str> {
    let mut scopes = Vec::new();
    if cfg.sync.iter().any(|r| r.from.account == account) {
        scopes.extend(SOURCE_SCOPES);
    }
    if cfg.sync.iter().any(|r| r.to.account == account) {
        scopes.extend(DEST_SCOPES);
    }
    scopes
}

fn parse_day(d: &str) -> Result<Weekday> {
    Ok(match d.to_ascii_lowercase().as_str() {
        "mon" => Weekday::Mon,
        "tue" => Weekday::Tue,
        "wed" => Weekday::Wed,
        "thu" => Weekday::Thu,
        "fri" => Weekday::Fri,
        "sat" => Weekday::Sat,
        "sun" => Weekday::Sun,
        _ => bail!("unknown day {d:?}; use mon, tue, wed, thu, fri, sat, sun"),
    })
}

fn day_name(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => "mon",
        Weekday::Tue => "tue",
        Weekday::Wed => "wed",
        Weekday::Thu => "thu",
        Weekday::Fri => "fri",
        Weekday::Sat => "sat",
        Weekday::Sun => "sun",
    }
}

fn parse_time(s: &str) -> Result<NaiveTime> {
    NaiveTime::parse_from_str(s, "%H:%M").map_err(|_| anyhow!("invalid time {s:?}; use HH:MM"))
}

fn format_margin(d: chrono::Duration) -> String {
    let m = d.num_minutes();
    if m == 0 {
        "0".into()
    } else if m % 60 == 0 {
        format!("{}h", m / 60)
    } else {
        format!("{m}m")
    }
}

fn expand_tilde(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(p),
    }
}

fn to_raw(c: &Config) -> RawConfig {
    let wh = &c.work_hours;
    RawConfig {
        oauth_client: c.oauth_client.to_string_lossy().into_owned(),
        label: Some(c.label.clone()),
        horizon_days: Some(c.horizon_days),
        poll_seconds: Some(c.poll_seconds),
        work_hours: RawWorkHours {
            timezone: wh.timezone.name().to_string(),
            days: wh.days.iter().map(|d| day_name(*d).to_string()).collect(),
            start: wh.start.format("%H:%M").to_string(),
            end: wh.end.format("%H:%M").to_string(),
            margin_before: Some(format_margin(wh.margin_before)),
            margin_after: Some(format_margin(wh.margin_after)),
        },
        accounts: c.accounts.clone(),
        sync: c.sync.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveTime, Weekday};
    use std::path::Path;

    const MINIMAL: &str = r#"
oauth_client = "/tmp/client.json"
[work_hours]
timezone = "America/Chicago"
days = ["mon"]
start = "08:00"
end = "17:00"
[accounts.personal]
[accounts.work]
[[sync]]
from = { account = "personal", calendar = "abc" }
to = { account = "work", calendar = "primary" }
"#;

    fn fixture() -> Config {
        load(Path::new("tests/fixtures/config_basic.toml")).unwrap()
    }

    #[test]
    fn home_honors_override() {
        assert_eq!(
            home_from(Some("/tmp/demo-home".into())),
            PathBuf::from("/tmp/demo-home")
        );
        assert_eq!(home_from(None), dirs::home_dir().unwrap());
    }

    #[test]
    fn loads_spec_example() {
        let c = fixture();
        assert_eq!(c.work_hours.timezone, chrono_tz::America::Chicago);
        assert_eq!(
            c.work_hours.days,
            vec![
                Weekday::Mon,
                Weekday::Tue,
                Weekday::Wed,
                Weekday::Thu,
                Weekday::Fri
            ]
        );
        assert_eq!(
            c.work_hours.start,
            NaiveTime::from_hms_opt(8, 0, 0).unwrap()
        );
        assert_eq!(c.work_hours.end, NaiveTime::from_hms_opt(17, 0, 0).unwrap());
        assert_eq!(c.work_hours.margin_before, chrono::Duration::minutes(60));
        assert_eq!(c.work_hours.margin_after, chrono::Duration::minutes(60));
        assert_eq!(c.poll_seconds, 300);
        assert_eq!(c.sync.len(), 1);
        assert_eq!(c.sync[0].from.account, "personal");
        assert_eq!(c.sync[0].to.account, "work");
        assert_eq!(c.sync[0].to.calendar, "primary");
        assert!(!c.oauth_client.to_string_lossy().starts_with('~'));
    }

    #[test]
    fn defaults_apply_when_omitted() {
        let c = parse(MINIMAL).unwrap();
        assert_eq!(c.label, "Personal Commitment");
        assert_eq!(c.horizon_days, 90);
        assert_eq!(c.poll_seconds, 300);
        assert_eq!(c.work_hours.margin_before, chrono::Duration::minutes(60));
        assert_eq!(c.work_hours.margin_after, chrono::Duration::minutes(60));
    }

    #[test]
    fn parse_margin_accepts_units() {
        assert_eq!(parse_margin("1h").unwrap(), chrono::Duration::minutes(60));
        assert_eq!(parse_margin("30m").unwrap(), chrono::Duration::minutes(30));
        assert_eq!(parse_margin("90m").unwrap(), chrono::Duration::minutes(90));
        assert_eq!(parse_margin("0").unwrap(), chrono::Duration::zero());
        assert!(parse_margin("abc").is_err());
        assert!(parse_margin("-1h").is_err());
    }

    #[test]
    fn rejects_rule_with_unknown_account() {
        let bad = MINIMAL.replace(r#"account = "work""#, r#"account = "nope""#);
        let err = parse(&bad).unwrap_err().to_string();
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn rejects_end_before_start() {
        let bad = MINIMAL.replace(r#"end = "17:00""#, r#"end = "07:00""#);
        let err = parse(&bad).unwrap_err().to_string();
        assert!(err.contains("end"), "{err}");
    }

    #[test]
    fn required_scopes_by_role() {
        let mut c = parse(MINIMAL).unwrap();
        assert_eq!(required_scopes(&c, "personal"), SOURCE_SCOPES.to_vec());
        assert_eq!(required_scopes(&c, "work"), DEST_SCOPES.to_vec());
        c.sync.push(SyncRule {
            from: CalendarRef {
                account: "work".into(),
                calendar: "x".into(),
                name: None,
            },
            to: CalendarRef {
                account: "work".into(),
                calendar: "primary".into(),
                name: None,
            },
        });
        let both = required_scopes(&c, "work");
        for s in SOURCE_SCOPES.iter().chain(DEST_SCOPES.iter()) {
            assert!(both.contains(s));
        }
        assert_eq!(both.len(), 3);
    }

    #[test]
    fn save_roundtrips() {
        let c = fixture();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        save(&c, &p).unwrap();
        assert_eq!(load(&p).unwrap(), c);
    }
}

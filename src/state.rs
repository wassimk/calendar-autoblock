use crate::event::CopyKey;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub known_copies: BTreeMap<String, KnownCopy>,
    #[serde(default, with = "tombstone_list")]
    pub tombstones: BTreeMap<CopyKey, DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnownCopy {
    pub key: CopyKey,
    pub end: DateTime<Utc>,
    pub dest_account: String,
    pub dest_calendar: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub last_pass: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_success: BTreeMap<String, DateTime<Utc>>,
    pub offline_since: Option<DateTime<Utc>>,
    #[serde(default)]
    pub auth_errors: BTreeMap<String, String>,
    #[serde(default)]
    pub calendar_errors: BTreeMap<String, String>,
    #[serde(default)]
    pub last_counts: Counts,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub created: u32,
    pub patched: u32,
    pub deleted: u32,
}

pub fn state_dir() -> PathBuf {
    dirs::home_dir()
        .expect("no home directory")
        .join("Library/Application Support/calendar-autoblock")
}

pub fn load_json<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            serde_json::from_str(&text).with_context(|| format!("corrupt {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e).with_context(|| format!("couldn't read {}", path.display())),
    }
}

pub fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// JSON object keys must be strings, so tombstones are stored as a list of pairs
mod tombstone_list {
    use super::*;
    use serde::{Deserializer, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Entry {
        key: CopyKey,
        end: DateTime<Utc>,
    }

    pub fn serialize<S: Serializer>(
        map: &BTreeMap<CopyKey, DateTime<Utc>>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        let list: Vec<Entry> = map
            .iter()
            .map(|(k, e)| Entry {
                key: k.clone(),
                end: *e,
            })
            .collect();
        list.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<CopyKey, DateTime<Utc>>, D::Error> {
        let list = Vec::<Entry>::deserialize(d)?;
        Ok(list.into_iter().map(|e| (e.key, e.end)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::CopyKey;

    #[test]
    fn missing_state_loads_default() {
        let dir = tempfile::tempdir().unwrap();
        let s: State = load_json(&dir.path().join("state.json")).unwrap();
        assert!(s.known_copies.is_empty() && s.tombstones.is_empty());
    }

    #[test]
    fn state_roundtrips_with_tombstones() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.json");
        let key = CopyKey {
            ical_uid: "a@google.com".into(),
            original_start: Some("2026-11-10T16:00:00Z".into()),
        };
        let end: DateTime<Utc> = "2026-11-10T17:00:00Z".parse().unwrap();
        let mut s = State::default();
        s.tombstones.insert(key.clone(), end);
        s.known_copies.insert(
            "dest1".into(),
            KnownCopy {
                key: key.clone(),
                end,
                dest_account: "work".into(),
                dest_calendar: "primary".into(),
            },
        );
        save_json(&p, &s).unwrap();
        let back: State = load_json(&p).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn save_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("status.json");
        save_json(&p, &Status::default()).unwrap();
        assert!(p.exists());
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }
}

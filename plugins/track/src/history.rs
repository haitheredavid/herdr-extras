//! Agent history log: JSONL, one file per UTC month, written by `herdr-track record` and loaded
//! on start so the track shows what happened before the panel opened.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};

use crate::herdr::PLUGIN_ID;
use crate::model::AgentStatus;

/// One line of the history log, written by `herdr-track record`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEvent {
    pub ts_ms: i64,
    /// `HERDR_SESSION`, `default` when unset.
    pub session: String,
    /// Dotted herdr event name, e.g. `pane.agent_status_changed`.
    pub event: String,
    pub pane_id: String,
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AgentStatus>,
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

/// `$HERDR_PLUGIN_STATE_DIR`, else `~/.local/state/herdr/plugins/herdr-extras.track`.
pub fn log_dir() -> PathBuf {
    if let Some(dir) = env_nonempty("HERDR_PLUGIN_STATE_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join(".local/state/herdr/plugins").join(PLUGIN_ID)
}

fn month_file(dir: &Path, year: i32, month: u32) -> PathBuf {
    dir.join(format!("events-{year:04}-{month:02}.jsonl"))
}

fn utc(ts_ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ts_ms).unwrap_or_default()
}

/// `dir/events-YYYY-MM.jsonl` for the UTC month of `ts_ms`.
pub fn log_file(dir: &Path, ts_ms: i64) -> PathBuf {
    let t = utc(ts_ms);
    month_file(dir, t.year(), t.month())
}

fn previous_month(year: i32, month: u32) -> (i32, u32) {
    if month <= 1 { (year - 1, 12) } else { (year, month - 1) }
}

fn parse_lines(text: &str) -> impl Iterator<Item = LogEvent> + '_ {
    text.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| serde_json::from_str(l).ok())
}

/// Events from the previous and current UTC month files, oldest first. Missing files, unparseable
/// lines and a torn trailing line from a concurrent append are skipped.
pub fn load_events(dir: &Path, now_ms: i64) -> Vec<LogEvent> {
    let now = utc(now_ms);
    let (py, pm) = previous_month(now.year(), now.month());
    let mut events = Vec::new();
    for path in [month_file(dir, py, pm), month_file(dir, now.year(), now.month())] {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        events.extend(parse_lines(&String::from_utf8_lossy(&bytes)));
    }
    events.sort_by_key(|e| e.ts_ms);
    events
}

/// `HERDR_SESSION`, `default` when unset or empty.
pub fn session_name() -> String {
    env_nonempty("HERDR_SESSION").unwrap_or_else(|| "default".into())
}

/// Events of `session` at or after `since_ms`, oldest first (previous + current UTC month files).
pub fn load_since(dir: &Path, session: &str, now_ms: i64, since_ms: i64) -> Vec<LogEvent> {
    load_events(dir, now_ms).into_iter().filter(|e| e.session == session && e.ts_ms >= since_ms).collect()
}

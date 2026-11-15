//! Context usage per agent, read from omp session files: every assistant turn records its token
//! usage, and input + cache read + cache write is the context that turn ran with.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub at_ms: i64,
    /// Context tokens in use at this turn.
    pub ctx: u64,
    /// Dollar cost of this request.
    pub cost: f64,
}

/// One session file, read incrementally: only bytes appended since the last refresh are parsed.
#[derive(Debug, Default)]
struct Tail {
    offset: u64,
    /// Bytes after the last newline: a line still being written.
    pending: Vec<u8>,
    samples: Vec<Sample>,
}

#[derive(Debug, Default)]
pub struct Tracker {
    files: HashMap<PathBuf, Tail>,
}

/// Sample for an assistant message line; None for every other line.
fn parse_sample(line: &str) -> Option<Sample> {
    // Cheap filter first: session files are mostly tool and custom records.
    if !line.contains("\"usage\"") || !line.contains("\"assistant\"") {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "message" || v.pointer("/message/role")?.as_str()? != "assistant" {
        return None;
    }
    let usage = v.pointer("/message/usage")?;
    let n = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
    let at_ms = chrono::DateTime::parse_from_rfc3339(v.get("timestamp")?.as_str()?).ok()?.timestamp_millis();
    let cost = usage.pointer("/cost/total").and_then(Value::as_f64).unwrap_or(0.0);
    Some(Sample { at_ms, ctx: n("input") + n("cacheRead") + n("cacheWrite"), cost })
}

impl Tail {
    fn refresh(&mut self, path: &Path) {
        let Ok(mut f) = File::open(path) else { return };
        let Ok(len) = f.metadata().map(|m| m.len()) else { return };
        if len < self.offset {
            // Truncated or replaced: start over.
            *self = Tail::default();
        }
        if len == self.offset || f.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut bytes = std::mem::take(&mut self.pending);
        let Ok(read) = f.take(len - self.offset).read_to_end(&mut bytes) else { return };
        self.offset += read as u64;
        let complete = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        self.pending = bytes.split_off(complete);
        let text = String::from_utf8_lossy(&bytes);
        let before = self.samples.len();
        self.samples.extend(text.lines().filter_map(parse_sample));
        if self.samples[before.saturating_sub(1)..].windows(2).any(|w| w[0].at_ms > w[1].at_ms) {
            self.samples.sort_by_key(|s| s.at_ms);
        }
    }
}

impl Tracker {
    /// Reads whatever was appended to each of `paths` since the last call.
    pub fn refresh<'a>(&mut self, paths: impl IntoIterator<Item = &'a PathBuf>) {
        for p in paths {
            self.files.entry(p.clone()).or_default().refresh(p);
        }
    }

    fn samples<'a>(&'a self, sessions: &'a [PathBuf]) -> impl Iterator<Item = &'a [Sample]> + 'a {
        sessions.iter().filter_map(|p| self.files.get(p)).map(|t| t.samples.as_slice())
    }

    /// Whether any of the lane's session files has usage yet.
    pub fn has_samples(&self, sessions: &[PathBuf]) -> bool {
        self.samples(sessions).any(|s| !s.is_empty())
    }

    #[cfg(test)]
    pub fn insert(&mut self, path: PathBuf, samples: Vec<Sample>) {
        self.files.insert(path, Tail { samples, ..Tail::default() });
    }

    /// Time of the first usage sample across the lane's sessions.
    pub fn first_at(&self, sessions: &[PathBuf]) -> Option<i64> {
        self.samples(sessions).filter_map(|s| s.first()).map(|s| s.at_ms).min()
    }

    /// Context in use at `t_ms`: the latest sample at or before it across the lane's sessions.
    pub fn ctx_at(&self, sessions: &[PathBuf], t_ms: i64) -> Option<u64> {
        self.samples(sessions)
            .filter_map(|s| s[..s.partition_point(|x| x.at_ms <= t_ms)].last())
            .max_by_key(|s| s.at_ms)
            .map(|s| s.ctx)
    }

    /// Requests and their total cost in `[from_ms, to_ms]` across the lane's sessions.
    pub fn spend_between(&self, sessions: &[PathBuf], from_ms: i64, to_ms: i64) -> (usize, f64) {
        self.samples(sessions)
            .flat_map(|s| s.iter().filter(|x| x.at_ms >= from_ms && x.at_ms <= to_ms))
            .fold((0, 0.0), |(n, c), s| (n + 1, c + s.cost))
    }

    /// Largest context in `[from_ms, to_ms]`, counting the value already in effect at `from_ms`.
    pub fn max_between(&self, sessions: &[PathBuf], from_ms: i64, to_ms: i64) -> Option<u64> {
        let inside =
            self.samples(sessions).flat_map(|s| s.iter().filter(|x| x.at_ms > from_ms && x.at_ms <= to_ms)).map(|s| s.ctx).max();
        inside.max(self.ctx_at(sessions, from_ms))
    }
}

/// `312k`, `1.25M`, `2M`.
pub fn fmt_tokens(n: u64) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..1_000_000 => format!("{}k", n / 1000),
        _ => format!("{:.2}", n as f64 / 1e6).trim_end_matches('0').trim_end_matches('.').to_string() + "M",
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    pub fn assistant(ts: &str, input: u64, cache_read: u64, cache_write: u64) -> String {
        format!(
            r#"{{"type":"message","timestamp":"{ts}","message":{{"role":"assistant","usage":{{"input":{input},"output":9,"cacheRead":{cache_read},"cacheWrite":{cache_write},"cost":{{"total":0.25}}}}}}}}"#
        )
    }

    #[test]
    fn parses_assistant_usage_only() {
        let s = parse_sample(&assistant("2026-11-13T00:00:01.000Z", 2, 300, 10)).unwrap();
        assert_eq!(s.ctx, 312);
        assert_eq!(s.cost, 0.25);
        assert_eq!(s.at_ms, chrono::DateTime::parse_from_rfc3339("2026-11-13T00:00:01Z").unwrap().timestamp_millis());
        let user = r#"{"type":"message","timestamp":"2026-11-13T00:00:01Z","message":{"role":"user","usage":{"input":1}}}"#;
        assert_eq!(parse_sample(user), None);
        assert_eq!(parse_sample(r#"{"type":"model_usage","usage":{"input":1},"x":"assistant"}"#), None);
    }

    #[test]
    fn reads_appends_and_holds_partial_lines() {
        let dir = std::env::temp_dir().join(format!("herdr-trail-usage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        let line1 = assistant("2026-11-13T00:00:01Z", 0, 100, 0);
        let line2 = assistant("2026-11-13T00:00:05Z", 0, 400, 0);
        std::fs::write(&path, format!("{line1}\n{}", &line2[..20])).unwrap();
        let sessions = vec![path.clone()];
        let mut t = Tracker::default();
        t.refresh(&sessions);
        let t1 = chrono::DateTime::parse_from_rfc3339("2026-11-13T00:00:01Z").unwrap().timestamp_millis();
        assert_eq!(t.ctx_at(&sessions, t1 + 10_000), Some(100), "torn line not parsed yet");

        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{}", &line2[20..]).unwrap();
        t.refresh(&sessions);
        assert_eq!(t.ctx_at(&sessions, t1 - 1), None);
        assert_eq!(t.ctx_at(&sessions, t1 + 3000), Some(100));
        assert_eq!(t.ctx_at(&sessions, t1 + 4000), Some(400));
        assert_eq!(t.max_between(&sessions, t1 + 1000, t1 + 3000), Some(100), "value in effect counts");
        assert_eq!(t.max_between(&sessions, t1, t1 + 9000), Some(400));
        assert_eq!(t.spend_between(&sessions, t1, t1 + 9000), (2, 0.5));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn token_counts_read_short() {
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(312_400), "312k");
        assert_eq!(fmt_tokens(1_250_000), "1.25M");
        assert_eq!(fmt_tokens(1_500_000), "1.5M");
        assert_eq!(fmt_tokens(2_000_000), "2M");
    }
}

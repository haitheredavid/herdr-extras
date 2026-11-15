//! `config.toml` in the plugin config dir: behavior knobs, read once at start. Every key is
//! optional; a key it can't use is named in the header and keeps its default.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use toml::{Table, Value};

use crate::view::preset_from_name;

/// `$HERDR_PLUGIN_CONFIG_DIR`, else `~/.config/herdr/plugins/config/<plugin id>`.
pub fn config_dir() -> PathBuf {
    std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("HOME").unwrap_or_default();
        PathBuf::from(home).join(".config/herdr/plugins/config").join(crate::herdr::PLUGIN_ID)
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// `[start] view`: a `--view` name, already checked.
    pub start_view: Option<String>,
    /// `[start] orient`: `h` or `v`, already checked.
    pub start_orient: Option<String>,
    /// `[start] remember`: read and write `prefs.toml`.
    pub remember: bool,
    /// Mouse capture: click to select, double-click to jump.
    pub mouse: bool,
    /// `[filter] quiet_after`: how long an idle, done or unknown agent stays in `active_only`.
    pub quiet_after_ms: i64,
    /// `[notify] blocked_after`: notify when an agent has been blocked this long; None is off.
    pub blocked_after_ms: Option<i64>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            start_view: None,
            start_orient: None,
            remember: true,
            mouse: false,
            quiet_after_ms: 30 * 60_000,
            blocked_after_ms: None,
        }
    }
}

/// `90s`, `5m`, `2h` in ms.
pub fn parse_dur(s: &str) -> Option<i64> {
    let (n, unit) = s.split_at(s.len().checked_sub(1)?);
    let n: i64 = n.parse().ok().filter(|n| *n >= 0)?;
    Some(
        n * match unit {
            "s" => 1000,
            "m" => 60_000,
            "h" => 3_600_000,
            _ => return None,
        },
    )
}

impl Config {
    /// Applies the keys of a `config.toml`; returns one message per key it couldn't use.
    fn apply(&mut self, table: &Table) -> Vec<String> {
        let mut problems = Vec::new();
        for (key, value) in table {
            let result = match (key.as_str(), value.as_table()) {
                ("mouse", _) => value.as_bool().map(|b| self.mouse = b).ok_or_else(|| "expected true or false".to_string()),
                ("start" | "filter" | "notify", Some(inner)) => {
                    for (name, v) in inner {
                        if let Err(e) = self.set(key, name, v) {
                            problems.push(format!("{key}.{name}: {e}"));
                        }
                    }
                    Ok(())
                }
                ("start" | "filter" | "notify", None) => Err("expected a table".into()),
                _ => Err("unknown key".into()),
            };
            if let Err(e) = result {
                problems.push(format!("{key}: {e}"));
            }
        }
        problems
    }

    fn set(&mut self, table: &str, name: &str, value: &Value) -> Result<(), String> {
        let text = || value.as_str().ok_or_else(|| "expected a string".to_string());
        let dur = || text().and_then(|v| parse_dur(v).ok_or_else(|| format!("expected a duration like \"5m\", got {v:?}")));
        match (table, name) {
            ("start", "view") => {
                let v = text()?;
                preset_from_name(v).ok_or_else(|| format!("unknown view {v:?}"))?;
                self.start_view = Some(v.to_string());
            }
            ("start", "orient") => {
                let v = text()?;
                if !matches!(v, "h" | "v") {
                    return Err(format!("expected \"h\" or \"v\", got {v:?}"));
                }
                self.start_orient = Some(v.to_string());
            }
            ("start", "remember") => self.remember = value.as_bool().ok_or("expected true or false")?,
            ("filter", "quiet_after") => self.quiet_after_ms = dur()?,
            ("notify", "blocked_after") => self.blocked_after_ms = Some(dur()?),
            _ => return Err("unknown key".into()),
        }
        Ok(())
    }
}

/// Defaults with `path` applied; a missing file is not a problem.
pub fn load(path: &Path) -> (Config, Vec<String>) {
    let mut config = Config::default();
    let Ok(text) = std::fs::read_to_string(path) else { return (config, Vec::new()) };
    match text.parse::<Table>() {
        Ok(table) => {
            let problems = config.apply(&table);
            (config, problems)
        }
        Err(e) => (config, vec![format!("{}: {}", path.display(), e.message())]),
    }
}

/// The process config and any problems loading it. Tests always get the defaults.
static CONFIG: LazyLock<(Config, Vec<String>)> =
    LazyLock::new(|| if cfg!(test) { (Config::default(), Vec::new()) } else { load(&config_dir().join("config.toml")) });

pub fn config() -> &'static Config {
    &CONFIG.0
}

/// Keys of `config.toml` that couldn't be used, for the header.
pub fn config_problems() -> &'static [String] {
    &CONFIG.1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_checked_and_bad_ones_keep_defaults() {
        let table: Table = "bogus = 1\nmouse = true\n[start]\nview = \"heat\"\norient = \"sideways\"\nremember = false\n[filter]\nquiet_after = \"10m\"\n[notify]\nblocked_after = \"soon\""
            .parse()
            .unwrap();
        let mut c = Config::default();
        let mut problems = c.apply(&table);
        problems.sort();
        assert_eq!(c.start_view.as_deref(), Some("heat"));
        assert_eq!(c.start_orient, None, "invalid value keeps the default");
        assert!(c.mouse && !c.remember);
        assert_eq!(c.quiet_after_ms, 600_000);
        assert_eq!(c.blocked_after_ms, None);
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems[0].starts_with("bogus: unknown key"));
        assert!(problems[1].starts_with("notify.blocked_after"));
        assert!(problems[2].starts_with("start.orient"));
    }

    #[test]
    fn durations_parse() {
        assert_eq!(parse_dur("90s"), Some(90_000));
        assert_eq!(parse_dur("5m"), Some(300_000));
        assert_eq!(parse_dur("2h"), Some(7_200_000));
        assert_eq!(parse_dur("5"), None);
        assert_eq!(parse_dur("m"), None);
        assert_eq!(parse_dur(""), None);
    }
}

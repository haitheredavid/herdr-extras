//! `config.toml` in the plugin config dir: behavior knobs, read once at start. Every key is
//! optional; a key it can't use is named in the header and keeps its default.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use toml::{Table, Value};

use crate::view::ViewKind;

/// `$HERDR_PLUGIN_CONFIG_DIR`, else `~/.config/herdr/plugins/config/<plugin id>`.
pub fn config_dir() -> PathBuf {
    std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("HOME").unwrap_or_default();
        PathBuf::from(home).join(".config/herdr/plugins/config").join(crate::herdr::PLUGIN_ID)
    })
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    /// `[start] view`: a `--view` name, already checked.
    pub start_view: Option<String>,
    /// `[start] orient`: `h` or `v`, already checked.
    pub start_orient: Option<String>,
}

impl Config {
    /// Applies the keys of a `config.toml`; returns one message per key it couldn't use.
    fn apply(&mut self, table: &Table) -> Vec<String> {
        let mut problems = Vec::new();
        for (key, value) in table {
            match (key.as_str(), value.as_table()) {
                ("start", Some(inner)) => {
                    for (name, v) in inner {
                        if let Err(e) = self.set_start(name, v) {
                            problems.push(format!("start.{name}: {e}"));
                        }
                    }
                }
                ("start", None) => problems.push(format!("{key}: expected a table")),
                _ => problems.push(format!("{key}: unknown key")),
            }
        }
        problems
    }

    fn set_start(&mut self, name: &str, value: &Value) -> Result<(), String> {
        let text = || value.as_str().ok_or_else(|| "expected a string".to_string());
        match name {
            "view" => {
                let v = text()?;
                ViewKind::from_name(v).ok_or_else(|| format!("unknown view {v:?}"))?;
                self.start_view = Some(v.to_string());
            }
            "orient" => {
                let v = text()?;
                if !matches!(v, "h" | "v") {
                    return Err(format!("expected \"h\" or \"v\", got {v:?}"));
                }
                self.start_orient = Some(v.to_string());
            }
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
    fn start_keys_are_checked() {
        let table: Table = "bogus = 1\n[start]\nview = \"heat\"\norient = \"sideways\"".parse().unwrap();
        let mut c = Config::default();
        let mut problems = c.apply(&table);
        problems.sort();
        assert_eq!(c.start_view.as_deref(), Some("heat"));
        assert_eq!(c.start_orient, None, "invalid value keeps the default");
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].starts_with("bogus: unknown key"));
        assert!(problems[1].starts_with("start.orient"));
    }
}

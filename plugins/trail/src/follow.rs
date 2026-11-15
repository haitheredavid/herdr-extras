//! `herdr-trail follow`: the `workspace.focused`/`tab.focused` hook that keeps the band pinned.
//! `open split` records the band's terminal id (pane ids are per workspace and change when a pane
//! moves; the terminal id doesn't), and each focus change moves the band into the focused tab.

use std::path::PathBuf;

use anyhow::{Context, anyhow};
use serde_json::{Value, json};

use crate::herdr::request;
use crate::history::log_dir;
use crate::open::{BAND_ROWS, shorten};

fn pin_file() -> PathBuf {
    log_dir().join("pinned")
}

/// Pins the band in `pane`, a pane object as herdr reports it.
pub fn pin(pane: &Value) -> anyhow::Result<()> {
    let terminal = match pane["terminal_id"].as_str() {
        Some(t) => t.to_string(),
        None => {
            let id = pane["pane_id"].as_str().ok_or_else(|| anyhow!("no pane id to pin"))?;
            let got = request("pane.get", json!({ "pane_id": id }))?;
            got["pane"]["terminal_id"].as_str().ok_or_else(|| anyhow!("pane.get: no terminal id"))?.to_string()
        }
    };
    let path = pin_file();
    std::fs::create_dir_all(log_dir()).with_context(|| format!("create {}", log_dir().display()))?;
    std::fs::write(&path, terminal).with_context(|| format!("write {}", path.display()))
}

pub fn run() -> anyhow::Result<()> {
    let Ok(terminal) = std::fs::read_to_string(pin_file()) else { return Ok(()) };
    let payload: Value =
        std::env::var("HERDR_PLUGIN_EVENT_JSON").ok().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or(Value::Null);
    let data = &payload["data"];
    let tab_id = match data["tab_id"].as_str() {
        Some(t) => t.to_string(),
        None => {
            let ws = data["workspace_id"].as_str().ok_or_else(|| anyhow!("focus event names no workspace"))?;
            let got = request("workspace.get", json!({ "workspace_id": ws }))?;
            got["workspace"]["active_tab_id"].as_str().ok_or_else(|| anyhow!("workspace.get: no active tab"))?.to_string()
        }
    };

    let list = request("pane.list", json!({}))?;
    let panes = list["panes"].as_array().map(Vec::as_slice).unwrap_or_default();
    let Some(band) = panes.iter().find(|p| p["terminal_id"].as_str() == Some(terminal.trim())) else {
        // The band was closed: unpin.
        let _ = std::fs::remove_file(pin_file());
        return Ok(());
    };
    let (Some(band_id), Some(band_tab)) = (band["pane_id"].as_str(), band["tab_id"].as_str()) else { return Ok(()) };
    if !should_move(panes, band_tab, &tab_id) {
        return Ok(());
    }

    let moved = request(
        "pane.move",
        json!({
            "pane_id": band_id,
            "destination": { "type": "tab", "tab_id": tab_id, "split": "down" },
            "focus": false,
        }),
    )?;
    let new_id = moved["move_result"]["pane"]["pane_id"].as_str().ok_or_else(|| anyhow!("pane.move: no pane id in response"))?;
    shorten(new_id, BAND_ROWS)
}

/// Move unless the band is already in the focused tab, or is alone in its tab (moving it would
/// close that tab).
fn should_move(panes: &[Value], band_tab: &str, focused_tab: &str) -> bool {
    band_tab != focused_tab && panes.iter().filter(|p| p["tab_id"].as_str() == Some(band_tab)).count() > 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_only_into_another_tab_and_never_empties_one() {
        let panes = [
            json!({"pane_id": "a:p1", "tab_id": "a:t1"}),
            json!({"pane_id": "a:p2", "tab_id": "a:t1"}),
            json!({"pane_id": "b:p1", "tab_id": "b:t1"}),
        ];
        assert!(should_move(&panes, "a:t1", "b:t1"));
        assert!(!should_move(&panes, "a:t1", "a:t1"), "already in the focused tab");
        assert!(!should_move(&panes, "b:t1", "a:t1"), "alone in its tab");
    }
}

//! `herdr-track open split|popup`: opens the track from an action, as a full-width split below the
//! focused pane (horizontal timelines want the width) or as a popup over the layout.

use anyhow::bail;
use serde_json::json;

use crate::herdr::{plugin_id, request};

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let params = match args.first().map(String::as_str) {
        Some("split") => json!({
            "plugin_id": plugin_id(),
            "entrypoint": "track",
            "placement": "split",
            "direction": "down",
            "focus": true,
        }),
        Some("popup") => json!({
            "plugin_id": plugin_id(),
            "entrypoint": "track-popup",
            "placement": "popup",
            "width": "80%",
            "height": "80%",
            "focus": true,
        }),
        other => bail!("unknown open target {other:?}; usage: herdr-track open split|popup"),
    };
    request("plugin.pane.open", params).map(drop)
}

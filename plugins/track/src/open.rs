//! `herdr-track open split|popup`: opens the track from an action, as a band (a split below the
//! focused pane, shortened to `BAND_ROWS`) or as a large popup over the layout.

use anyhow::{anyhow, bail};
use serde_json::{Value, json};

use crate::herdr::{plugin_id, request};

/// Band height in rows: header, two usage-band lanes, detail and footer.
pub const BAND_ROWS: u16 = 14;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        Some("split") => {
            let opened = request(
                "plugin.pane.open",
                json!({
                    "plugin_id": plugin_id(),
                    "entrypoint": "track",
                    "placement": "split",
                    "direction": "down",
                    "focus": true,
                }),
            )?;
            let pane_id = opened["plugin_pane"]["pane"]["pane_id"]
                .as_str()
                .ok_or_else(|| anyhow!("plugin.pane.open: no pane id in response"))?;
            shorten(pane_id, BAND_ROWS)
        }
        Some("popup") => request(
            "plugin.pane.open",
            json!({
                "plugin_id": plugin_id(),
                "entrypoint": "track-popup",
                "placement": "popup",
                "width": "80%",
                "height": "80%",
                "focus": true,
            }),
        )
        .map(drop),
        other => bail!("unknown open target {other:?}; usage: herdr-track open split|popup"),
    }
}

/// herdr only sizes popups, so shrink the new split to `rows` by setting its parent split's ratio.
fn shorten(pane_id: &str, rows: u16) -> anyhow::Result<()> {
    let export = request("layout.export", json!({ "pane_id": pane_id }))?;
    let root = &export["layout"]["root"];
    let path = path_to(root, pane_id).ok_or_else(|| anyhow!("layout.export: {pane_id} not in tab layout"))?;
    let Some((&is_second, parent_path)) = path.split_last() else { return Ok(()) };
    let parent = parent_path.iter().fold(root, |n, &second| &n[if second { "second" } else { "first" }]);
    let mut members = Vec::new();
    leaves(parent, &mut members);

    let layout = request("pane.layout", json!({ "pane_id": pane_id }))?;
    let rects: Vec<(i64, i64)> = layout["layout"]["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["pane_id"].as_str().is_some_and(|id| members.contains(&id)))
        .filter_map(|p| Some((p["rect"]["y"].as_i64()?, p["rect"]["y"].as_i64()? + p["rect"]["height"].as_i64()?)))
        .collect();
    let (Some(top), Some(bottom)) = (rects.iter().map(|r| r.0).min(), rects.iter().map(|r| r.1).max()) else {
        bail!("pane.layout: no rects for the split around {pane_id}");
    };
    request(
        "layout.set_split_ratio",
        json!({
            "tab_id": export["layout"]["tab_id"],
            "path": parent_path,
            "ratio": first_ratio(is_second, (bottom - top) as f64, f64::from(rows)),
        }),
    )?;
    Ok(())
}

/// Ratio (share of the first child) that leaves `cells` for our pane.
fn first_ratio(ours_is_second: bool, total: f64, cells: f64) -> f64 {
    let ours = (cells / total).clamp(0.05, 0.95);
    if ours_is_second { 1.0 - ours } else { ours }
}

/// Path of `first`(false)/`second`(true) steps from `node` to the pane leaf.
fn path_to(node: &Value, pane_id: &str) -> Option<Vec<bool>> {
    match node["type"].as_str()? {
        "pane" => (node["pane_id"].as_str() == Some(pane_id)).then(Vec::new),
        "split" => [(false, "first"), (true, "second")].into_iter().find_map(|(step, key)| {
            path_to(&node[key], pane_id).map(|mut rest| {
                rest.insert(0, step);
                rest
            })
        }),
        _ => None,
    }
}

fn leaves<'a>(node: &'a Value, out: &mut Vec<&'a str>) {
    match node["type"].as_str() {
        Some("pane") => out.extend(node["pane_id"].as_str()),
        Some("split") => {
            leaves(&node["first"], out);
            leaves(&node["second"], out);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Value {
        json!({"type": "split", "first": {"type": "pane", "pane_id": "a"},
               "second": {"type": "split",
                          "first": {"type": "pane", "pane_id": "b"},
                          "second": {"type": "pane", "pane_id": "c"}}})
    }

    #[test]
    fn path_to_walks_first_and_second() {
        assert_eq!(path_to(&tree(), "a"), Some(vec![false]));
        assert_eq!(path_to(&tree(), "c"), Some(vec![true, true]));
        assert_eq!(path_to(&tree(), "zz"), None);
    }

    #[test]
    fn leaves_lists_subtree_panes() {
        let t = tree();
        let mut out = Vec::new();
        leaves(&t["second"], &mut out);
        assert_eq!(out, ["b", "c"]);
    }

    #[test]
    fn ratio_leaves_requested_columns_on_our_side() {
        assert!((first_ratio(true, 65.0, 14.0) - 51.0 / 65.0).abs() < 1e-9);
        assert!((first_ratio(false, 100.0, 25.0) - 0.25).abs() < 1e-9);
        assert!((first_ratio(true, 10.0, 14.0) - 0.05).abs() < 1e-9, "clamped when the split is too short");
    }
}

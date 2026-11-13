# herdr-extras

Personal [herdr](https://herdr.dev) TUI plugins, one subdirectory per plugin, each installable on its own.

| Plugin | What it does |
| --- | --- |
| `track` | Agent activity timeline: one growing line per agent pane, a node at every state change, in time or sequence view, left→right or top→bottom |

## Install

```
herdr plugin install haitheredavid/herdr-extras/plugins/track   # from GitHub; builds with cargo
herdr plugin link ~/projects/herdr-extras/plugins/track         # from a checkout; edits go live
```

`plugin link` runs no build step, so run `cargo build --release` in the plugin directory first (and after each change).

## Layout

Each `plugins/<name>/` is a self-contained crate with its own `herdr-plugin.toml`, `Cargo.lock` and `target/`. There is no Cargo workspace: herdr builds and runs each plugin from its own subdirectory, and the manifest's commands expect the binary at `plugins/<name>/target/release/`.

## track

```
track · TIME · ↔ · 2 agents · bucket 15s · follow ●
                      ┊19:27:00   ┊19:30:00   ┊19:33:00
● herdr-box — … herdr ◆─●━━●━━▸
✕ fix login   ctx-h   ◇━━━●──✕
```

Actions `herdr-extras.track.open` (side panel: a 64-column split right of the focused pane) and `herdr-extras.track.open-popup`.

- **Views** (`Tab`): TIME draws one cell per time bucket, so line length is wall-clock duration. SEQUENCE draws one evenly spaced node per state change, so every lane starts at the near edge.
- **Orientations** (`o`): ↔ lanes are rows with time flowing right; ↕ lanes are columns with time flowing down.
- **Keys** follow the orientation. The lane keys are `j/k` in ↔ and `h/l` in ↕, and the other pair scrolls along time. In TIME, `+/-` zoom through 1s, 5s, 15s, 1m and 5m buckets. `f` returns to following now, `Enter` focuses the selected agent's pane, `r` re-snapshots and `q` quits (`Esc` also quits in the popup).
- **Nodes**: `◆` created, `◇` first seen already running, `●` status change (colored by the new status), `◉` several changes in one cell, `✕` closed or exited, `▸`/`▾` now. Lines are `━` working (green), blocked (red) or done (blue), `─` idle, `┈` unknown.
- **History**: an `[[events]]` recorder appends every pane lifecycle event to `$HERDR_PLUGIN_STATE_DIR/events-YYYY-MM.jsonl` (one file per UTC month). The panel loads the last 2 h of the log on open, so nodes from before it opened still show.

## Develop

```
cd plugins/track
cargo test
cargo build --release
./target/release/herdr-track --dump --width 120 --height 20 [--view sequence] [--orient v]
```

`--dump` renders one frame against the running herdr to stdout.

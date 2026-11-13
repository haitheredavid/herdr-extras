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

Actions `herdr-extras.track.open` (a 14-row band below the focused pane, full width in a single-pane tab) and `herdr-extras.track.open-popup`.

- **Views** (`Tab`): TIME draws one cell per time bucket, so line length is wall-clock duration. SEQUENCE draws one evenly spaced node per state change, so every lane starts at the near edge. AREA is an area chart of each agent's context in use: a braille line per agent in its own color over a tinted fill, stacked so the top line is the total (`s` overlays them from zero instead: largest behind, overlapping fills blended so each shows through). It has a value axis with round ticks, a legend, and a label with the current context at each line's end. While following it fits the last 2 h of session history across the axis (`fit` in the header); `+/-` switch to TIME's buckets and `f` fits again.
- **Orientations** (`o`): ↔ lanes are rows with time flowing right; ↕ lanes are columns with time flowing down.
- **Usage bands** (`u` toggles): for omp agents, each lane is a 4-cell band and the line sits across it by context in use (input + cache tokens of the latest turn, read from the session file herdr reports for the pane), so it climbs as the conversation grows and drops on compaction. The scale is shared across visible lanes (`ctx ≤ 320k` in the header), and each lane's current context shows under its label. Without usage data lanes stay one cell thick.
- **Keys** follow the orientation. The lane keys are `j/k` in ↔ and `h/l` in ↕, and the other pair scrolls along time. In TIME, `+/-` zoom through 1s, 5s, 15s, 1m and 5m buckets. `f` returns to following now, `Enter` focuses the selected agent's pane, `u` toggles usage bands, `r` re-snapshots and `q` quits (`Esc` also quits in the popup).
- **Nodes**: `◆` created, `◇` first seen already running, `●` status change (colored by the new status), `◉` several changes in one cell, `✕` closed or exited, `▸`/`▾` now. Lines are `━` working (green), blocked (red) or done (blue), `─` idle, `┈` unknown.
- **History**: an `[[events]]` recorder appends every pane lifecycle event to `$HERDR_PLUGIN_STATE_DIR/events-YYYY-MM.jsonl` (one file per UTC month). The panel loads the last 2 h of the log on open, so nodes from before it opened still show.

## Develop

```
cd plugins/track
cargo test
cargo build --release
./target/release/herdr-track --dump --width 120 --height 20 [--view sequence|area] [--orient v]
```

`--dump` renders one frame against the running herdr to stdout.

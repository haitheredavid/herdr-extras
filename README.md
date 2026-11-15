# herdr-extras

Personal [herdr](https://herdr.dev) TUI plugins, one subdirectory per plugin, each installable on its own.

| Plugin | What it does |
| --- | --- |
| `trail` | Agent activity timeline: one growing line per agent pane, a node at every state change, in time or sequence view, left→right or top→bottom |

## Install

```
herdr plugin install haitheredavid/herdr-extras/plugins/trail   # from GitHub; builds with cargo
herdr plugin link path/to/herdr-extras/plugins/trail            # from a checkout; edits go live
```

`plugin link` runs no build step, so run `cargo build --release --locked --target-dir target` in the plugin directory first (and after each change).

## Layout

Each `plugins/<name>/` is a self-contained crate with its own `herdr-plugin.toml`, `Cargo.lock` and `target/`. There is no Cargo workspace: herdr builds and runs each plugin from its own subdirectory, and the manifest's commands expect the binary at `plugins/<name>/target/release/`.

## trail

```
trail · TIME · ↔ · 2 agents · bucket 15s · follow ●
                      ┊19:27:00   ┊19:30:00   ┊19:33:00
● herdr-box — … herdr ◆─●━━●━━▸
✕ fix login   ctx-h   ◇━━━●──✕
```

Actions `herdr-trail.open` (a 14-row band below the focused pane, full width in a single-pane tab) and `herdr-trail.open-popup`.

- **Views** (`Tab` cycles, `1`-`5` jump: TIME, SEQUENCE, AREA, HEAT, DASH): TIME draws one cell per time bucket, so line length is wall-clock duration. SEQUENCE draws one evenly spaced node per state change, so every lane starts at the near edge. HEAT gives each agent a row of time buckets shaded along the same viridis stretch as the agent colors by the share of the bucket spent working (red if blocked at all), with TIME's zoom and scrolling. DASH is a table, one row per agent over the last 2 h: context sparkline in the agent's color, current context, working share, cost, requests and current state. AREA is an area chart of each agent's context in use: a braille line per agent over a tinted fill, colored along viridis (blue-purple at the base to yellow on top, spread by agent count; the darkest purples are skipped for dark terminals), in one of three layouts that `s` cycles: stacked, so the top line is the total; overlaid from zero, largest behind, with overlapping fills blended so each shows through; or a streamgraph, stacked around a centered baseline so the thickness is the total (`Σ` on the axis). It has a value axis with round ticks, a legend, and a label with the current context at each line's end. While following it fits the last 2 h of session history across the axis (`fit` in the header); `+/-` switch to TIME's buckets and `f` fits again.
- **Orientations** (`o`): ↔ lanes are rows with time flowing right; ↕ lanes are columns with time flowing down.
- **Usage bands** (`u` toggles): for omp agents, each lane is a 4-cell band and the line sits across it by context in use (input + cache tokens of the latest turn, read from the session file herdr reports for the pane), so it climbs as the conversation grows and drops on compaction. The scale is shared across visible lanes (`ctx ≤ 320k` in the header), and each lane's current context shows under its label. Without usage data lanes stay one cell thick.
- **Keys** follow the orientation. The lane keys are `j/k` in ↔ and `h/l` in ↕, and the other pair scrolls along time. In TIME, `+/-` zoom through 1s, 5s, 15s, 1m and 5m buckets. `f` returns to following now, `Enter` focuses the selected agent's pane, `u` toggles usage bands, `r` re-snapshots and `q` quits (`Esc` also quits in the popup).
- **Nodes**: `◆` created, `◇` first seen already running, `●` status change (colored by the new status), `◉` several changes in one cell, `✕` closed or exited, `▸`/`▾` now. Lines take each agent's viridis color (the same one AREA uses); their weight is the status: `━` working, blocked or done, `─` idle, `┈` unknown. Nodes, the head and the gutter dot carry status colors: green working, red blocked, blue done.
- **History**: an `[[events]]` recorder appends every pane lifecycle event to `$HERDR_PLUGIN_STATE_DIR/events-YYYY-MM.jsonl` (one file per UTC month). The panel loads the last 2 h of the log on open, so nodes from before it opened still show.
- **Pinned band**: the band from `open` follows you. `workspace.focused`/`tab.focused` hooks move it (same process, so view and scroll survive) to the bottom of whichever tab you switch to, full width and 14 rows, without taking focus. It is tracked by terminal id in `$HERDR_PLUGIN_STATE_DIR/pinned`, since pane ids change when a pane moves across workspaces; closing the band unpins it, and opening another band pins the new one. It stays put if moving it would leave its tab empty.

### Theme

Every color comes from one set of tokens (`src/theme.rs`). To change any of them, put a `theme.toml` in the plugin's config directory (`herdr plugin config-dir herdr-trail`, also `$HERDR_PLUGIN_CONFIG_DIR`) and reopen the panel. Colors take a name (`green`, `darkgray`), `#rrggbb` or a 0-255 palette index; `background` and the `series` ramp need `#rrggbb` because fills are mixed from them. Keys it can't use are named in the header and keep their defaults. The defaults:

```toml
background = "#1c1c1e"   # assumed terminal background; fills mix toward it
dim = "darkgray"         # captions, axes, closed lanes, help
grid = "#48484e"         # chart gridlines
accent = "cyan"          # view name in the header
error = "red"            # socket and theme errors
fill_alpha = 0.3         # how much of a series' color its fill lays over what's behind
series_from = 0.25       # where along `series` the used stretch starts
# The one color ramp (viridis): agents are spread evenly along it from `series_from`, and HEAT
# shades along the same stretch, so every view draws from the same colors.
series = ["#440154", "#482475", "#414487", "#355f8d", "#2a788e", "#21918c", "#22a884", "#44bf70", "#7ad151", "#bddf26", "#fde725"]
heat_blocked = "red"

[status]
working = "green"
blocked = "red"
done = "blue"
idle = "darkgray"
unknown = "darkgray"

[node]
created = "cyan"
seen = "cyan"
closed = "gray"
```

## Develop

```
cd plugins/trail
cargo test
cargo build --release --locked --target-dir target
./target/release/herdr-trail --dump --width 120 --height 20 [--view sequence|area|heat|dash] [--orient v]
```

`--dump` renders one frame against the running herdr to stdout.

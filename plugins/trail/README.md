# trail

A [herdr](https://herdr.dev) plugin that shows what every agent pane has been doing: one lane per pane with a node at every state change, context and cost per agent, and a step-through timeline of an agent's session (requests, tools, waits, subagents). It opens as a band under your panes that follows you across tabs, or as a popup.

## Requirements

- herdr 0.9.5 or newer
- macOS or Linux
- A prebuilt release binary (downloaded on install), or Rust 1.88 or newer via [rustup](https://rustup.rs) for the source fallback

## Install

```
herdr plugin install haitheredavid/herdr-extras/plugins/trail --ref trail-v0.1.0
```

The install step downloads the binary for your platform from the `trail-v0.1.0` release, checks it against the release's `SHA256SUMS`, and falls back to `cargo build --release` when there is no binary for your machine or the download fails. Updating is reinstalling.

## Keybinding

Add to `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+alt+g"
type = "plugin_action"
command = "herdr-trail.open"          # a band below the focused pane
description = "agent trail (band)"

[[keys.command]]
key = "prefix+alt+shift+g"
type = "plugin_action"
command = "herdr-trail.open-popup"    # a large popup
description = "agent trail (popup)"
```

The band is pinned: when you switch workspace or tab, it moves to the bottom of the tab you land on (full width, `band.rows` tall) without taking focus, keeping its view and scroll. Closing it unpins it; opening another band pins the new one. It stays put if moving it would leave its tab empty.

## Views

The frames below are real `--dump` output with the workspace, tab and task names replaced.

### LANES

One lane per pane, grouped by workspace in ↔. The gutter shows the workspace, the agent's state shape, its task and how long it has been in that state; the right edge shows current context, cost over the history window and working share. `v` cycles the lens over the strip: **status** (a node per state change, line weight by state), **heat** (time buckets shaded by working share, red if blocked) and **ctx** (context in use, with a `Σ` totals row). `Space` expands an agent into a usage band. `t` switches the status lens between wall-clock time and evenly spaced steps.

160×14, status lens, inspector on the right:

```
LANES usage │ 3/3 │ status · time · fit 5m                                             hx ./docs/search-ranking-design.md · feat-search-ranking-api · hx  ? keys
                                               ┊17:00      ┊18:00      ┊19:00      ┊20:00      ┊21                  │ ● hx ./docs/search-ranking-design.md
feat-sea… ▌● hx ./docs/search… working 0s                                                        ◇     –      –   0%│ feat-search-ranking-api · tab data-infra …
web-clie… ▌○ omp — add rate l… idle 52m55s                                 ◆◉╍◉◉●━━✓●◉◉○─────────▸  129k  $9.48  26%│ working 0s
data-inf… ▌· user@workstation… unknown 1h54m                              ◇┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈▸     –      –   0%│ opened 21:13 · alive 0s
                                                                                                                    │ ctx – · peak –
                                                                                                                    │ – · 0 reqs · working 0% · 1 steps
                                                                                                                    │ ─ recent ─────────────────────────────────
                                                                                                                    │ 21:13:00  ◇ seen
                                                                                                                    │
                                                                                                                    │
                                                                                                                    │
                                                                                                                    │
▸ 2 closed in the last 2h — c shows them                                                                            │ I timeline · ⏎ jump · i hide
```

160×14, ctx lens:

```
LANES usage │ 3/3 │ ctx · fit 5m                                                       hx ./docs/search-ranking-design.md · feat-search-ranking-api · hx  ? keys
                                               ┊17:00      ┊18:00      ┊19:00      ┊20:00      ┊21                  │ ● hx ./docs/search-ranking-design.md
feat-sea… ▌● hx ./docs/search… working 0s                                                              –      –   0%│ feat-search-ranking-api · tab data-infra …
web-clie… ▌○ omp — add rate l… idle 52m55s                                     ▅▅▇▅▆▆▇████████████  129k  $9.48  26%│ working 0s
data-inf… ▌· user@workstation… unknown 1h54m                                                           –      –   0%│ opened 21:13 · alive 0s
   Σ                                                                                                129k  $9.48     │ ctx – · peak –
```

50×30, a narrow sidebar: two-row lanes under group headers, inspector below:

```
LANES usage │ 3/3 │ status · time · fit 5m  ? keys
           ┊18:00      ┊19:00      ┊20:00      ┊21
─ feat-search-ranking-api ────────────────────●1 ─
▌● hx ./docs/search… working 0s                  –
                                                 ◇
─ web-client ─────────────────────────────────○1 ─
▌○ omp — add rate l… idle 52m55s              129k
                           ◆◉╍◉◉●━━✓●◉◉○─────────▸
─ data-infra ─────────────────────────────────·1 ─
▌· user@workstation… unknown 1h54m               –
                          ◇┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈▸

▸ 2 closed in the last 2h — c shows them
──────────────────────────────────────────────────
● hx ./docs/search-ranking-design.md
feat-search-ranking-api · tab data-infra — nightl…
working 0s
opened 21:13 · alive 0s
ctx – · peak –
– · 0 reqs · working 0% · 1 steps
─ recent ─────────────────────────────────────────
21:13:00  ◇ seen

I timeline · ⏎ jump · i hide
```

### USAGE

`Tab` switches to USAGE: each agent's context in use over time as a braille line over a tinted fill, `v` toggles stacked and overlaid.

160×14:

```
lanes USAGE │ 3/3 │ stacked · fit                                                      hx ./docs/search-ranking-design.md · feat-search-ranking-api · hx  ? keys
 150k┤┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈
                                                                               ⣀⡤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤ omp — add… 129k
                                                                         ⢠⠖⠋⠉⠉⠉⠁
                                                 ⣰⠒⠚⠉⠉⡇                ⢠⠤⠼
 100k┤┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈⢸⠁┈┈┈┈⡇┈┈┈┈┈┈┈┈┈┈┈┈⢀⣀⣠⠤⠏┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈
                                          ⢀⣀⡴⠒⠒⠒⠋     ⡇⣠⠖⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠚
                                    ⢠⠖⠒⠒⠒⠒⠚           ⣿⠁
  50k┤┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈⣠⠏┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈⣿┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈
                                  ⢸⠁                  ⣿
                                  ⠉                   ⣿
                                                      ⣿
    0┤┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈⣿┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈ no usage: hx .…
       ┊19:20      ┊19:30       ┊19:40      ┊19:50      ┊20:00      ┊20:10      ┊20:20      ┊20:30      ┊20:40      ┊20:50      ┊21:00      ┊21:10
```

50×30:

```
lanes USAGE │ 3/3 │ stacked · fit           ? keys
 150k┤┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈


                     ⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀ omp — add… 129k
                    ⣸⠁
                   ⢠⠇
                   ⢸
              ⢀⣿   ⢸
              ⢸⢸   ⡼
 100k┤┈┈┈┈┈┈┈┈⢸⢸┈┈┈⡇┈┈┈┈┈┈┈┈┈┈┈┈┈┈
              ⢸⢸  ⢀⡇
              ⢸⢸  ⢸
             ⢠⠼⢸⡤⠖⠚
             ⡼ ⢸⡇
            ⣀⡇ ⠘⠃
            ⡇
            ⡇
            ⡇
  50k┤┈┈┈┈┈⠐⠃┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈

    0┤┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈ no usage: hx .…
                ┊20:00        ┊21:00
```

### Inspector

`i` toggles the inspector: the selected agent's task, location, state, age, context (current and peak), cost, requests, working share and its recent state changes. It sits on the right when there is room and below the lanes in a narrow pane (see the frames above).

### TIMELINE

`I` steps through the selected agent's session: a scrubber across the top (one glyph per stretch of the session, idle gaps collapsed), the steps below (turns, model requests, tool calls, waits), and the selected step's detail on the right. Turns and requests fold with `Space`; `Enter` opens a subagent's own timeline. Prompt and intent text is hidden until `T` reveals it.

160×14:

```
TIMELINE ▸ omp — add rate limits to search api │ live ● │ n: errors (4) │ gap                                                                             ? keys
✕▆▆▆▆▆▃▆▃▆▆▆▆…3m…│▃▃⑂·····│▆▆▃│…2m…│▆▆▆▃·│▆·······│▃·····✕▃▃⑂✕⧖⧖⧖⧖▆▆⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖▆…4m…▆⑂⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖⧖✕⧖▆·······│▆▆····⑂⧖⧖…2m…│▆·│◆…52m… 19:41 ─ 21:13
    20:19:19  ◆ gpt-6.1-terra  2s  +143 ctx 128k                                                    │ ◆ model · gpt-6.1-terra
  ▶ 20:19:52  turn 14  10 steps · 12s · $0.04 ✓                                                     │ 20:20:03 · took 1s
    20:19:52  ◆ gpt-6.1-terra  2s  +37 ctx 128k                                                     │ in 129k (cache 99%) · out 35
      20:19:55  r read  0s                                                                          │ ctx after 129k
      20:19:55  $ bash  0s                                                                          │ cost $0.01 · turn $0.04 · session $2.35
    20:19:55  ◆ gpt-6.1-terra  2s  +834 ctx 128k                                                    │
      20:19:58  ƒ eval  0s                                                                          │
    20:19:58  ◆ gpt-6.1-terra  1s  +227 ctx 129k                                                    │
      20:20:00  ƒ eval  0s                                                                          │
    20:20:00  ◆ gpt-6.1-terra  2s  +501 ctx 129k                                                    │
      20:20:02  $ bash  1s                                                                          │
▸   20:20:03  ◆ gpt-6.1-terra  1s  +112 ctx 129k                                                    │
```

50×30, the scrubber turns into a column on the left:

```
TIMELINE ▸ omp — add rate limits to search api
✕     20:15:58  ◆ gpt-6.1-terra  1s  +2.0k ctx 12…
▆       20:16:00  ⧖ wait 5s
…     20:16:05  ◆ gpt-6.1-terra  1s  +97 ctx 124k
⑂       20:16:06  ⧖ wait 5s
·     20:16:12  ◆ gpt-6.1-terra  3s  +272 ctx 124k
│   ┄┄ 2m idle ┄┄
…   ▶ 20:19:05  turn 13  11 steps · 16s · $0.03 ✓
│     20:19:05  ◆ gpt-6.1-terra  6s  +262 ctx 124k
│       20:19:12  r read  0s
·       20:19:12  r read  0s
│       20:19:12  r read  0s
✕       20:19:12  r read  0s
✕       20:19:12  r read  0s
⧖     20:19:12  ◆ gpt-6.1-terra  4s  +2.5k ctx 12…
⧖       20:19:16  e edit  0s
⧖     20:19:16  ◆ gpt-6.1-terra  3s  +710 ctx 127k
⧖       20:19:19  $ bash  0s
⧖     20:19:19  ◆ gpt-6.1-terra  2s  +143 ctx 128k
…   ▶ 20:19:52  turn 14  10 steps · 12s · $0.04 ✓
⑂     20:19:52  ◆ gpt-6.1-terra  2s  +37 ctx 128k
⧖       20:19:55  r read  0s
⧖       20:19:55  $ bash  0s
✕     20:19:55  ◆ gpt-6.1-terra  2s  +834 ctx 128k
·       20:19:58  ƒ eval  0s
│     20:19:58  ◆ gpt-6.1-terra  1s  +227 ctx 129k
⑂       20:20:00  ƒ eval  0s
…     20:20:00  ◆ gpt-6.1-terra  2s  +501 ctx 129k
◆       20:20:02  $ bash  1s
… ▸   20:20:03  ◆ gpt-6.1-terra  1s  +112 ctx 129k
```

## Keys

`?` shows these in the panel. Keys follow the orientation: in ↕ (`o`), agents are columns and the arrows follow the screen.

| Key | Action |
| --- | --- |
| **Move** | |
| `j`/`k` | next/previous agent |
| `J`/`K` | next/previous group |
| `b` | next blocked agent |
| `h`/`l`, `H`/`L` | back/forward in time, a quarter or a whole strip |
| `Home`/`End`, `PgUp`/`PgDn` | first/last agent, a page |
| `f` | follow now |
| `0` | follow now and fit time to the history |
| **Show** | |
| `Tab` | LANES ⇄ USAGE |
| `1`-`5` | time, sequence, area, heat, dash |
| `v` | lens: status, heat, ctx (USAGE: stacked or overlaid) |
| `t` | status lens on time or steps |
| `Space` | expand the agent into a usage band |
| `Z` | density: auto, compact, roomy |
| `i` | inspector on/off |
| `o` | agents as rows or columns |
| `+`/`-` | zoom time in/out |
| **Filter** | |
| `/` | search names, tasks, workspaces, folders |
| `a` | only active agents (blocked ones always show) |
| `c` | show closed agents |
| `w` | scope: all, this workspace, this tab |
| `s` | sort: stable, attention, recent, name |
| `g` | group: workspace, tab, none |
| `x` | fold the group (`Space` or `Enter` opens it) |
| **Act** | |
| `Enter` | jump to the agent's pane |
| `I` | step through the agent's session |
| `r` | refresh now |
| `Esc` | collapse, then close a popup |
| `q` | quit |

In the timeline:

| Key | Action |
| --- | --- |
| `j`/`k`, `J`/`K` | next/previous step, turn |
| `h`/`l` | previous/next stretch of the scrubber |
| `H`/`L` | first/last step |
| `n`/`N` | next/previous match |
| `;` then `e b c p s t` | match errors, blocked, compactions, prompts, subagents, a tool |
| `f` | follow the live end |
| `Space` | fold the turn, or the request's tools |
| `z` | fold every other turn |
| `t` | scrubber: gap, wall clock, one slot per step |
| `T` | text: none, intents, prompts (capped by `privacy.max_text`) |
| `Enter`, `]` | open a subagent (`Enter` elsewhere: jump to the pane) |
| `[`, `Backspace` | back out of a subagent, or to the previous session |
| `Esc` | back out; leave the timeline at the root |

With `mouse = true`, clicks work once the band is focused: click selects, double-click jumps to the pane.

## Agent support

Every agent herdr detects gets a lane with its state. Usage and the timeline come from the session file herdr reports for the pane.

| Agent | Context | Tokens | Cost | Timeline |
| --- | --- | --- | --- | --- |
| omp | yes | yes | yes, including subagents and side calls | yes |
| Claude Code | yes | yes | no | yes |
| Codex | yes | yes | no | yes |
| others | no | no | no | status only |

## Configuration

Both files live in the plugin's config directory (`herdr plugin config-dir herdr-trail`, also `$HERDR_PLUGIN_CONFIG_DIR`) and are read when the panel opens. Every key is optional; a key that can't be used is named in the header for the first 10 seconds and listed under `?`, and keeps its default.

### config.toml

The defaults:

```toml
mouse = false              # click to select, double-click to jump

[start]
view = "time"              # time | sequence | area | heat | dash  (presets 1-5)
orient = "h"               # h | v
remember = true            # restore the last mode, lens, density, sort, group and scope (prefs.toml)

[history]
window = "2h"              # log load, closed-lane pruning, cost/working windows, fit cap
retain_months = 3          # month log files kept; 0 keeps every one

[band]
split = "down"             # down | right
rows = 14                  # band height when split = "down"
columns = 50               # band width when split = "right"
follow = true              # pin the band and move it to the focused tab

[popup]
width = "80%"
height = "80%"

[filter]
quiet_after = "30m"        # how long an idle, done or unknown agent still counts as active for `a`

[notify]
# blocked_after = "5m"     # notify when an agent stays blocked this long; unset = off

[privacy]
max_text = "prompt"        # none | intent | prompt: the most `T` can reveal in the timeline
```

Durations take `s`, `m` or `h`.

### theme.toml

Every color comes from one set of tokens. Colors take a name (`green`, `darkgray`), `#rrggbb` or a 0-255 palette index; `background` and the `series` ramp need `#rrggbb` because fills are mixed from them.

`appearance` picks the base the other keys adjust, and is applied first: `"dark"` (the default), `"light"` (background `#f5f5f7`, dim `#6e6e73`, grid `#d2d2d7`, accent `blue`, and the whole ramp, `series_from = 0.0`, since its dark end reads on light), or `"auto"`, which reads `COLORFGBG` and picks light when its background field is `7` or `15`, dark otherwise or when unset.

The dark defaults:

```toml
appearance = "dark"      # dark | light | auto
background = "#1c1c1e"   # assumed terminal background; fills mix toward it
dim = "darkgray"         # captions, axes, closed lanes, help
grid = "#48484e"         # chart gridlines
accent = "cyan"          # active mode, notices
error = "red"            # socket, config and theme errors
fill_alpha = 0.3         # how much of a series' color its fill lays over what's behind
series_from = 0.40       # where along `series` the used stretch starts
# The one color ramp (viridis): each agent keeps a fixed spot along it from `series_from`, and HEAT
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

## Data and privacy

- **Read:** herdr's snapshot and event stream, and only the session files herdr attributes to agent panes, plus those sessions' subagent files. Nothing else on disk.
- **Stored:** an `[[events]]` recorder appends each pane lifecycle event (pane and workspace ids and labels, cwd, agent, state, session file path) to `events-YYYY-MM.jsonl` in the plugin state directory (`$HERDR_PLUGIN_STATE_DIR`, by default `~/.local/state/herdr/plugins/herdr-trail`), one file per UTC month. On a new month, files older than `history.retain_months` are deleted. The same directory holds `prefs.toml` (view settings) and `pinned-<session>` (the pinned band's terminal id). Nothing read from transcripts is ever written.
- **Shown:** prompt and intent text appears only in the timeline, only after `T`, and never past `privacy.max_text`.

## Troubleshooting

- `herdr plugin log list --plugin herdr-trail` shows the output of the install step, the recorder and the follow hook.
- The header shows `herdr not reachable at <socket>` when the panel can't talk to herdr; it keeps retrying.
- `herdr protocol N untested (built for 22)` means herdr is newer than this build; it keeps working but may miss fields.
- To force a source build (no download): `HERDR_TRAIL_BUILD=source sh scripts/install.sh` in the plugin directory.

## Uninstall

```
herdr plugin uninstall herdr-trail
rm -rf ~/.local/state/herdr/plugins/herdr-trail    # history log and prefs
```

Remove the keybindings from `config.toml`.

## Develop

```
cd plugins/trail
cargo test
HERDR_TRAIL_BUILD=source sh scripts/install.sh    # builds and copies to bin/, which the manifest runs
herdr plugin link "$PWD"                          # once; link runs no build step
./target/release/herdr-trail --dump --width 120 --height 20 [--view sequence|area|heat|dash] [--orient v] [--keys "j Tab"]
```

Run the install script after every change. `--dump` renders one frame against the running herdr to stdout, after pressing `--keys`.

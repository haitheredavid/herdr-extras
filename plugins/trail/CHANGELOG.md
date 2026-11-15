# Changelog

All notable changes to trail are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-11-14

First release as `herdr-trail` (formerly `track`).

### Added

- Prebuilt binaries for macOS and Linux (x86_64, aarch64), checked against `SHA256SUMS` on install, with a `cargo build` fallback; `--version`.
- Two modes: LANES (one lane per pane, with status, heat and ctx lenses, time or step axis) and USAGE (context in use per agent, stacked or overlaid).
- Inspector: the selected agent's task, location, state, context, cost, requests, working share and recent changes.
- Session TIMELINE (`I`): a scrubber plus every turn, model request, tool call and wait, with folding, match jumps, subagent drill-down and opt-in prompt/intent text (`T`).
- Session adapters for omp (context, tokens, cost including subagents and side calls), Claude Code and Codex (context and tokens).
- Stable per-agent colors along one viridis ramp, status shapes that read without color, and a one-line header with a blocked-agent badge.
- Filtering (`/` search, `a` active only, `c` closed, `w` scope), sorting (`s`), grouping by workspace or tab (`g`, `x` folds), `b` jumps to the next blocked agent.
- Size-aware layout: two-row lanes in narrow sidebars, the inspector beside or below, density control (`Z`).
- Optional mouse support, blocked-agent notifications (`notify.blocked_after`) and remembered view settings (`prefs.toml`).
- `config.toml` for start view, history window, log retention, band split and size, follow, popup size, filter and privacy knobs.
- Light theme (`appearance = "light"`, or `"auto"` from `COLORFGBG`).
- Per-herdr-session state: the pinned band and the history log follow `HERDR_SESSION`.
- Month log files older than `history.retain_months` are deleted.
- Clearer errors: the socket path when herdr is unreachable, a notice and resync when events are lost, resubscribe backoff, and a warning on an untested herdr protocol.

### Changed

- Renamed the plugin from `track` to `trail` (id `herdr-trail`).
- The band follows focus into the tab you switch to, keeping its view.

### Known issues

- Codex lanes can stay status-only with recent Codex releases: Codex runs its session hooks from a background process, so herdr's integration reports the session for the wrong pane and herdr never learns it. See the README's troubleshooting section.

//! Shared frame (one header line over the body), lane layout for both orientations, glyphs and keys.

mod area;
mod chrome;
mod dash;
mod gutter;
mod heat;
mod help;
mod sequence;
mod time;

use chrono::{Local, TimeZone};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::config::config_problems;
use crate::model::{AgentStatus, HISTORY_MS, Lane, Mark, Query, State};
use crate::session::{Roots, Store, fmt_tokens};
use crate::theme::{theme, theme_problems};
use chrome::Seg;
use gutter::{Cols, display_labels};

pub const BUCKETS_S: [i64; 10] = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600];
/// Cells across a lane when usage bands are drawn; the line sits higher the more context is in use.
const BAND: u16 = 4;
/// Config and theme problems stay in the header this long after start; `?` lists them for good.
const PROBLEMS_MS: i64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    Time,
    Sequence,
    /// Stacked context per agent over time.
    Area,
    /// Working share per agent per time bucket.
    Heat,
    /// One table row per agent.
    Dash,
}

impl ViewKind {
    /// In `Tab` order; `1`-`5` pick by position.
    const ALL: [ViewKind; 5] = [ViewKind::Time, ViewKind::Sequence, ViewKind::Area, ViewKind::Heat, ViewKind::Dash];

    /// Cells are time buckets: zoom and time scrolling apply.
    fn timed(self) -> bool {
        matches!(self, ViewKind::Time | ViewKind::Area | ViewKind::Heat)
    }

    /// `--view` name.
    fn name(self) -> &'static str {
        match self {
            ViewKind::Time => "time",
            ViewKind::Sequence => "sequence",
            ViewKind::Area => "area",
            ViewKind::Heat => "heat",
            ViewKind::Dash => "dash",
        }
    }

    /// Header tab.
    fn short(self) -> &'static str {
        match self {
            ViewKind::Sequence => "seq",
            v => v.name(),
        }
    }

    /// The view a `--view` / `[start] view` name picks.
    pub fn from_name(name: &str) -> Option<ViewKind> {
        ViewKind::ALL.iter().copied().find(|v| v.name() == name)
    }

    fn next(self) -> ViewKind {
        let i = ViewKind::ALL.iter().position(|&v| v == self).unwrap_or(0);
        ViewKind::ALL[(i + 1) % ViewKind::ALL.len()]
    }
}

/// How AREA lays its series out; `s` cycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaMode {
    /// Each series on the ones before it; the top line is the total.
    Stacked,
    /// Every series from zero, largest behind, overlaps blended.
    Overlaid,
    /// Stacked around a centered baseline (a streamgraph); thickness is the total.
    Stream,
}

impl AreaMode {
    fn name(self) -> &'static str {
        match self {
            AreaMode::Stacked => "stacked",
            AreaMode::Overlaid => "overlaid",
            AreaMode::Stream => "stream",
        }
    }

    fn next(self) -> AreaMode {
        match self {
            AreaMode::Stacked => AreaMode::Overlaid,
            AreaMode::Overlaid => AreaMode::Stream,
            AreaMode::Stream => AreaMode::Stacked,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orient {
    Horizontal,
    Vertical,
}

pub struct App {
    pub state: State,
    pub view: ViewKind,
    pub orient: Orient,
    pub bucket_idx: usize,
    /// Timed views pick the bucket that fits the history while following; `+`/`-` turn it off, `0` back on.
    pub bucket_auto: bool,
    /// Time view: right/bottom edge tracks now. Sequence view: no steps hidden from the end.
    pub follow: bool,
    pub view_end_ms: i64,
    /// Sequence view: steps hidden from the end of every lane.
    pub seq_back: usize,
    /// Id of the selected lane; it stays selected however the rows reorder.
    pub selected: Option<u64>,
    /// Row of the last selection, used while the selected lane isn't shown.
    sel_hint: usize,
    lane_scroll: usize,
    /// Rows scrolled out above, drawn, and scrolled out below in the last render.
    shown: (usize, usize, usize),
    /// Largest context on screen when the last render drew usage bands.
    band_max: Option<u64>,
    /// Time-axis cells in the last render; scroll steps are a quarter of it.
    time_cells: usize,
    pub query: Query,
    pub now_ms: i64,
    started_ms: i64,
    pub error: Option<String>,
    /// Last snapshot succeeded, so lanes reflect the live panes.
    pub connected: bool,
    /// Running as a popup: Esc closes, and jumping to an agent closes too.
    pub popup: bool,
    /// The `?` overlay is open.
    pub help: bool,
    pub quit: bool,
    pub refresh_requested: bool,
    /// Pane the user asked to jump to; the event loop sends `pane.focus`.
    pub focus_requested: Option<String>,
    /// Draw lanes as usage bands when any lane has usage.
    pub bands: bool,
    pub area_mode: AreaMode,
    pub sessions: Store,
}

impl App {
    pub fn new(state: State, view: ViewKind, orient: Orient, now_ms: i64) -> Self {
        App {
            state,
            view,
            orient,
            bucket_idx: 4,
            bucket_auto: true,
            follow: true,
            view_end_ms: now_ms,
            seq_back: 0,
            selected: None,
            sel_hint: 0,
            lane_scroll: 0,
            shown: (0, 0, 0),
            band_max: None,
            time_cells: 1,
            query: Query::default(),
            now_ms,
            started_ms: now_ms,
            error: None,
            connected: false,
            popup: false,
            help: false,
            quit: false,
            refresh_requested: false,
            focus_requested: None,
            bands: true,
            area_mode: AreaMode::Stacked,
            sessions: Store::new(Roots::from_env()),
        }
    }

    /// Reads new usage from every lane's agent sessions (at most once a second), and backdates
    /// first-seen lanes to when their session shows the agent started.
    pub fn refresh_usage(&mut self) {
        self.sessions.refresh(&self.state.lanes, self.now_ms);
        let since = self.now_ms - HISTORY_MS;
        let early: Vec<(usize, i64)> = (0..self.state.lanes.len())
            .filter_map(|i| {
                let lane = &self.state.lanes[i];
                let first = self.sessions.first_at(lane)?;
                (first < lane.opened_ms && first >= since).then_some((i, first))
            })
            .collect();
        for (i, t) in early {
            self.state.backdate(i, t);
        }
    }

    fn bucket_ms(&self) -> i64 {
        BUCKETS_S[self.bucket_idx] * 1000
    }

    /// While following with the auto bucket, the smallest bucket whose `n` cells reach back to the
    /// earliest lane (at most `HISTORY_MS`), so the strip holds the history and no empty future.
    fn fit_bucket(&mut self, lanes: &[usize], n: usize) {
        if !(self.bucket_auto && self.follow) || n == 0 {
            return;
        }
        let now = self.now_ms;
        let earliest = lanes.iter().map(|&i| self.state.lanes[i].opened_ms).min().unwrap_or(now).max(now - HISTORY_MS);
        self.bucket_idx = BUCKETS_S.iter().position(|&b| n as i64 * b * 1000 >= now - earliest).unwrap_or(BUCKETS_S.len() - 1);
    }

    /// The lanes the views show, in row order.
    fn rows(&self) -> Vec<usize> {
        self.state.visible_lanes(&self.query)
    }

    /// Highlighted row: the selected lane's, else the last selected row (clamped) while it's hidden.
    pub(crate) fn sel_row(&self, rows: &[usize]) -> Option<usize> {
        self.selected
            .and_then(|id| self.state.lane_by_id(id))
            .and_then(|li| rows.iter().position(|&i| i == li))
            .or_else(|| (!rows.is_empty()).then(|| self.sel_hint.min(rows.len() - 1)))
    }

    fn select_row(&mut self, rows: &[usize], r: usize) {
        let Some(&i) = rows.get(r) else { return };
        self.selected = Some(self.state.lanes[i].id);
        self.sel_hint = r;
    }

    fn selected_lane(&self) -> Option<usize> {
        let rows = self.rows();
        self.sel_row(&rows).map(|r| rows[r])
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        let c = key.code;
        if self.help {
            match c {
                KeyCode::Char('?') | KeyCode::Esc => self.help = false,
                KeyCode::Char('q') => self.quit = true,
                _ => {}
            }
            return;
        }
        // DASH is a table in either orientation, so its lane keys stay j/k.
        let horizontal = self.orient == Orient::Horizontal || self.view == ViewKind::Dash;
        let (next, prev, back, fwd) = if horizontal {
            (
                matches!(c, KeyCode::Char('j') | KeyCode::Down),
                matches!(c, KeyCode::Char('k') | KeyCode::Up),
                matches!(c, KeyCode::Char('h') | KeyCode::Left),
                matches!(c, KeyCode::Char('l') | KeyCode::Right),
            )
        } else {
            (
                matches!(c, KeyCode::Char('l') | KeyCode::Right),
                matches!(c, KeyCode::Char('h') | KeyCode::Left),
                matches!(c, KeyCode::Char('k') | KeyCode::Up),
                matches!(c, KeyCode::Char('j') | KeyCode::Down),
            )
        };
        if next || prev {
            let rows = self.rows();
            if let Some(r) = self.sel_row(&rows) {
                let r = if next { (r + 1).min(rows.len() - 1) } else { r.saturating_sub(1) };
                self.select_row(&rows, r);
            }
            return;
        }
        let step = (self.time_cells as i64 / 4).max(1) * self.bucket_ms();
        if back {
            match self.view {
                ViewKind::Time | ViewKind::Area | ViewKind::Heat => {
                    if self.follow {
                        self.view_end_ms = self.now_ms;
                    }
                    self.view_end_ms -= step;
                    self.follow = false;
                }
                ViewKind::Sequence => {
                    self.seq_back += 1;
                    self.follow = false;
                }
                ViewKind::Dash => {}
            }
            return;
        }
        if fwd {
            match self.view {
                ViewKind::Dash => {}
                ViewKind::Time | ViewKind::Area | ViewKind::Heat => {
                    self.view_end_ms += step;
                    if self.view_end_ms >= self.now_ms {
                        self.follow = true;
                    }
                }
                ViewKind::Sequence => {
                    self.seq_back = self.seq_back.saturating_sub(1);
                    self.follow = self.seq_back == 0;
                }
            }
            return;
        }
        match c {
            KeyCode::Tab => self.view = self.view.next(),
            KeyCode::Char(c @ '1'..='5') => self.view = ViewKind::ALL[c as usize - '1' as usize],
            KeyCode::Char('o') => {
                self.orient = match self.orient {
                    Orient::Horizontal => Orient::Vertical,
                    Orient::Vertical => Orient::Horizontal,
                };
                self.lane_scroll = 0;
            }
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc if self.popup => self.quit = true,
            KeyCode::Char('r') => self.refresh_requested = true,
            KeyCode::Char('c') => self.query.show_closed = !self.query.show_closed,
            KeyCode::Char('s') if self.view == ViewKind::Area => self.area_mode = self.area_mode.next(),
            KeyCode::Char('u') => {
                self.bands = !self.bands;
                self.lane_scroll = 0;
            }
            KeyCode::Enter => self.focus_requested = self.selected_lane().map(|i| self.state.lanes[i].pane_id.clone()),
            KeyCode::Char('f') => {
                self.follow = true;
                self.seq_back = 0;
            }
            KeyCode::Char('0') => {
                self.bucket_auto = true;
                self.follow = true;
                self.seq_back = 0;
            }
            KeyCode::Char('+') | KeyCode::Char('=') if self.view.timed() => {
                self.bucket_auto = false;
                self.bucket_idx = self.bucket_idx.saturating_sub(1);
            }
            KeyCode::Char('-') if self.view.timed() => {
                self.bucket_auto = false;
                self.bucket_idx = (self.bucket_idx + 1).min(BUCKETS_S.len() - 1);
            }
            _ => {}
        }
    }
}

/// Shape of a status, so state reads without color.
pub fn status_glyph(s: AgentStatus) -> char {
    match s {
        AgentStatus::Working => '●',
        AgentStatus::Blocked => '▲',
        AgentStatus::Done => '✓',
        AgentStatus::Idle => '○',
        AgentStatus::Unknown => '·',
    }
}

pub fn line_glyph(s: AgentStatus, orient: Orient) -> char {
    match (s, orient) {
        (AgentStatus::Working | AgentStatus::Done, Orient::Horizontal) => '━',
        (AgentStatus::Blocked, Orient::Horizontal) => '╍',
        (AgentStatus::Idle, Orient::Horizontal) => '─',
        (AgentStatus::Unknown, Orient::Horizontal) => '┈',
        (AgentStatus::Working | AgentStatus::Done, Orient::Vertical) => '┃',
        (AgentStatus::Blocked, Orient::Vertical) => '╏',
        (AgentStatus::Idle, Orient::Vertical) => '│',
        (AgentStatus::Unknown, Orient::Vertical) => '┆',
    }
}

pub fn node_glyph(mark: Mark) -> (char, Color) {
    match mark {
        Mark::Created => ('◆', theme().created),
        Mark::Seen => ('◇', theme().seen),
        Mark::Status(s) => (status_glyph(s), theme().status(s)),
        Mark::Closed | Mark::Exited => ('✕', theme().closed),
    }
}

fn head_glyph(orient: Orient) -> char {
    match orient {
        Orient::Horizontal => '▸',
        Orient::Vertical => '▾',
    }
}

/// One cell along a lane's time axis.
type Cell = Option<(char, Color)>;

/// One position along a lane: what to draw, how far across the band it sits (0 = least context),
/// and the status of the line through it, which styles any bend into it.
#[derive(Debug, Clone, Copy)]
struct Pt {
    cell: Cell,
    level: u8,
    status: Option<AgentStatus>,
}

/// Maps context tokens to a level across the band, against the largest context on screen.
#[derive(Debug, Clone, Copy)]
struct Scale {
    band: u16,
    max: u64,
}

impl Scale {
    /// Bands only when they're on, there is usage, lanes are rows, and every lane fits in `rows`
    /// as a band; otherwise lanes stay one row each.
    fn of(app: &App, lanes: &[usize], rows: u16) -> Scale {
        let max = lanes
            .iter()
            .filter_map(|&i| {
                let l = &app.state.lanes[i];
                app.sessions.max_between(l, l.opened_ms, l.closed_ms.unwrap_or(app.now_ms))
            })
            .max()
            .unwrap_or(0);
        let fits = lanes.len() * usize::from(BAND + 1) <= usize::from(rows) + 1;
        let band = if app.bands && max > 0 && app.orient == Orient::Horizontal && fits { BAND } else { 1 };
        Scale { band, max }
    }

    fn level(self, ctx: Option<u64>) -> u8 {
        if self.band <= 1 || self.max == 0 {
            return 0;
        }
        let top = u64::from(self.band - 1);
        ((ctx.unwrap_or(0).min(self.max) * top + self.max / 2) / self.max) as u8
    }
}

fn is_line(ch: char) -> bool {
    matches!(ch, '━' | '─' | '┈' | '┃' | '│' | '┆' | '╍' | '╏')
}

/// Corner at the old level, run between, corner at the new level, for a line stepping across the
/// band; `up` means toward more context (up in ↔, right in ↕).
fn bend(orient: Orient, up: bool, heavy: bool) -> (char, char, char) {
    match (orient, up, heavy) {
        (Orient::Horizontal, true, false) => ('┘', '│', '┌'),
        (Orient::Horizontal, true, true) => ('┛', '┃', '┏'),
        (Orient::Horizontal, false, false) => ('┐', '│', '└'),
        (Orient::Horizontal, false, true) => ('┓', '┃', '┗'),
        (Orient::Vertical, true, false) => ('└', '─', '┐'),
        (Orient::Vertical, true, true) => ('┗', '━', '┓'),
        (Orient::Vertical, false, false) => ('┘', '─', '┌'),
        (Orient::Vertical, false, true) => ('┛', '━', '┏'),
    }
}

fn line_cell(s: AgentStatus, orient: Orient) -> Cell {
    Some((line_glyph(s, orient), theme().status(s)))
}

fn head_cell(s: AgentStatus, orient: Orient) -> Cell {
    Some((head_glyph(orient), theme().status(s)))
}

fn mark_rank(m: Mark) -> u8 {
    match m {
        Mark::Closed | Mark::Exited => 3,
        Mark::Created | Mark::Seen => 2,
        Mark::Status(_) => 1,
    }
}

/// Node for several steps sharing one cell: lifecycle marks win, ties go to the newest, and a
/// cell holding more than one status change draws `◉`.
fn node_cell(steps: &[crate::model::Step]) -> Cell {
    let (_, winner) = steps.iter().enumerate().max_by_key(|(i, s)| (mark_rank(s.mark), *i))?;
    let (ch, color) = node_glyph(winner.mark);
    let statuses = steps.iter().filter(|s| matches!(s.mark, Mark::Status(_))).count();
    Some((if matches!(winner.mark, Mark::Status(_)) && statuses >= 2 { '◉' } else { ch }, color))
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Writes `s` at (x, y) clipped to `area`; returns the x after the last cell.
pub fn put_str(buf: &mut Buffer, area: Rect, x: u16, y: u16, s: &str, style: Style) -> u16 {
    let mut cx = x;
    if y < area.y || y >= area.bottom() {
        return cx + s.chars().count() as u16;
    }
    for ch in s.chars() {
        if cx >= area.x && cx < area.right() {
            buf[(cx, y)].set_char(ch).set_style(style);
        }
        cx += 1;
    }
    cx
}

fn local(ms: i64, format: &str) -> String {
    Local.timestamp_millis_opt(ms).single().map_or_else(|| "--:--".into(), |t| t.format(format).to_string())
}

fn hms(ms: i64) -> String {
    local(ms, "%H:%M:%S")
}

fn hm(ms: i64) -> String {
    local(ms, "%H:%M")
}

/// Round steps for time-axis ticks, in seconds.
const TICK_STEPS_S: [i64; 15] = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 21600];

/// Ticks for a time axis of `cells` cells of `cell_ms` each from `start_ms`, as (cell, label):
/// the smallest round step whose labels keep apart (a label plus `┊` and a gap across ↔, four rows
/// down ↕), on multiples of the step in local time. Labels read `HH:MM`, with seconds only when
/// the step is under a minute.
fn time_ticks(start_ms: i64, cell_ms: f64, cells: usize, horizontal: bool) -> Vec<(usize, String)> {
    let offset_ms = Local.timestamp_millis_opt(start_ms).single().map_or(0, |t| i64::from(t.offset().local_minus_utc()) * 1000);
    let fits = |step_s: i64| {
        let label_w = if step_s < 60 { 8 } else { 5 };
        let gap = if horizontal { label_w + 3 } else { 4 };
        (step_s * 1000) as f64 >= gap as f64 * cell_ms
    };
    let step_s = TICK_STEPS_S.iter().copied().find(|&s| fits(s)).unwrap_or(TICK_STEPS_S[TICK_STEPS_S.len() - 1]);
    let step = step_s * 1000;
    let format = if step_s < 60 { "%H:%M:%S" } else { "%H:%M" };
    let first = (start_ms + offset_ms + step - 1).div_euclid(step) * step - offset_ms;
    (0..)
        .map(|i| first + i * step)
        .map(|t| (((t - start_ms) as f64 / cell_ms).floor() as usize, t))
        .take_while(|&(cell, _)| cell < cells)
        .map(|(cell, t)| (cell, local(t, format)))
        .collect()
}

fn fmt_bucket(s: i64) -> String {
    if s >= 60 { format!("{}m", s / 60) } else { format!("{s}s") }
}

/// `42s`, `3m05s`, `2h07m`.
fn fmt_dur(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m{:02}s", s / 60, s % 60),
        _ => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
    }
}

/// Lane placement in the body. `lead` is what precedes the lanes across the time axis: the axis
/// row (↔ time views) or the axis gutter (↕ time views). `band` is each lane's thickness; bands
/// are separated by a blank row in ↔ (columns already have spare cells in ↕).
struct Grid {
    orient: Orient,
    body: Rect,
    lead: u16,
    band: u16,
    /// ↔ gutter columns.
    cols: Cols,
    /// ↕ column width.
    col_w: u16,
}

impl Grid {
    fn new(orient: Orient, body: Rect, lead: u16, band: u16, lanes: usize) -> Grid {
        let col_w = (body.width.saturating_sub(lead) / lanes.max(1) as u16).clamp(6, 18);
        Grid { orient, body, lead, band, cols: Cols::fit(body.width), col_w }
    }

    /// Cells along the time axis.
    fn n(&self) -> usize {
        match self.orient {
            Orient::Horizontal => self.cols.strip(self.body.width) as usize,
            Orient::Vertical => self.body.height.saturating_sub(1) as usize,
        }
    }

    fn gap(&self) -> u16 {
        u16::from(self.band > 1)
    }

    /// Lanes that fit side by side.
    fn slots(&self) -> usize {
        match self.orient {
            Orient::Horizontal => ((self.body.height.saturating_sub(self.lead) + self.gap()) / (self.band + self.gap())) as usize,
            Orient::Vertical => (self.body.width.saturating_sub(self.lead) / self.col_w) as usize,
        }
    }

    /// First row (↔) or column (↕) of lane `slot`.
    fn lane_start(&self, slot: usize) -> u16 {
        match self.orient {
            Orient::Horizontal => self.body.y + self.lead + slot as u16 * (self.band + self.gap()),
            Orient::Vertical => self.body.x + self.lead + slot as u16 * self.col_w,
        }
    }

    fn cell_pos(&self, slot: usize, i: usize, level: u8) -> (u16, u16) {
        let start = self.lane_start(slot);
        match self.orient {
            Orient::Horizontal => (self.body.x + self.cols.left() + i as u16, start + (self.band - 1 - u16::from(level))),
            Orient::Vertical => (start + 2, self.body.y + 1 + i as u16),
        }
    }

    /// Where the time-axis label for cell `i` starts.
    fn axis_pos(&self, i: usize) -> (u16, u16) {
        match self.orient {
            Orient::Horizontal => (self.body.x + self.cols.left() + i as u16, self.body.y),
            Orient::Vertical => (self.body.x, self.body.y + 1 + i as u16),
        }
    }

    /// Where time-axis labels may draw: the strip's span (↔) or the axis gutter (↕).
    fn axis_area(&self) -> Rect {
        match self.orient {
            Orient::Horizontal => Rect::new(self.body.x + self.cols.left(), self.body.y, self.n() as u16, 1),
            Orient::Vertical => Rect::new(self.body.x, self.body.y, self.lead, self.body.height),
        }
    }

    fn put(&self, buf: &mut Buffer, (x, y): (u16, u16), ch: char, color: Color) {
        if x < self.body.right() && y < self.body.bottom() {
            buf[(x, y)].set_char(ch).set_style(Style::new().fg(color));
        }
    }

    /// Draws each lane's gutter (↔) or column header (↕) and its line, scrolling so the selected
    /// lane stays in view. Where the level changes between cells, the line bends across the band
    /// in the new cell.
    fn draw_lanes(&self, buf: &mut Buffer, app: &mut App, lanes: &[usize], pts: impl Fn(&Lane, &Store, usize) -> Vec<Pt>) {
        let slots = self.slots();
        let sel = app.sel_row(lanes);
        let s = sel.unwrap_or(0);
        app.lane_scroll = app.lane_scroll.min(lanes.len().saturating_sub(slots));
        if s < app.lane_scroll {
            app.lane_scroll = s;
        } else if slots > 0 && s >= app.lane_scroll + slots {
            app.lane_scroll = s + 1 - slots;
        }
        let shown = slots.min(lanes.len() - app.lane_scroll);
        app.shown = (app.lane_scroll, shown, lanes.len() - app.lane_scroll - shown);
        let labels = display_labels(&app.state, lanes);
        let n = self.n();
        let dim = Style::new().fg(theme().dim);
        for (slot, (k, &li)) in lanes.iter().enumerate().skip(app.lane_scroll).take(slots).enumerate() {
            let lane = &app.state.lanes[li];
            let selected = Some(k) == sel;
            match self.orient {
                Orient::Horizontal => {
                    gutter::draw_row(buf, self.body, &self.cols, app, li, &labels[k], self.lane_start(slot), self.band, selected)
                }
                Orient::Vertical => {
                    let x = self.lane_start(slot);
                    let sel_style = if selected { Style::new().add_modifier(Modifier::REVERSED) } else { Style::new() };
                    let (glyph, gs) = if lane.is_open() {
                        (status_glyph(lane.status), Style::new().fg(theme().status(lane.status)))
                    } else {
                        ('✕', dim)
                    };
                    put_str(buf, self.body, x, self.body.y, &glyph.to_string(), gs.patch(sel_style));
                    let w = usize::from(self.col_w - 2);
                    let label_style = if lane.is_open() { Style::new() } else { dim };
                    put_str(
                        buf,
                        self.body,
                        x + 1,
                        self.body.y,
                        &format!("{:<w$}", truncate(&labels[k], w)),
                        label_style.patch(sel_style),
                    );
                }
            }
            // Lines, bends, nodes and the head all take status colors.
            let mut prev: Option<u8> = None;
            for (i, pt) in pts(lane, &app.sessions, n).into_iter().enumerate().take(n) {
                let Some((mut ch, mut color)) = pt.cell else {
                    prev = None;
                    continue;
                };
                if let Some(p) = prev
                    && p != pt.level
                {
                    let s = pt.status.unwrap_or_default();
                    let heavy = matches!(s, AgentStatus::Working | AgentStatus::Blocked | AgentStatus::Done);
                    let c = theme().status(s);
                    let (from, mid, to) = bend(self.orient, pt.level > p, heavy);
                    self.put(buf, self.cell_pos(slot, i, p), from, c);
                    for l in p.min(pt.level) + 1..p.max(pt.level) {
                        self.put(buf, self.cell_pos(slot, i, l), mid, c);
                    }
                    if is_line(ch) {
                        (ch, color) = (to, c);
                    }
                }
                self.put(buf, self.cell_pos(slot, i, pt.level), ch, color);
                prev = Some(pt.level);
            }
        }
    }
}

/// Header segments, by priority: error, views, counts, paused, selected, scale, problems, help.
fn header(app: &App, lanes: &[usize]) -> Vec<Seg> {
    let dim = Style::new().fg(theme().dim);
    let error = Style::new().fg(theme().error);
    let mut segs = Vec::new();
    if let Some(err) = &app.error {
        segs.push(Seg::left(0, vec![Span::styled(format!("⚠ herdr: {err} — retrying"), error)]));
    }

    let mut views = Vec::new();
    for (i, v) in ViewKind::ALL.iter().enumerate() {
        if i > 0 {
            views.push(Span::raw(" "));
        }
        views.push(if *v == app.view {
            Span::styled(v.short().to_uppercase(), Style::new().fg(theme().accent).add_modifier(Modifier::BOLD))
        } else {
            Span::styled(v.short(), dim)
        });
    }
    if app.orient == Orient::Vertical {
        views.push(Span::raw(" ↕"));
    }
    segs.push(Seg::left(1, views));

    let (above, shown, below) = app.shown;
    let mut counts = format!("{shown}/{}", lanes.len());
    if above > 0 {
        counts += &format!(" ↑{above}");
    }
    if below > 0 {
        counts += &format!(" ↓{below}");
    }
    segs.push(Seg::left(2, vec![Span::raw(counts)]));

    let paused = match app.view {
        _ if app.follow => None,
        ViewKind::Sequence => Some(format!("⏸ −{} · f", app.seq_back)),
        ViewKind::Dash => None,
        _ => Some(format!("⏸ {} · f", hms(app.view_end_ms))),
    };
    if let Some(p) = paused {
        segs.push(Seg::left(3, vec![Span::styled(p, Style::new().fg(theme().accent))]));
    }

    let fit = app.bucket_auto && app.follow;
    let bucket = fmt_bucket(BUCKETS_S[app.bucket_idx]);
    let mut scale = match app.view {
        ViewKind::Time | ViewKind::Heat if fit => format!("fit {bucket}"),
        ViewKind::Time | ViewKind::Heat => bucket,
        ViewKind::Area if fit => "fit".into(),
        ViewKind::Area => bucket,
        ViewKind::Sequence | ViewKind::Dash => String::new(),
    };
    if let Some(max) = app.band_max {
        if !scale.is_empty() {
            scale += " · ";
        }
        scale += &format!("ctx≤{}", fmt_tokens(max));
    }
    if !scale.is_empty() {
        segs.push(Seg::left(5, vec![Span::styled(scale, dim)]));
    }

    if app.now_ms - app.started_ms < PROBLEMS_MS {
        let mut spans = Vec::new();
        for (file, problems) in [("theme.toml", theme_problems()), ("config.toml", config_problems())] {
            if let [first, rest @ ..] = problems {
                let more = if rest.is_empty() { String::new() } else { format!(" (+{} more)", rest.len()) };
                if !spans.is_empty() {
                    spans.push(Span::styled(" · ", dim));
                }
                spans.push(Span::styled(format!("{file} {first}{more}"), error));
            }
        }
        if !spans.is_empty() {
            segs.push(Seg::left(6, spans));
        }
    }

    if let Some(li) = app.sel_row(lanes).map(|r| lanes[r]) {
        let lane = &app.state.lanes[li];
        segs.push(Seg::right(
            4,
            vec![
                Span::raw(lane.label().to_string()),
                Span::styled(format!(" · {} · {}", app.state.workspace_label(li), lane.agent.as_deref().unwrap_or("-")), dim),
            ],
        ));
    }
    segs.push(Seg::right(7, vec![Span::styled("? keys", dim)]));
    segs
}

pub fn draw(buf: &mut Buffer, area: Rect, app: &mut App) {
    let [header_area, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    let dim = Style::new().fg(theme().dim);
    let lanes = app.rows();
    if app.view.timed() && app.follow {
        app.view_end_ms = app.now_ms;
    }
    app.band_max = None;
    app.shown = (0, lanes.len(), 0);

    // Closed lanes fold into one summary row at the bottom (AREA has no rows to fold).
    let hidden = app.state.closed_hidden(&app.query);
    let summary = hidden > 0 && app.view != ViewKind::Area && body.height > 1;
    let lane_body = if summary { Rect { height: body.height - 1, ..body } } else { body };
    if lanes.is_empty() {
        if hidden == 0 {
            put_str(buf, body, body.x, body.y, "no agents yet — start one in a herdr pane", dim);
        }
    } else {
        match app.view {
            ViewKind::Time => time::render(buf, lane_body, app, &lanes),
            ViewKind::Sequence => sequence::render(buf, lane_body, app, &lanes),
            ViewKind::Area => area::render(buf, lane_body, app, &lanes),
            ViewKind::Heat => heat::render(buf, lane_body, app, &lanes),
            ViewKind::Dash => dash::render(buf, lane_body, app, &lanes),
        }
    }
    if summary {
        let text = format!("▸ {hidden} closed in the last {}h — c shows them", HISTORY_MS / 3_600_000);
        put_str(buf, body, body.x, body.bottom() - 1, &text, dim);
    }

    chrome::draw_header(buf, header_area, header(app, &lanes));
    if app.help {
        help::draw(buf, body);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::Step;
    use ratatui::crossterm::event::KeyModifiers;

    /// Lane opened at 0 (Created, idle), working at 2000, blocked at 5000; `now_ms = 8500`.
    pub fn sample_app(view: ViewKind, orient: Orient) -> App {
        let mut state = State::default();
        let i = state.open("w1:p1", "w1", 0, Mark::Created, AgentStatus::Idle, true);
        state.lanes[i].agent = Some("omp".into());
        state.set_status(i, AgentStatus::Working, 2000);
        state.set_status(i, AgentStatus::Blocked, 5000);
        state.assign_slots();
        App::new(state, view, orient, 8500)
    }

    pub fn row(buf: &Buffer, y: u16, x0: u16, x1: u16) -> String {
        (x0..x1).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string()
    }

    pub fn col(buf: &Buffer, x: u16, y0: u16, y1: u16) -> Vec<String> {
        (y0..y1).map(|y| buf[(x, y)].symbol().to_string()).collect()
    }

    pub fn render(app: &mut App, w: u16, h: u16) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
        draw(&mut buf, Rect::new(0, 0, w, h), app);
        buf
    }

    pub fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn key(app: &mut App, c: char) {
        press(app, KeyCode::Char(c));
    }

    fn add_lane(app: &mut App, pane: &str, status: AgentStatus) -> usize {
        let i = app.state.open(pane, "w1", 100, Mark::Seen, AgentStatus::Unknown, true);
        app.state.lanes[i].agent = Some("omp".into());
        app.state.set_status(i, status, 200);
        app.state.assign_slots();
        i
    }

    fn two_lanes(orient: Orient) -> App {
        let mut app = sample_app(ViewKind::Time, orient);
        add_lane(&mut app, "w1:p2", AgentStatus::Idle);
        app
    }

    fn sel(app: &App) -> Option<usize> {
        app.sel_row(&app.rows())
    }

    #[test]
    fn vertical_keys_swap_lane_and_time_axes() {
        let mut app = two_lanes(Orient::Vertical);
        key(&mut app, 'l');
        assert_eq!(sel(&app), Some(1));
        key(&mut app, 'k');
        assert!(!app.follow);
        let back = app.view_end_ms;
        assert!(back < app.now_ms);
        key(&mut app, 'j');
        assert!(app.view_end_ms > back);
        assert_eq!(sel(&app), Some(1), "time keys leave the selection alone");
    }

    #[test]
    fn horizontal_j_selects_next_lane() {
        let mut app = two_lanes(Orient::Horizontal);
        key(&mut app, 'j');
        assert_eq!(sel(&app), Some(1));
        key(&mut app, 'j');
        assert_eq!(sel(&app), Some(1), "clamped");
        assert!(app.follow);
    }

    #[test]
    fn selection_stays_on_its_lane_when_a_new_lane_sorts_before_it() {
        let mut state = State::default();
        let b = state.open("b:p1", "b", 0, Mark::Seen, AgentStatus::Idle, true);
        state.lanes[b].agent = Some("omp".into());
        let mut app = App::new(state, ViewKind::Time, Orient::Horizontal, 1000);
        let rows = app.rows();
        app.select_row(&rows, 0);
        let id = app.selected;
        let a = app.state.open("a:p1", "a", 10, Mark::Seen, AgentStatus::Idle, true);
        app.state.lanes[a].agent = Some("omp".into());
        assert_eq!(app.selected, id);
        assert_eq!(sel(&app), Some(1), "the highlight follows the lane to its new row");
    }

    #[test]
    fn every_status_has_its_own_shape() {
        use AgentStatus::*;
        let mut state = State::default();
        for (k, s) in [Working, Blocked, Done, Idle, Unknown].into_iter().enumerate() {
            let i = state.open(&format!("w1:p{k}"), "w1", k as i64, Mark::Seen, Unknown, true);
            state.lanes[i].agent = Some("omp".into());
            state.set_status(i, s, 1000);
        }
        state.assign_slots();
        let mut app = App::new(state, ViewKind::Time, Orient::Horizontal, 8500);
        let buf = render(&mut app, 60, 8);
        let glyphs: Vec<String> = (2..7).map(|y| buf[(1, y)].symbol().to_string()).collect();
        assert_eq!(glyphs, ["●", "▲", "✓", "○", "·"]);
        assert!(row(&buf, 3, 33, 60).contains('╍'), "{}", row(&buf, 3, 33, 60));
    }

    #[test]
    fn every_agent_row_shows_its_state() {
        let mut app = two_lanes(Orient::Horizontal);
        let buf = render(&mut app, 80, 6);
        assert!(row(&buf, 2, 0, 33).contains("blocked 3s"), "{}", row(&buf, 2, 0, 33));
        assert!(row(&buf, 3, 0, 33).contains("idle 8s"), "{}", row(&buf, 3, 0, 33));
    }

    #[test]
    fn closed_lanes_fold_into_a_summary_row() {
        let mut app = two_lanes(Orient::Horizontal);
        app.state.close("w1:p2", Mark::Closed, 8000);
        let buf = render(&mut app, 80, 6);
        assert!(row(&buf, 5, 0, 80).starts_with("▸ 1 closed"), "{}", row(&buf, 5, 0, 80));
        assert_eq!(row(&buf, 3, 0, 80), "", "the closed lane's row is gone");
        key(&mut app, 'c');
        let buf = render(&mut app, 80, 6);
        assert!(row(&buf, 3, 0, 33).contains("closed"), "{}", row(&buf, 3, 0, 33));
        assert!(!row(&buf, 5, 0, 80).starts_with('▸'));
    }

    #[test]
    fn hidden_lanes_are_counted() {
        let mut app = sample_app(ViewKind::Time, Orient::Horizontal);
        for k in 2..=6 {
            add_lane(&mut app, &format!("w1:p{k}"), AgentStatus::Idle);
        }
        let buf = render(&mut app, 80, 5);
        assert!(row(&buf, 0, 0, 80).contains("3/6 ↓3"), "{}", row(&buf, 0, 0, 80));
    }

    #[test]
    fn header_is_one_line_of_views() {
        let mut app = sample_app(ViewKind::Time, Orient::Horizontal);
        let buf = render(&mut app, 120, 4);
        let header = row(&buf, 0, 0, 120);
        assert!(header.starts_with("TIME seq area heat dash"), "{header}");
        assert!(header.ends_with("? keys"), "{header}");
    }

    #[test]
    fn help_overlay_opens_and_closes() {
        let mut app = sample_app(ViewKind::Time, Orient::Horizontal);
        key(&mut app, '?');
        let buf = render(&mut app, 100, 30);
        let body: String = (1..30).map(|y| row(&buf, y, 0, 100)).collect::<Vec<_>>().join("\n");
        assert!(body.contains(" keys ") && body.contains("quit"), "{body}");
        key(&mut app, 'j');
        assert!(app.help, "other keys do nothing while help is open");
        press(&mut app, KeyCode::Esc);
        assert!(!app.help);
        assert!(!app.quit);
    }

    #[test]
    fn following_fits_the_bucket_to_the_history() {
        let mut app = sample_app(ViewKind::Time, Orient::Horizontal);
        app.now_ms = 600_000;
        render(&mut app, 60, 4);
        // 27 cells over 600 s: 30 s buckets reach back far enough, 15 s don't.
        assert_eq!(BUCKETS_S[app.bucket_idx], 30);
        key(&mut app, '-');
        assert!(!app.bucket_auto);
        assert_eq!(BUCKETS_S[app.bucket_idx], 60);
        key(&mut app, '0');
        render(&mut app, 60, 4);
        assert_eq!(BUCKETS_S[app.bucket_idx], 30);
    }

    #[test]
    fn durations_read_short() {
        assert_eq!(fmt_dur(42_000), "42s");
        assert_eq!(fmt_dur(185_000), "3m05s");
        assert_eq!(fmt_dur(7_620_000), "2h07m");
    }

    #[test]
    fn time_ticks_pick_round_steps_that_keep_labels_apart() {
        // 15 s cells across ↔: `┊HH:MM` plus a gap needs 8 cells, so ticks every 2 minutes.
        let start = 1_794_529_561_427;
        let ticks = time_ticks(start, 15_000.0, 100, true);
        assert!(ticks.len() >= 11, "{ticks:?}");
        assert!(ticks.windows(2).all(|w| w[1].0 - w[0].0 == 8), "{ticks:?}");
        assert!(ticks.iter().all(|(_, l)| l.len() == 5 && l[3..].parse::<u32>().unwrap() % 2 == 0), "{ticks:?}");
        // 1 s cells: seconds show, every 15 s (8 + 3 cells needed).
        let ticks = time_ticks(start, 1000.0, 60, true);
        assert!(ticks.iter().all(|(_, l)| l.len() == 8 && l[6..].parse::<u32>().unwrap() % 15 == 0), "{ticks:?}");
        // Down ↕ only rows matter: 4 rows of 15 s → every minute.
        let ticks = time_ticks(start, 15_000.0, 40, false);
        assert!(ticks.windows(2).all(|w| w[1].0 - w[0].0 == 4), "{ticks:?}");
    }

    #[test]
    fn crowded_cell_prefers_lifecycle_then_counts_statuses() {
        use AgentStatus::*;
        let s = |at_ms, mark, status| Step { at_ms, mark, status };
        let both = [s(0, Mark::Status(Working), Working), s(1, Mark::Status(Blocked), Blocked)];
        assert_eq!(node_cell(&both), Some(('◉', theme().blocked)));
        let closed = [s(0, Mark::Status(Working), Working), s(1, Mark::Closed, Working)];
        assert_eq!(node_cell(&closed), Some(('✕', theme().closed)));
        assert_eq!(node_cell(&[]), None);
    }
}

//! Shared frame (header, detail, footer), lane layout for both orientations, glyphs and keys.

mod area;
mod sequence;
mod time;

use chrono::{Local, TimeZone};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::model::{AgentStatus, Lane, Mark, State};
use crate::usage::{Tracker, fmt_tokens};

pub const BUCKETS_S: [i64; 5] = [1, 5, 15, 60, 300];
/// Horizontal lane gutter: status glyph, label, workspace.
const GUTTER: u16 = 22;
/// Vertical lane column width.
const COL_W: u16 = 6;
/// Cells across a lane when usage bands are drawn; the line sits higher the more context is in use.
const BAND: u16 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    Time,
    Sequence,
    /// Stacked context per agent over time.
    Area,
}

impl ViewKind {
    /// Cells are time buckets: zoom and time scrolling apply.
    fn timed(self) -> bool {
        matches!(self, ViewKind::Time | ViewKind::Area)
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
    /// Time view: right/bottom edge tracks now. Sequence view: no steps hidden from the end.
    pub follow: bool,
    pub view_end_ms: i64,
    /// Sequence view: steps hidden from the end of every lane.
    pub seq_back: usize,
    /// Index into `state.visible_lanes()`.
    pub selected: usize,
    lane_scroll: usize,
    /// Time-axis cells in the last render; scroll steps are a quarter of it.
    time_cells: usize,
    pub now_ms: i64,
    pub error: Option<String>,
    /// Last snapshot succeeded, so lanes reflect the live panes.
    pub connected: bool,
    /// Running as a popup: Esc closes, and jumping to an agent closes too.
    pub popup: bool,
    pub quit: bool,
    pub refresh_requested: bool,
    /// Pane the user asked to jump to; the event loop sends `pane.focus`.
    pub focus_requested: Option<String>,
    /// Draw lanes as usage bands when any lane has usage.
    pub bands: bool,
    /// Area view: series stacked on each other, else overlaid from zero.
    pub area_stacked: bool,
    /// Area view while following: stretch the history across the time axis instead of buckets.
    pub area_fit: bool,
    pub usage: Tracker,
}

impl App {
    pub fn new(state: State, view: ViewKind, orient: Orient, now_ms: i64) -> Self {
        App {
            state,
            view,
            orient,
            bucket_idx: 2,
            follow: true,
            view_end_ms: now_ms,
            seq_back: 0,
            selected: 0,
            lane_scroll: 0,
            time_cells: 1,
            now_ms,
            error: None,
            connected: false,
            popup: false,
            quit: false,
            refresh_requested: false,
            focus_requested: None,
            bands: true,
            area_stacked: true,
            area_fit: true,
            usage: Tracker::default(),
        }
    }

    /// Reads new usage from every lane's agent session files.
    pub fn refresh_usage(&mut self) {
        let App { state, usage, .. } = self;
        usage.refresh(state.lanes.iter().flat_map(|l| &l.sessions));
    }

    fn bucket_ms(&self) -> i64 {
        BUCKETS_S[self.bucket_idx] * 1000
    }

    fn selected_lane(&self) -> Option<usize> {
        let lanes = self.state.visible_lanes();
        lanes.get(self.selected.min(lanes.len().saturating_sub(1))).copied()
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        let horizontal = self.orient == Orient::Horizontal;
        let c = key.code;
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
        if next {
            let n = self.state.visible_lanes().len();
            self.selected = (self.selected + 1).min(n.saturating_sub(1));
            return;
        }
        if prev {
            self.selected = self.selected.saturating_sub(1);
            return;
        }
        let step = (self.time_cells as i64 / 4).max(1) * self.bucket_ms();
        if back {
            match self.view {
                ViewKind::Time | ViewKind::Area => {
                    if self.follow {
                        self.view_end_ms = self.now_ms;
                    }
                    self.view_end_ms -= step;
                }
                ViewKind::Sequence => self.seq_back += 1,
            }
            self.follow = false;
            return;
        }
        if fwd {
            match self.view {
                ViewKind::Time | ViewKind::Area => {
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
            KeyCode::Tab => {
                self.view = match self.view {
                    ViewKind::Time => ViewKind::Sequence,
                    ViewKind::Sequence => ViewKind::Area,
                    ViewKind::Area => ViewKind::Time,
                }
            }
            KeyCode::Char('o') => {
                self.orient = if horizontal { Orient::Vertical } else { Orient::Horizontal };
                self.lane_scroll = 0;
            }
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc if self.popup => self.quit = true,
            KeyCode::Char('r') => self.refresh_requested = true,
            KeyCode::Char('s') if self.view == ViewKind::Area => self.area_stacked = !self.area_stacked,
            KeyCode::Char('u') => {
                self.bands = !self.bands;
                self.lane_scroll = 0;
            }
            KeyCode::Enter => self.focus_requested = self.selected_lane().map(|i| self.state.lanes[i].pane_id.clone()),
            KeyCode::Char('f') => {
                self.follow = true;
                self.seq_back = 0;
                self.area_fit = true;
            }
            KeyCode::Char('+') | KeyCode::Char('=') if self.view.timed() => {
                self.bucket_idx = self.bucket_idx.saturating_sub(1);
                self.area_fit = false;
            }
            KeyCode::Char('-') if self.view.timed() => {
                self.bucket_idx = (self.bucket_idx + 1).min(BUCKETS_S.len() - 1);
                self.area_fit = false;
            }
            _ => {}
        }
    }
}

/// Viridis, as 11 evenly spaced stops; colors between are interpolated.
const VIRIDIS: [(u8, u8, u8); 11] = [
    (0x44, 0x01, 0x54),
    (0x48, 0x24, 0x75),
    (0x41, 0x44, 0x87),
    (0x35, 0x5f, 0x8d),
    (0x2a, 0x78, 0x8e),
    (0x21, 0x91, 0x8c),
    (0x22, 0xa8, 0x84),
    (0x44, 0xbf, 0x70),
    (0x7a, 0xd1, 0x51),
    (0xbd, 0xdf, 0x26),
    (0xfd, 0xe7, 0x25),
];
/// Darkest viridis position used: below this the purples vanish on a dark terminal.
const VIRIDIS_FROM: f64 = 0.25;

/// Viridis at `t` in 0..=1.
fn viridis(t: f64) -> (u8, u8, u8) {
    let x = t.clamp(0.0, 1.0) * (VIRIDIS.len() - 1) as f64;
    let i = (x.floor() as usize).min(VIRIDIS.len() - 2);
    let f = x - i as f64;
    let (a, b) = (VIRIDIS[i], VIRIDIS[i + 1]);
    let lerp = |p: u8, q: u8| (f64::from(p) + (f64::from(q) - f64::from(p)) * f).round() as u8;
    (lerp(a.0, b.0), lerp(a.1, b.1), lerp(a.2, b.2))
}

/// Color of agent `k` of `n` (visible lanes, in order): spread evenly over viridis from
/// `VIRIDIS_FROM` to yellow, so the first agent (the base of a stack) is blue-purple and the last
/// yellow.
fn series_rgb(k: usize, n: usize) -> (u8, u8, u8) {
    let t = if n <= 1 { 0.5 } else { k as f64 / (n - 1) as f64 };
    viridis(VIRIDIS_FROM + (1.0 - VIRIDIS_FROM) * t)
}

pub fn series_color(k: usize, n: usize) -> Color {
    let (r, g, b) = series_rgb(k, n);
    Color::Rgb(r, g, b)
}

pub fn status_color(s: AgentStatus) -> Color {
    match s {
        AgentStatus::Working => Color::Green,
        AgentStatus::Blocked => Color::Red,
        AgentStatus::Done => Color::Blue,
        AgentStatus::Idle | AgentStatus::Unknown => Color::DarkGray,
    }
}

pub fn line_glyph(s: AgentStatus, orient: Orient) -> char {
    match (s, orient) {
        (AgentStatus::Working | AgentStatus::Blocked | AgentStatus::Done, Orient::Horizontal) => '━',
        (AgentStatus::Idle, Orient::Horizontal) => '─',
        (AgentStatus::Unknown, Orient::Horizontal) => '┈',
        (AgentStatus::Working | AgentStatus::Blocked | AgentStatus::Done, Orient::Vertical) => '┃',
        (AgentStatus::Idle, Orient::Vertical) => '│',
        (AgentStatus::Unknown, Orient::Vertical) => '┆',
    }
}

pub fn node_glyph(mark: Mark) -> (char, Color) {
    match mark {
        Mark::Created => ('◆', Color::Cyan),
        Mark::Seen => ('◇', Color::Cyan),
        Mark::Status(s) => ('●', status_color(s)),
        Mark::Closed | Mark::Exited => ('✕', Color::Gray),
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
    fn of(app: &App, lanes: &[usize]) -> Scale {
        let max = lanes
            .iter()
            .filter_map(|&i| {
                let l = &app.state.lanes[i];
                app.usage.max_between(&l.sessions, l.opened_ms, l.closed_ms.unwrap_or(app.now_ms))
            })
            .max()
            .unwrap_or(0);
        Scale { band: if app.bands && max > 0 { BAND } else { 1 }, max }
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
    matches!(ch, '━' | '─' | '┈' | '┃' | '│' | '┆')
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
    Some((line_glyph(s, orient), status_color(s)))
}

fn head_cell(s: AgentStatus, orient: Orient) -> Cell {
    Some((head_glyph(orient), status_color(s)))
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

fn hms(ms: i64) -> String {
    Local.timestamp_millis_opt(ms).single().map_or_else(|| "--:--:--".into(), |t| t.format("%H:%M:%S").to_string())
}

fn fmt_bucket(s: i64) -> String {
    if s >= 60 { format!("{}m", s / 60) } else { format!("{s}s") }
}

fn fmt_dur(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    if s >= 3600 { format!("{}h{}m", s / 3600, s % 3600 / 60) } else { format!("{}m{}s", s / 60, s % 60) }
}

/// Lane placement in the body. `lead` is what precedes the lanes across the time axis: the axis
/// row (horizontal time view) or the axis gutter (vertical time view). `band` is each lane's
/// thickness; bands are separated by a blank row in ↔ (columns already have a spare cell in ↕).
struct Grid {
    orient: Orient,
    body: Rect,
    lead: u16,
    band: u16,
}

impl Grid {
    /// Cells along the time axis.
    fn n(&self) -> usize {
        match self.orient {
            Orient::Horizontal => self.body.width.saturating_sub(GUTTER) as usize,
            Orient::Vertical => self.body.height.saturating_sub(1) as usize,
        }
    }

    fn gap(&self) -> u16 {
        u16::from(self.band > 1)
    }

    /// Lanes that fit side by side.
    fn slots(&self) -> usize {
        match self.orient {
            Orient::Horizontal => {
                ((self.body.height.saturating_sub(self.lead) + self.gap()) / (self.band + self.gap())) as usize
            }
            Orient::Vertical => (self.body.width.saturating_sub(self.lead) / COL_W) as usize,
        }
    }

    /// First row (↔) or column (↕) of lane `slot`.
    fn lane_start(&self, slot: usize) -> u16 {
        match self.orient {
            Orient::Horizontal => self.body.y + self.lead + slot as u16 * (self.band + self.gap()),
            Orient::Vertical => self.body.x + self.lead + slot as u16 * COL_W,
        }
    }

    fn cell_pos(&self, slot: usize, i: usize, level: u8) -> (u16, u16) {
        let start = self.lane_start(slot);
        match self.orient {
            Orient::Horizontal => (self.body.x + GUTTER + i as u16, start + (self.band - 1 - u16::from(level))),
            Orient::Vertical => {
                let across = if self.band > 1 { 1 + u16::from(level) } else { 2 };
                (start + across, self.body.y + 1 + i as u16)
            }
        }
    }

    /// Where the time-axis label for cell `i` starts.
    fn axis_pos(&self, i: usize) -> (u16, u16) {
        match self.orient {
            Orient::Horizontal => (self.body.x + GUTTER + i as u16, self.body.y),
            Orient::Vertical => (self.body.x, self.body.y + 1 + i as u16),
        }
    }

    fn put(&self, buf: &mut Buffer, (x, y): (u16, u16), ch: char, color: Color) {
        if x < self.body.right() && y < self.body.bottom() {
            buf[(x, y)].set_char(ch).set_style(Style::new().fg(color));
        }
    }

    /// Draws each lane's label and its line, scrolling so the selected lane stays in view. Where
    /// the level changes between cells, the line bends across the band in the new cell.
    fn draw_lanes(&self, buf: &mut Buffer, app: &mut App, lanes: &[usize], pts: impl Fn(&Lane, &Tracker, usize) -> Vec<Pt>) {
        let slots = self.slots();
        let sel = app.selected;
        if sel < app.lane_scroll {
            app.lane_scroll = sel;
        } else if slots > 0 && sel >= app.lane_scroll + slots {
            app.lane_scroll = sel + 1 - slots;
        }
        let n = self.n();
        let dim = Style::new().fg(Color::DarkGray);
        for (slot, (k, &li)) in lanes.iter().enumerate().skip(app.lane_scroll).take(slots).enumerate() {
            let lane = &app.state.lanes[li];
            let mut label_style = if lane.is_open() { Style::new() } else { dim };
            if k == sel {
                label_style = label_style.add_modifier(Modifier::REVERSED);
            }
            match self.orient {
                Orient::Horizontal => {
                    let (x, y) = (self.body.x, self.lane_start(slot));
                    if k == sel {
                        for cx in x..(x + GUTTER.min(self.body.width)) {
                            buf[(cx, y)].set_style(Style::new().add_modifier(Modifier::REVERSED));
                        }
                    }
                    let (glyph, gs) = if lane.is_open() { ('●', Style::new().fg(status_color(lane.status))) } else { ('✕', dim) };
                    put_str(buf, self.body, x, y, &glyph.to_string(), gs.patch(label_style));
                    put_str(buf, self.body, x + 2, y, &truncate(lane.label(), 13), label_style);
                    let ws = truncate(app.state.workspace_label(li), 6);
                    put_str(buf, self.body, x + 16, y, &ws, dim.patch(label_style));
                    if self.band > 1
                        && let Some(ctx) = app.usage.ctx_at(&lane.sessions, lane.closed_ms.unwrap_or(app.now_ms))
                    {
                        put_str(buf, self.body, x + 2, y + 1, &format!("{} ctx", fmt_tokens(ctx)), dim);
                    }
                }
                Orient::Vertical => {
                    let x = self.lane_start(slot);
                    put_str(buf, self.body, x, self.body.y, &format!("{:<5}", truncate(lane.label(), 5)), label_style);
                }
            }
            // Lines and bends take the agent's color; nodes and the head keep their status colors.
            let agent = series_color(k, lanes.len());
            let mut prev: Option<u8> = None;
            for (i, pt) in pts(lane, &app.usage, n).into_iter().enumerate().take(n) {
                let Some((mut ch, mut color)) = pt.cell else {
                    prev = None;
                    continue;
                };
                if is_line(ch) {
                    color = agent;
                }
                if let Some(p) = prev
                    && p != pt.level
                {
                    let s = pt.status.unwrap_or_default();
                    let heavy = matches!(s, AgentStatus::Working | AgentStatus::Blocked | AgentStatus::Done);
                    let c = agent;
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

fn render_line(line: Line, area: Rect, buf: &mut Buffer) {
    line.render(area, buf);
}

pub fn draw(buf: &mut Buffer, area: Rect, app: &mut App) {
    let [header, body, detail, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)])
            .areas(area);
    let dim = Style::new().fg(Color::DarkGray);
    let lanes = app.state.visible_lanes();
    app.selected = app.selected.min(lanes.len().saturating_sub(1));
    if app.view.timed() && app.follow {
        app.view_end_ms = app.now_ms;
    }

    let scale = Scale::of(app, &lanes);
    if lanes.is_empty() {
        put_str(buf, body, body.x, body.y, "no agents yet — start one in a herdr pane", dim);
    } else {
        match app.view {
            ViewKind::Time => time::render(buf, body, app, &lanes, scale),
            ViewKind::Sequence => sequence::render(buf, body, app, &lanes, scale),
            ViewKind::Area => area::render(buf, body, app, &lanes),
        }
    }

    let horizontal = app.orient == Orient::Horizontal;
    let mut spans = vec![
        Span::styled("track", Style::new().add_modifier(Modifier::BOLD)),
        Span::raw(" · "),
        Span::styled(
            match app.view {
                ViewKind::Time => "TIME",
                ViewKind::Sequence => "SEQUENCE",
                ViewKind::Area => "AREA",
            },
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::raw(if horizontal { " · ↔" } else { " · ↕" }),
        Span::raw(format!(" · {} agents", lanes.len())),
    ];
    if app.view == ViewKind::Area && app.area_fit && app.follow {
        spans.push(Span::raw(" · fit"));
    } else if app.view.timed() {
        spans.push(Span::raw(format!(" · bucket {}", fmt_bucket(BUCKETS_S[app.bucket_idx]))));
    }
    if scale.band > 1 && app.view != ViewKind::Area {
        spans.push(Span::raw(format!(" · ctx ≤ {}", fmt_tokens(scale.max))));
    }
    spans.push(Span::raw(format!(" · follow {}", if app.follow { '●' } else { '○' })));
    if let Some(err) = &app.error {
        spans.push(Span::styled(format!(" · herdr socket unavailable: {err} — retrying"), Style::new().fg(Color::Red)));
    }
    render_line(Line::from(spans), header, buf);

    if let Some(&li) = lanes.get(app.selected) {
        let lane = &app.state.lanes[li];
        let color = status_color(lane.status);
        let glyph = if lane.is_open() { '●' } else { '✕' };
        let when = match lane.closed_ms {
            None => format!(" {}", fmt_dur(app.now_ms - lane.last_ms())),
            Some(c) => format!(" closed {}", hms(c)),
        };
        let ctx = app
            .usage
            .ctx_at(&lane.sessions, lane.closed_ms.unwrap_or(app.now_ms))
            .map(|c| format!(" · {} ctx", fmt_tokens(c)))
            .unwrap_or_default();
        let text = format!(
            " {} · {} · {} · {}{when}{ctx} · {} steps · {}",
            lane.label(),
            app.state.workspace_label(li),
            lane.agent.as_deref().unwrap_or("-"),
            lane.status.as_str(),
            lane.steps.len(),
            lane.pane_id,
        );
        render_line(Line::from(vec![Span::styled(glyph.to_string(), Style::new().fg(color)), Span::raw(text)]), detail, buf);
    }

    let (other, flip, lane_keys, scroll_keys) = (
        match app.view {
            ViewKind::Time => "sequence",
            ViewKind::Sequence => "area",
            ViewKind::Area => "time",
        },
        if horizontal { "vertical" } else { "horizontal" },
        if horizontal { "j/k" } else { "h/l" },
        if horizontal { "h/l" } else { "j/k" },
    );
    let zoom = if app.view.timed() { " · +/- zoom" } else { "" };
    let usage = if app.view == ViewKind::Area { " · s stack" } else { " · u usage" };
    let help = format!(
        "Tab {other} · o {flip} · {lane_keys} agent · {scroll_keys} scroll{zoom}{usage} · f follow · enter jump · r refresh · q quit"
    );
    render_line(Line::styled(help, dim), footer, buf);
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
        let mut app = App::new(state, view, orient, 8500);
        app.bucket_idx = 0;
        app
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

    fn press(app: &mut App, c: char) {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }

    fn two_lanes(orient: Orient) -> App {
        let mut app = sample_app(ViewKind::Time, orient);
        let i = app.state.open("w1:p2", "w1", 100, Mark::Seen, AgentStatus::Idle, true);
        app.state.lanes[i].agent = Some("omp".into());
        app
    }

    #[test]
    fn vertical_keys_swap_lane_and_time_axes() {
        let mut app = two_lanes(Orient::Vertical);
        press(&mut app, 'l');
        assert_eq!(app.selected, 1);
        press(&mut app, 'k');
        assert!(!app.follow);
        let back = app.view_end_ms;
        assert!(back < app.now_ms);
        press(&mut app, 'j');
        assert!(app.view_end_ms > back);
        assert_eq!(app.selected, 1, "time keys leave the selection alone");
    }

    #[test]
    fn horizontal_j_selects_next_lane() {
        let mut app = two_lanes(Orient::Horizontal);
        press(&mut app, 'j');
        assert_eq!(app.selected, 1);
        press(&mut app, 'j');
        assert_eq!(app.selected, 1, "clamped");
        assert!(app.follow);
    }

    #[test]
    fn series_spread_over_viridis() {
        assert_eq!(viridis(0.0), (0x44, 0x01, 0x54));
        assert_eq!(viridis(1.0), (0xfd, 0xe7, 0x25));
        assert_eq!(viridis(0.5), (0x21, 0x91, 0x8c));
        assert_eq!(series_rgb(1, 2), (0xfd, 0xe7, 0x25), "top series is yellow");
        assert_eq!(series_rgb(0, 2), viridis(VIRIDIS_FROM), "base skips the darkest purples");
        assert_eq!(series_rgb(1, 3), viridis(VIRIDIS_FROM + (1.0 - VIRIDIS_FROM) / 2.0));
    }

    #[test]
    fn crowded_cell_prefers_lifecycle_then_counts_statuses() {
        use AgentStatus::*;
        let s = |at_ms, mark, status| Step { at_ms, mark, status };
        let both = [s(0, Mark::Status(Working), Working), s(1, Mark::Status(Blocked), Blocked)];
        assert_eq!(node_cell(&both), Some(('◉', Color::Red)));
        let closed = [s(0, Mark::Status(Working), Working), s(1, Mark::Closed, Working)];
        assert_eq!(node_cell(&closed), Some(('✕', Color::Gray)));
        assert_eq!(node_cell(&[]), None);
    }
}

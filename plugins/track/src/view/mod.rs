//! Shared frame (header, detail, footer), lane layout for both orientations, glyphs and keys.

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

pub const BUCKETS_S: [i64; 5] = [1, 5, 15, 60, 300];
/// Horizontal lane gutter: status glyph, label, workspace.
const GUTTER: u16 = 22;
/// Vertical lane column width.
const COL_W: u16 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    Time,
    Sequence,
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
        }
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
                ViewKind::Time => {
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
                ViewKind::Time => {
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
                    ViewKind::Sequence => ViewKind::Time,
                }
            }
            KeyCode::Char('o') => {
                self.orient = if horizontal { Orient::Vertical } else { Orient::Horizontal };
                self.lane_scroll = 0;
            }
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc if self.popup => self.quit = true,
            KeyCode::Char('r') => self.refresh_requested = true,
            KeyCode::Enter => self.focus_requested = self.selected_lane().map(|i| self.state.lanes[i].pane_id.clone()),
            KeyCode::Char('f') => {
                self.follow = true;
                self.seq_back = 0;
            }
            KeyCode::Char('+') | KeyCode::Char('=') if self.view == ViewKind::Time => {
                self.bucket_idx = self.bucket_idx.saturating_sub(1)
            }
            KeyCode::Char('-') if self.view == ViewKind::Time => {
                self.bucket_idx = (self.bucket_idx + 1).min(BUCKETS_S.len() - 1)
            }
            _ => {}
        }
    }
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
/// row (horizontal time view) or the axis gutter (vertical time view).
struct Grid {
    orient: Orient,
    body: Rect,
    lead: u16,
}

impl Grid {
    /// Cells along the time axis.
    fn n(&self) -> usize {
        match self.orient {
            Orient::Horizontal => self.body.width.saturating_sub(GUTTER) as usize,
            Orient::Vertical => self.body.height.saturating_sub(1) as usize,
        }
    }

    /// Lanes that fit side by side.
    fn slots(&self) -> usize {
        match self.orient {
            Orient::Horizontal => self.body.height.saturating_sub(self.lead) as usize,
            Orient::Vertical => (self.body.width.saturating_sub(self.lead) / COL_W) as usize,
        }
    }

    fn cell_pos(&self, slot: usize, i: usize) -> (u16, u16) {
        let (x, y) = (self.body.x, self.body.y);
        match self.orient {
            Orient::Horizontal => (x + GUTTER + i as u16, y + self.lead + slot as u16),
            Orient::Vertical => (x + self.lead + slot as u16 * COL_W + 2, y + 1 + i as u16),
        }
    }

    /// Where the time-axis label for cell `i` starts.
    fn axis_pos(&self, i: usize) -> (u16, u16) {
        match self.orient {
            Orient::Horizontal => (self.body.x + GUTTER + i as u16, self.body.y),
            Orient::Vertical => (self.body.x, self.body.y + 1 + i as u16),
        }
    }

    /// Draws each lane's label and its cells, scrolling so the selected lane stays in view.
    fn draw_lanes(&self, buf: &mut Buffer, app: &mut App, lanes: &[usize], cells: impl Fn(&Lane, usize) -> Vec<Cell>) {
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
                    let (x, y) = (self.body.x, self.cell_pos(slot, 0).1);
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
                }
                Orient::Vertical => {
                    let x = self.body.x + self.lead + slot as u16 * COL_W;
                    put_str(buf, self.body, x, self.body.y, &format!("{:<5}", truncate(lane.label(), 5)), label_style);
                }
            }
            for (i, cell) in cells(lane, n).into_iter().enumerate().take(n) {
                let Some((ch, color)) = cell else { continue };
                let (x, y) = self.cell_pos(slot, i);
                if x < self.body.right() && y < self.body.bottom() {
                    buf[(x, y)].set_char(ch).set_style(Style::new().fg(color));
                }
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
    if app.view == ViewKind::Time && app.follow {
        app.view_end_ms = app.now_ms;
    }

    if lanes.is_empty() {
        put_str(buf, body, body.x, body.y, "no agents yet — start one in a herdr pane", dim);
    } else {
        match app.view {
            ViewKind::Time => time::render(buf, body, app, &lanes),
            ViewKind::Sequence => sequence::render(buf, body, app, &lanes),
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
            },
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::raw(if horizontal { " · ↔" } else { " · ↕" }),
        Span::raw(format!(" · {} agents", lanes.len())),
    ];
    if app.view == ViewKind::Time {
        spans.push(Span::raw(format!(" · bucket {}", fmt_bucket(BUCKETS_S[app.bucket_idx]))));
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
        let text = format!(
            " {} · {} · {} · {}{when} · {} steps · {}",
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
            ViewKind::Sequence => "time",
        },
        if horizontal { "vertical" } else { "horizontal" },
        if horizontal { "j/k" } else { "h/l" },
        if horizontal { "h/l" } else { "j/k" },
    );
    let zoom = if app.view == ViewKind::Time { " · +/- zoom" } else { "" };
    let help = format!(
        "Tab {other} · o {flip} · {lane_keys} agent · {scroll_keys} scroll{zoom} · f follow · enter jump · r refresh · q quit"
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

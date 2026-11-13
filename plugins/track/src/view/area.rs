//! Area view: context in use per agent over time, as an area chart. Each agent is a series in its
//! own color: a braille line (2×4 dots per cell) over a tinted fill. Stacked, each series sits on
//! the ones below and the top line is the total; overlaid (`s`), every series starts at zero with
//! the largest at the back. A value axis, a legend and labels at each line's end frame it.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::time::{AXIS_W, start_ms};
use super::{App, Orient, hms, put_str, truncate};
use crate::model::{HISTORY_MS, Lane};
use crate::usage::{Tracker, fmt_tokens};

const PALETTE: [(u8, u8, u8); 6] =
    [(97, 175, 239), (198, 120, 221), (152, 195, 121), (229, 192, 123), (86, 182, 194), (209, 154, 102)];
/// Assumed terminal background, for tinting fills.
const BG: (u8, u8, u8) = (28, 28, 30);
/// ↔ value-axis gutter (`530k `) and end-label margin.
const VAX_W: u16 = 6;
const LABEL_W: u16 = 14;
const GRID: Color = Color::Rgb(72, 72, 78);
/// ↕ rows under the chart for end labels; labels that would collide drop to the next row.
const END_ROWS: u16 = 2;

fn series_rgb(k: usize) -> (u8, u8, u8) {
    PALETTE[k % PALETTE.len()]
}

fn line_color(k: usize) -> Color {
    let (r, g, b) = series_rgb(k);
    Color::Rgb(r, g, b)
}

/// Fill where the series `ks` cover a cell, back to front: each lays a 30% tint of its color over
/// what is behind it, so overlapping areas show through each other.
fn fill(ks: impl IntoIterator<Item = usize>) -> Option<Color> {
    let mix = |behind: u8, c: u8| ((u16::from(c) * 3 + u16::from(behind) * 7) / 10) as u8;
    let mut out = None;
    for k in ks {
        let (r, g, b) = series_rgb(k);
        let (br, bgr, bb) = out.unwrap_or(BG);
        out = Some((mix(br, r), mix(bgr, g), mix(bb, b)));
    }
    out.map(|(r, g, b)| Color::Rgb(r, g, b))
}

fn fill_color(k: usize) -> Color {
    fill([k]).unwrap_or(Color::Reset)
}

/// Smallest 1, 1.5, 2, 2.5, 3, 4, 5, 6 or 8 × 10ⁿ at or above `v`.
fn nice_ceil(v: u64) -> u64 {
    if v == 0 {
        return 0;
    }
    let mag = 10f64.powi((v as f64).log10().floor() as i32);
    [1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0]
        .iter()
        .map(|m| (m * mag).round() as u64)
        .find(|&n| n >= v)
        .unwrap_or(v)
}

/// Braille cells: per-cell dot mask, line color and fill color.
struct Canvas {
    cols: usize,
    rows: usize,
    mask: Vec<u8>,
    fg: Vec<Option<Color>>,
    bg: Vec<Option<Color>>,
}

impl Canvas {
    fn new(cols: usize, rows: usize) -> Self {
        Canvas { cols, rows, mask: vec![0; cols * rows], fg: vec![None; cols * rows], bg: vec![None; cols * rows] }
    }

    /// Lights screen dot (sx, sy): 2 dots across, 4 down per cell.
    fn dot(&mut self, sx: usize, sy: usize, color: Color) {
        let (c, r) = (sx / 2, sy / 4);
        if c >= self.cols || r >= self.rows {
            return;
        }
        const BITS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
        let i = r * self.cols + c;
        self.mask[i] |= BITS[sx % 2][sy % 4];
        self.fg[i] = Some(color);
    }
}

/// One agent's values per time dot (None outside its life or before any usage).
struct Series {
    k: usize,
    vals: Vec<Option<u64>>,
}

/// Chart geometry in one orientation: time and value axes in dots, and where the chart sits.
struct Plot {
    orient: Orient,
    area: Rect,
}

impl Plot {
    fn t_dots(&self) -> usize {
        match self.orient {
            Orient::Horizontal => self.area.width as usize * 2,
            Orient::Vertical => self.area.height as usize * 4,
        }
    }

    fn v_dots(&self) -> usize {
        match self.orient {
            Orient::Horizontal => self.area.height as usize * 4,
            Orient::Vertical => self.area.width as usize * 2,
        }
    }

    /// Screen dot for time dot `t`, value dot `v` (0 = baseline).
    fn screen(&self, t: usize, v: usize) -> (usize, usize) {
        match self.orient {
            Orient::Horizontal => (t, self.v_dots() - 1 - v.min(self.v_dots() - 1)),
            Orient::Vertical => (v, t),
        }
    }

    /// Chart-space dot at the center of cell (col, row).
    fn cell_center(&self, col: usize, row: usize) -> (usize, usize) {
        let (sx, sy) = (col * 2 + 1, row * 4 + 2);
        match self.orient {
            Orient::Horizontal => (sx, self.v_dots() - 1 - sy),
            Orient::Vertical => (sy, sx),
        }
    }

    /// Terminal position of the cell holding screen dot (sx, sy).
    fn cell_pos(&self, sx: usize, sy: usize) -> (u16, u16) {
        (self.area.x + (sx / 2) as u16, self.area.y + (sy / 4) as u16)
    }
}

/// First and last ms the chart covers, and ms per time dot. Fitting while following stretches the
/// history (capped at `HISTORY_MS`) across the whole time axis; otherwise TIME's buckets apply.
fn time_frame(app: &App, lanes: &[usize], t_dots: usize, dots_per_cell: usize) -> (i64, i64) {
    let now = app.now_ms;
    if app.area_fit && app.follow {
        let earliest = lanes
            .iter()
            .map(|&i| {
                let l = &app.state.lanes[i];
                app.usage.first_at(&l.sessions).map_or(l.opened_ms, |f| f.min(l.opened_ms))
            })
            .min()
            .unwrap_or(now)
            .max(now - HISTORY_MS);
        let dot_ms = ((now - earliest) / t_dots.max(1) as i64 + 1).max(1000);
        return (earliest, dot_ms);
    }
    let bucket = app.bucket_ms();
    let start = start_ms(app, lanes, bucket, t_dots / dots_per_cell);
    (start, (bucket / dots_per_cell as i64).max(1))
}

fn value_at(lane: &Lane, usage: &Tracker, at: i64) -> Option<u64> {
    if lane.closed_ms.is_some_and(|c| at >= c) {
        return None;
    }
    usage.ctx_at(&lane.sessions, at)
}

pub fn render(buf: &mut Buffer, body: Rect, app: &mut App, lanes: &[usize]) {
    let horizontal = app.orient == Orient::Horizontal;
    let dim = Style::new().fg(Color::DarkGray);
    // ↔: legend row, chart, time-axis row; value axis left, end labels right.
    // ↕: legend row, value-axis row, chart, two end-label rows; time axis left.
    let label_w = if horizontal && body.width >= 40 { LABEL_W } else { 0 };
    let plot = if horizontal {
        Rect::new(body.x + VAX_W, body.y + 1, body.width.saturating_sub(VAX_W + label_w), body.height.saturating_sub(2))
    } else {
        Rect::new(body.x + AXIS_W, body.y + 2, body.width.saturating_sub(AXIS_W), body.height.saturating_sub(2 + END_ROWS))
    };
    let plot = Plot { orient: app.orient, area: plot };
    let (t_dots, v_dots) = (plot.t_dots(), plot.v_dots());
    let dots_per_cell = if horizontal { 2 } else { 4 };
    app.time_cells = (t_dots / dots_per_cell).max(1);
    if t_dots == 0 || v_dots == 0 {
        return;
    }
    let (start, dot_ms) = time_frame(app, lanes, t_dots, dots_per_cell);
    let now = app.now_ms;

    let mut series: Vec<Series> = lanes
        .iter()
        .enumerate()
        .map(|(k, &li)| {
            let lane = &app.state.lanes[li];
            let vals = (0..t_dots)
                .map(|j| {
                    let t0 = start + j as i64 * dot_ms;
                    (t0 <= now).then(|| value_at(lane, &app.usage, (t0 + dot_ms - 1).min(now))).flatten()
                })
                .collect();
            Series { k, vals }
        })
        .collect();
    let stacked = app.area_stacked;
    if !stacked {
        // Back to front: the largest series behind, so smaller ones stay visible.
        series.sort_by_key(|s| std::cmp::Reverse(s.vals.iter().flatten().max().copied().unwrap_or(0)));
    }
    // Top of each series per time dot, in tokens: cumulative when stacked.
    let tops: Vec<Vec<Option<u64>>> = {
        let mut cum = vec![0u64; t_dots];
        series
            .iter()
            .map(|s| {
                s.vals
                    .iter()
                    .enumerate()
                    .map(|(j, v)| {
                        v.map(|v| {
                            if stacked {
                                cum[j] += v;
                                cum[j]
                            } else {
                                v
                            }
                        })
                    })
                    .collect()
            })
            .collect()
    };
    let max = nice_ceil(tops.iter().flatten().flatten().copied().max().unwrap_or(0));

    // Legend.
    let mut x = body.x;
    for (k, &li) in lanes.iter().enumerate() {
        let lane = &app.state.lanes[li];
        let mut style = Style::new().fg(line_color(k));
        if k == app.selected {
            style = style.add_modifier(Modifier::REVERSED);
        }
        x = put_str(buf, body, x, body.y, "■ ", Style::new().fg(line_color(k)));
        x = put_str(buf, body, x, body.y, &truncate(lane.label(), 14), style) + 2;
    }
    put_str(
        buf,
        body,
        x,
        body.y,
        if stacked { "stacked" } else { "overlaid" },
        dim,
    );
    if max == 0 {
        put_str(buf, body, plot.area.x, plot.area.y, "no usage yet — needs omp session usage", dim);
        return;
    }
    let to_dot = |tokens: u64| ((tokens as f64 / max as f64) * (v_dots - 1) as f64).round() as usize;

    let mut canvas = Canvas::new(plot.area.width as usize, plot.area.height as usize);
    // Fill from the series whose areas hold each cell's center. Stacked bands don't overlap, so
    // the lowest band containing it owns the cell; overlaid areas all show, blended back to front.
    for row in 0..canvas.rows {
        for col in 0..canvas.cols {
            let (t, v) = plot.cell_center(col, row);
            let covering = tops.iter().enumerate().filter(|(_, top)| top[t].is_some_and(|top| v <= to_dot(top)));
            canvas.bg[row * canvas.cols + col] = if stacked {
                covering.min_by_key(|(_, top)| top[t]).map(|(i, _)| fill_color(series[i].k))
            } else {
                fill(covering.map(|(i, _)| series[i].k))
            };
        }
    }
    // Lines, joined across each step so they stay continuous.
    for (i, s) in series.iter().enumerate() {
        let color = line_color(s.k);
        let mut prev: Option<usize> = None;
        for t in 0..t_dots {
            let Some(top) = tops[i][t] else {
                prev = None;
                continue;
            };
            let d = to_dot(top);
            let (lo, hi) = prev.map_or((d, d), |p| (p.min(d), p.max(d)));
            for v in lo..=hi {
                let (sx, sy) = plot.screen(t, v);
                canvas.dot(sx, sy, color);
            }
            prev = Some(d);
        }
    }

    // Gridlines at value ticks, behind lines; then the canvas.
    let ticks: Vec<u64> = (0..=4).map(|q| max * q / 4).collect();
    for (row, col) in (0..canvas.rows).flat_map(|r| (0..canvas.cols).map(move |c| (r, c))) {
        let i = row * canvas.cols + col;
        let (x, y) = (plot.area.x + col as u16, plot.area.y + row as u16);
        let bg = canvas.bg[i].unwrap_or(Color::Reset);
        let cell = &mut buf[(x, y)];
        if canvas.mask[i] != 0 {
            let ch = char::from_u32(0x2800 + u32::from(canvas.mask[i])).unwrap_or(' ');
            cell.set_char(ch).set_style(Style::new().fg(canvas.fg[i].unwrap_or(Color::Reset)).bg(bg));
            continue;
        }
        let (t, _) = plot.cell_center(col, row);
        let on_tick = ticks.iter().any(|&tk| {
            let d = to_dot(tk);
            match plot.orient {
                Orient::Horizontal => plot.screen(t, d).1 / 4 == row,
                Orient::Vertical => plot.screen(t, d).0 / 2 == col,
            }
        });
        let grid = match plot.orient {
            Orient::Horizontal => '┈',
            Orient::Vertical => '┆',
        };
        cell.set_char(if on_tick { grid } else { ' ' }).set_style(Style::new().fg(GRID).bg(bg));
    }

    // Value axis labels.
    for &tk in &ticks {
        let (sx, sy) = plot.screen(0, to_dot(tk));
        let text = fmt_tokens(tk);
        match plot.orient {
            Orient::Horizontal => {
                let y = plot.cell_pos(sx, sy).1;
                put_str(buf, body, body.x, y, &format!("{text:>5}"), dim);
            }
            Orient::Vertical => {
                let len = text.chars().count() as u16;
                let x = plot.cell_pos(sx, sy).0.saturating_sub(len / 2).max(plot.area.x);
                let x = x.min(body.right().saturating_sub(len));
                put_str(buf, body, x, body.y + 1, &text, dim);
            }
        }
    }

    // Time axis labels.
    let cells = t_dots / dots_per_cell;
    let every = if horizontal { 12 } else { 6 };
    for c in (0..cells).step_by(every) {
        let t_ms = start + (c * dots_per_cell) as i64 * dot_ms;
        if t_ms > now {
            break;
        }
        match plot.orient {
            Orient::Horizontal => {
                put_str(buf, body, plot.area.x + c as u16, body.bottom() - 1, &format!("┊{}", hms(t_ms)), dim);
            }
            Orient::Vertical => {
                put_str(buf, body, body.x, plot.area.y + c as u16, &hms(t_ms), dim);
            }
        }
    }

    // End labels at each line's last point.
    let last_t = (0..t_dots).rev().find(|&t| tops.iter().any(|top| top[t].is_some()));
    let Some(last_t) = last_t else { return };
    let mut ends: Vec<(u16, u16, usize, u64)> = series
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let top = tops[i][last_t]?;
            let (sx, sy) = plot.screen(last_t, to_dot(top));
            let (x, y) = plot.cell_pos(sx, sy);
            Some((x, y, s.k, s.vals[last_t].unwrap_or(0)))
        })
        .collect();
    match plot.orient {
        Orient::Horizontal if label_w > 0 => {
            ends.sort_by_key(|e| e.1);
            let mut used: Vec<u16> = Vec::new();
            for (_, y, k, val) in ends {
                let mut y = y;
                while used.contains(&y) && y + 1 < plot.area.bottom() {
                    y += 1;
                }
                used.push(y);
                let name = truncate(app.state.lanes[lanes[k]].label(), (LABEL_W - 7) as usize);
                let x = plot.area.right() + 1;
                put_str(buf, body, x, y, &format!("{name} {}", fmt_tokens(val)), Style::new().fg(line_color(k)));
            }
        }
        Orient::Vertical => {
            let first = (plot.cell_pos(0, plot.screen(last_t, 0).1).1 + 1).min(body.bottom().saturating_sub(END_ROWS));
            ends.sort_by_key(|e| e.0);
            // Free x per row; each label takes the first row it fits in, at its line's x.
            let mut free = vec![body.x; END_ROWS as usize];
            for (x, _, k, val) in ends {
                let name = truncate(app.state.lanes[lanes[k]].label(), 8);
                let text = format!("{name} {}", fmt_tokens(val));
                let len = text.chars().count() as u16;
                let x = x.min(body.right().saturating_sub(len));
                let Some(row) = free.iter().position(|&f| f <= x) else { continue };
                free[row] = x + len + 1;
                put_str(buf, body, x, first + row as u16, &text, Style::new().fg(line_color(k)));
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::super::ViewKind;
    use super::super::tests::{render, sample_app};
    use super::*;
    use crate::model::{AgentStatus, Mark};
    use crate::usage::Sample;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;

    /// Lane 0 at 100 tokens and lane 1 at 300, both from 0 ms; `now_ms = 8500`.
    fn two_series(orient: Orient) -> App {
        let mut app = sample_app(ViewKind::Area, orient);
        let i = app.state.open("w1:p2", "w1", 0, Mark::Created, AgentStatus::Working, true);
        app.state.lanes[i].agent = Some("claude".into());
        for (li, ctx) in [(0, 100), (i, 300)] {
            let path = PathBuf::from(format!("/s{li}.jsonl"));
            app.state.lanes[li].sessions.push(path.clone());
            app.usage.insert(path, vec![Sample { at_ms: 0, ctx }]);
        }
        app
    }

    fn braille(s: &str) -> bool {
        s.chars().next().is_some_and(|c| ('\u{2801}'..='\u{28FF}').contains(&c))
    }

    #[test]
    fn nice_ceil_rounds_up_to_readable_maxima() {
        assert_eq!(nice_ceil(400), 400);
        assert_eq!(nice_ceil(401), 500);
        assert_eq!(nice_ceil(527_000), 600_000);
        assert_eq!(nice_ceil(130_000), 150_000);
        assert_eq!(nice_ceil(0), 0);
    }

    /// Body is rows 1..=10 of a 13-row frame: legend row 1, chart rows 2..=9 (32 value dots), time
    /// axis row 10.
    #[test]
    fn stacked_series_sit_on_each_other() {
        let mut app = two_series(Orient::Horizontal);
        let buf = render(&mut app, 60, 13);
        let x = VAX_W + 2;
        // max 400 over 31 dots: lane 0's top at dot 8 (row 7), the total at dot 31 (row 2).
        assert!(braille(buf[(x, 2)].symbol()), "total line on the top row");
        assert_eq!(buf[(x, 2)].fg, line_color(1));
        assert!(braille(buf[(x, 7)].symbol()), "lane 0's line at a quarter height");
        assert_eq!(buf[(x, 7)].fg, line_color(0));
        assert_eq!(buf[(x, 9)].bg, fill_color(0), "lane 0 fills the base");
        assert_eq!(buf[(x, 4)].bg, fill_color(1), "lane 1 fills above it");
        assert_eq!(buf[(0, 2)].symbol().trim(), "", "value labels are right-aligned");
        assert!((0..VAX_W).map(|c| buf[(c, 2)].symbol()).collect::<String>().contains("400"));
    }

    #[test]
    fn overlaid_series_start_at_zero_with_largest_behind() {
        let mut app = two_series(Orient::Horizontal);
        app.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert!(!app.area_stacked);
        let buf = render(&mut app, 60, 13);
        let x = VAX_W + 2;
        // max 300: lane 1's line on top, lane 0's at a third. Below lane 0's line both areas
        // overlap, lane 0's tint over lane 1's; above it only lane 1's.
        assert_eq!(buf[(x, 2)].fg, line_color(1));
        let both = fill([1, 0]).unwrap();
        assert_eq!(buf[(x, 9)].bg, both, "overlap blends, smaller series in front");
        assert_ne!(both, fill_color(0));
        assert_ne!(both, fill_color(1));
        assert_eq!(buf[(x, 4)].bg, fill_color(1));
    }

    #[test]
    fn fit_spreads_history_across_the_axis() {
        let mut app = two_series(Orient::Horizontal);
        let buf = render(&mut app, 60, 13);
        // 8.5 s over 80 dots: the 1 s floor applies, so lines end around dot 8 (cell 4).
        let last = (VAX_W..60).rev().find(|&x| braille(buf[(x, 2)].symbol())).unwrap();
        assert_eq!(last, VAX_W + 4);
        app.now_ms = 200_000;
        let buf = render(&mut app, 60, 13);
        let last = (VAX_W..60 - LABEL_W).rev().find(|&x| braille(buf[(x, 2)].symbol())).unwrap();
        assert!(last >= 60 - LABEL_W - 2, "a long history reaches the right edge, got {last}");
    }

    #[test]
    fn vertical_values_grow_rightward() {
        let mut app = two_series(Orient::Vertical);
        let buf = render(&mut app, AXIS_W + 8, 12);
        // Chart row 3 (first time row): lane 0's fill at the left, lane 1's line at the right.
        assert_eq!(buf[(AXIS_W, 3)].bg, fill_color(0));
        let right = (AXIS_W..AXIS_W + 8).rev().find(|&x| braille(buf[(x, 3)].symbol())).unwrap();
        assert_eq!(buf[(right, 3)].fg, line_color(1));
        assert_eq!(right, AXIS_W + 7);
    }
}

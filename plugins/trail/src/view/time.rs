//! Time view: cells are fixed time buckets, so line length is wall-clock duration.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::gutter::Cols;
use super::{App, Cell, Grid, Orient, Pt, Scale, head_cell, line_cell, node_cell, put_str, time_ticks};
use crate::model::{Lane, status_at};
use crate::theme::theme;
use crate::usage::Tracker;

/// ↕ axis gutter: `HH:MM` ticks when they're a minute or more apart, else `HH:MM:SS`, plus a space.
pub(super) fn axis_w(bucket_ms: i64) -> u16 {
    if bucket_ms * 4 >= 60_000 { 6 } else { 9 }
}

/// Cells along the time axis in `body` for the app's orientation.
pub(super) fn cells(app: &App, body: Rect) -> usize {
    match app.orient {
        Orient::Horizontal => Cols::fit(body.width).strip(body.width) as usize,
        Orient::Vertical => body.height.saturating_sub(1) as usize,
    }
}

/// Start of the first cell: the last cell holds the view's end (now while following).
pub(super) fn start_ms(app: &App, bucket: i64, n: usize) -> i64 {
    let end = if app.follow { app.now_ms } else { app.view_end_ms };
    (end.div_euclid(bucket) + 1) * bucket - n as i64 * bucket
}

fn cell(lane: &Lane, t0: i64, t1: i64, now_ms: i64, orient: Orient) -> Cell {
    let lo = lane.steps.partition_point(|s| s.at_ms < t0);
    let hi = lane.steps.partition_point(|s| s.at_ms < t1);
    if let Some(node) = node_cell(&lane.steps[lo..hi]) {
        return Some(node);
    }
    if lane.is_open() && (t0..t1).contains(&now_ms) {
        return head_cell(lane.status, orient);
    }
    if t0 > now_ms {
        return None;
    }
    status_at(lane, t1 - 1).and_then(|s| line_cell(s, orient))
}

/// Cells for `lane`, each at the context in use at the end of its bucket (or now).
#[allow(clippy::too_many_arguments)]
fn pts(lane: &Lane, usage: &Tracker, scale: Scale, start: i64, bucket: i64, n: usize, now_ms: i64, orient: Orient) -> Vec<Pt> {
    (0..n)
        .map(|i| {
            let t0 = start + i as i64 * bucket;
            let t1 = t0 + bucket;
            let at = (t1 - 1).min(now_ms);
            Pt {
                cell: cell(lane, t0, t1, now_ms, orient),
                level: scale.level(usage.ctx_at(&lane.sessions, at)),
                status: status_at(lane, at),
            }
        })
        .collect()
}

/// Axis labels sit on absolute bucket multiples so they scroll with time.
pub(super) fn draw_axis(buf: &mut Buffer, grid: &Grid, start: i64, bucket: i64) {
    let horizontal = grid.orient == Orient::Horizontal;
    let dim = Style::new().fg(theme().dim);
    let area = grid.axis_area();
    for (i, label) in time_ticks(start, bucket as f64, grid.n(), horizontal) {
        let (x, y) = grid.axis_pos(i);
        put_str(buf, area, x, y, &if horizontal { format!("┊{label}") } else { label }, dim);
    }
}

pub fn render(buf: &mut Buffer, body: Rect, app: &mut App, lanes: &[usize]) {
    app.fit_bucket(lanes, cells(app, body));
    let bucket = app.bucket_ms();
    let lead = match app.orient {
        Orient::Horizontal => 1,
        Orient::Vertical => axis_w(bucket),
    };
    let scale = Scale::of(app, lanes, body.height.saturating_sub(lead));
    if scale.band > 1 {
        app.band_max = Some(scale.max);
    }
    let grid = Grid::new(app.orient, body, lead, scale.band, lanes.len());
    let n = grid.n();
    app.time_cells = n.max(1);
    let start = start_ms(app, bucket, n);

    draw_axis(buf, &grid, start, bucket);

    let (now, orient) = (app.now_ms, app.orient);
    grid.draw_lanes(buf, app, lanes, |lane, usage, n| pts(lane, usage, scale, start, bucket, n, now, orient));
}

#[cfg(test)]
mod tests {
    use super::super::ViewKind;
    use super::super::tests::{col, render, row, sample_app};
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn horizontal_lane_ends_at_now_on_the_far_edge() {
        let mut app = sample_app(ViewKind::Time, Orient::Horizontal);
        // Header, axis row, lane row. 27 strip cells from column 33, 1 s buckets fit 8.5 s.
        let buf = render(&mut app, 60, 3);
        assert_eq!(row(&buf, 2, 33, 60).trim_start(), "◆─●━━▲╍╍▸");
        assert_eq!(buf[(59, 2)].symbol(), "▸", "now sits in the last cell");
        assert_eq!(buf[(59, 2)].fg, theme().blocked, "head in the current status color");
        assert_eq!(buf[(52, 2)].fg, theme().idle, "lines take their status color");
        assert_eq!(buf[(53, 2)].fg, theme().working, "status nodes keep status colors");
        assert!(row(&buf, 0, 0, 60).starts_with("TIME seq area heat dash"));
    }

    #[test]
    fn vertical_lane_grows_downward() {
        let mut app = sample_app(ViewKind::Time, Orient::Vertical);
        // Header, column header row, 14 cells of 1 s: now (8.5 s) in the last.
        let buf = render(&mut app, axis_w(1000) + 6, 16);
        let cells = col(&buf, axis_w(1000) + 2, 7, 16);
        assert_eq!(cells, ["◆", "│", "●", "┃", "┃", "▲", "╏", "╏", "▾"]);
        assert_eq!(buf[(axis_w(1000) + 2, 6)].symbol(), " ", "nothing before the lane opened");
    }

    #[test]
    fn vertical_axis_narrows_for_minute_ticks() {
        assert_eq!(axis_w(1000), 9);
        assert_eq!(axis_w(15_000), 6);
    }

    /// Context 100 from the start, 400 from 4000 ms: the line bends up a level per cell of band.
    fn with_usage(orient: Orient) -> crate::view::App {
        use crate::usage::Sample;
        let mut app = sample_app(ViewKind::Time, orient);
        let path = std::path::PathBuf::from("/s.jsonl");
        app.state.lanes[0].sessions.push(path.clone());
        app.usage.insert(path, vec![Sample { at_ms: 0, ctx: 100, cost: 0.25 }, Sample { at_ms: 4000, ctx: 400, cost: 0.5 }]);
        app
    }

    #[test]
    fn horizontal_band_rises_with_context() {
        let mut app = with_usage(Orient::Horizontal);
        // Header, axis row, 4-row band from row 2; the lane starts at strip cell 18 (column 51).
        let buf = render(&mut app, 60, 7);
        assert_eq!(row(&buf, 2, 51, 60), "    ┏▲╍╍▸", "level 3: 400 of 400");
        assert_eq!(row(&buf, 3, 51, 60), "    ┃");
        assert_eq!(row(&buf, 4, 51, 60), "◆─●━┛", "level 1: 100 of 400");
        assert_eq!(row(&buf, 3, 3, 33), "ctx 400 · peak 400");
        assert_eq!(row(&buf, 4, 3, 33), "$0.75 · 2 reqs");
        assert_eq!(row(&buf, 5, 3, 33), "omp · w1:p1");
        let wide = render(&mut app, 120, 7);
        assert!(row(&wide, 0, 0, 120).contains("ctx≤400"), "{}", row(&wide, 0, 0, 120));

        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
        let buf = render(&mut app, 60, 7);
        assert_eq!(row(&buf, 2, 51, 60), "◆─●━━▲╍╍▸", "u flattens the bands");
    }

    #[test]
    fn bands_collapse_when_they_dont_fit() {
        let mut app = with_usage(Orient::Horizontal);
        // Header, axis row and three lane rows: a 4-row band doesn't fit.
        let buf = render(&mut app, 60, 5);
        assert_eq!(row(&buf, 2, 51, 60), "◆─●━━▲╍╍▸");
        assert_eq!(app.band_max, None);
    }

    #[test]
    fn vertical_lanes_never_band() {
        let mut app = with_usage(Orient::Vertical);
        let buf = render(&mut app, axis_w(1000) + 6, 16);
        let cells = col(&buf, axis_w(1000) + 2, 7, 16);
        assert_eq!(cells, ["◆", "│", "●", "┃", "┃", "▲", "╏", "╏", "▾"], "the line keeps its column");
    }
}

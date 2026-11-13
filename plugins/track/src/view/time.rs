//! Time view: cells are fixed time buckets, so line length is wall-clock duration.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use super::{App, Cell, Grid, Orient, Pt, Scale, head_cell, hms, line_cell, node_cell, put_str};
use crate::model::{Lane, status_at};
use crate::usage::Tracker;

/// Vertical axis gutter: `HH:MM:SS` plus a space.
pub(super) const AXIS_W: u16 = 9;

/// Start of the first cell. Following, lines start at the near edge while everything fits and
/// grow toward now; once they don't, the view scrolls with now on the last cell.
pub(super) fn start_ms(app: &App, lanes: &[usize], bucket: i64, n: usize) -> i64 {
    let span = n as i64 * bucket;
    if !app.follow {
        return (app.view_end_ms.div_euclid(bucket) + 1) * bucket - span;
    }
    let now = app.now_ms;
    let earliest = lanes.iter().map(|&i| app.state.lanes[i].opened_ms).min().unwrap_or(now);
    if now - earliest < span {
        earliest.div_euclid(bucket) * bucket
    } else {
        (now.div_euclid(bucket) + 1) * bucket - span
    }
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
    let (every, prefix) = match grid.orient {
        Orient::Horizontal => (12, "┊"),
        Orient::Vertical => (6, ""),
    };
    let dim = Style::new().fg(Color::DarkGray);
    for i in 0..grid.n() {
        let t0 = start + i as i64 * bucket;
        if (t0 / bucket).rem_euclid(every) == 0 {
            let (x, y) = grid.axis_pos(i);
            put_str(buf, grid.body, x, y, &format!("{prefix}{}", hms(t0)), dim);
        }
    }
}

pub fn render(buf: &mut Buffer, body: Rect, app: &mut App, lanes: &[usize], scale: Scale) {
    let lead = match app.orient {
        Orient::Horizontal => 1,
        Orient::Vertical => AXIS_W,
    };
    let grid = Grid { orient: app.orient, body, lead, band: scale.band };
    let n = grid.n();
    app.time_cells = n.max(1);
    let bucket = app.bucket_ms();
    let start = start_ms(app, lanes, bucket, n);

    draw_axis(buf, &grid, start, bucket);

    let (now, orient) = (app.now_ms, app.orient);
    grid.draw_lanes(buf, app, lanes, |lane, usage, n| pts(lane, usage, scale, start, bucket, n, now, orient));
}

#[cfg(test)]
mod tests {
    use super::super::tests::{col, render, row, sample_app};
    use super::super::{GUTTER, ViewKind};
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn horizontal_lane_grows_toward_now() {
        let mut app = sample_app(ViewKind::Time, Orient::Horizontal);
        // Header, axis row, lane row, detail, footer.
        let buf = render(&mut app, GUTTER + 12, 5);
        assert_eq!(row(&buf, 2, GUTTER, GUTTER + 12), "◆─●━━●━━▸");
        assert_eq!(buf[(GUTTER + 8, 2)].fg, Color::Red, "head in the current status color");
        assert_eq!(buf[(GUTTER + 1, 2)].fg, super::super::series_color(0, 1), "lines take the agent's color");
        assert_eq!(buf[(GUTTER + 2, 2)].fg, Color::Green, "status nodes keep status colors");
        assert!(row(&buf, 0, 0, GUTTER + 12).starts_with("track · TIME · ↔"));
    }

    #[test]
    fn vertical_lane_grows_downward() {
        let mut app = sample_app(ViewKind::Time, Orient::Vertical);
        // Header, lane header row + 12 cells, detail, footer.
        let buf = render(&mut app, AXIS_W + 6, 16);
        let cells = col(&buf, AXIS_W + 2, 2, 11);
        assert_eq!(cells, ["◆", "│", "●", "┃", "┃", "●", "┃", "┃", "▾"]);
        assert_eq!(buf[(AXIS_W + 2, 11)].symbol(), " ", "nothing after now");
    }

    /// Context 100 from the start, 400 from 4000 ms: the line bends up a level per cell of band.
    fn with_usage(orient: Orient) -> crate::view::App {
        use crate::usage::Sample;
        let mut app = sample_app(ViewKind::Time, orient);
        let path = std::path::PathBuf::from("/s.jsonl");
        app.state.lanes[0].sessions.push(path.clone());
        app.usage.insert(path, vec![Sample { at_ms: 0, ctx: 100, cost: 0.0 }, Sample { at_ms: 4000, ctx: 400, cost: 0.0 }]);
        app
    }

    #[test]
    fn horizontal_band_rises_with_context() {
        let mut app = with_usage(Orient::Horizontal);
        // Header, axis row, 4-row band, detail, footer.
        let buf = render(&mut app, GUTTER + 12, 8);
        assert_eq!(row(&buf, 2, GUTTER, GUTTER + 12), "    ┏●━━▸", "level 3: 400 of 400");
        assert_eq!(row(&buf, 3, GUTTER, GUTTER + 12), "    ┃");
        assert_eq!(row(&buf, 4, GUTTER, GUTTER + 12), "◆─●━┛", "level 1: 100 of 400");
        assert_eq!(row(&buf, 3, 2, GUTTER), "400 ctx");
        let wide = render(&mut app, 120, 8);
        assert!(row(&wide, 0, 0, 120).contains("ctx ≤ 400"), "{}", row(&wide, 0, 0, 120));

        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
        let buf = render(&mut app, GUTTER + 12, 8);
        assert_eq!(row(&buf, 2, GUTTER, GUTTER + 12), "◆─●━━●━━▸", "u flattens the bands");
    }

    #[test]
    fn vertical_band_moves_right_with_context() {
        let mut app = with_usage(Orient::Vertical);
        let buf = render(&mut app, AXIS_W + 6, 16);
        let x = AXIS_W;
        assert_eq!(row(&buf, 2, x, x + 6), "  ◆");
        assert_eq!(row(&buf, 6, x, x + 6), "  ┗━┓", "bend at 4000 ms");
        assert_eq!(row(&buf, 10, x, x + 6), "    ▾");
    }
}

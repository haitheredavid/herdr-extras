//! Time view: cells are fixed time buckets, so line length is wall-clock duration.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use super::{App, Cell, Grid, Orient, head_cell, hms, line_cell, node_cell, put_str};
use crate::model::{Lane, status_at};

/// Vertical axis gutter: `HH:MM:SS` plus a space.
const AXIS_W: u16 = 9;

/// Start of the first cell. Following, lines start at the near edge while everything fits and
/// grow toward now; once they don't, the view scrolls with now on the last cell.
fn start_ms(app: &App, lanes: &[usize], bucket: i64, n: usize) -> i64 {
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

fn cells(lane: &Lane, start: i64, bucket: i64, n: usize, now_ms: i64, orient: Orient) -> Vec<Cell> {
    (0..n)
        .map(|i| {
            let t0 = start + i as i64 * bucket;
            let t1 = t0 + bucket;
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
        })
        .collect()
}

pub fn render(buf: &mut Buffer, body: Rect, app: &mut App, lanes: &[usize]) {
    let lead = match app.orient {
        Orient::Horizontal => 1,
        Orient::Vertical => AXIS_W,
    };
    let grid = Grid { orient: app.orient, body, lead };
    let n = grid.n();
    app.time_cells = n.max(1);
    let bucket = app.bucket_ms();
    let start = start_ms(app, lanes, bucket, n);

    // Axis labels sit on absolute bucket multiples so they scroll with time.
    let (every, prefix) = match app.orient {
        Orient::Horizontal => (12, "┊"),
        Orient::Vertical => (6, ""),
    };
    let dim = Style::new().fg(Color::DarkGray);
    for i in 0..n {
        let t0 = start + i as i64 * bucket;
        if (t0 / bucket).rem_euclid(every) == 0 {
            let (x, y) = grid.axis_pos(i);
            put_str(buf, body, x, y, &format!("{prefix}{}", hms(t0)), dim);
        }
    }

    let (now, orient) = (app.now_ms, app.orient);
    grid.draw_lanes(buf, app, lanes, |lane, n| cells(lane, start, bucket, n, now, orient));
}

#[cfg(test)]
mod tests {
    use super::super::tests::{col, render, row, sample_app};
    use super::super::{GUTTER, ViewKind};
    use super::*;

    #[test]
    fn horizontal_lane_grows_toward_now() {
        let mut app = sample_app(ViewKind::Time, Orient::Horizontal);
        // Header, axis row, lane row, detail, footer.
        let buf = render(&mut app, GUTTER + 12, 5);
        assert_eq!(row(&buf, 2, GUTTER, GUTTER + 12), "◆─●━━●━━▸");
        assert_eq!(buf[(GUTTER + 8, 2)].fg, Color::Red, "head in the current status color");
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
}

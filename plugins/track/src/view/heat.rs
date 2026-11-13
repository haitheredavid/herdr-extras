//! Heat view: one row (↔) or column (↕) per agent, one cell per time bucket, shaded along the
//! theme's ramp (the same stretch agent colors use) by the share of the bucket the agent spent
//! working; `heat_blocked` where it was blocked at all.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::time::{AXIS_W, draw_axis, start_ms};
use super::{App, Grid, Orient, Pt, put_str};
use crate::theme::theme;
use crate::model::{Lane, status_time};

fn pts(lane: &Lane, start: i64, bucket: i64, n: usize, now_ms: i64) -> Vec<Pt> {
    (0..n)
        .map(|i| {
            let t0 = start + i as i64 * bucket;
            let cell = (t0 <= now_ms)
                .then(|| status_time(lane, t0, (t0 + bucket).min(now_ms + 1)))
                .filter(|st| st.alive_ms > 0)
                .map(|st| {
                    let color = if st.blocked_ms > 0 {
                        theme().heat_blocked
                    } else {
                        theme().heat_color(st.working_ms as f64 / st.alive_ms as f64)
                    };
                    ('█', color)
                });
            Pt { cell, level: 0, status: None }
        })
        .collect()
}

pub fn render(buf: &mut Buffer, body: Rect, app: &mut App, lanes: &[usize]) {
    let horizontal = app.orient == Orient::Horizontal;
    let grid = Grid { orient: app.orient, body, lead: if horizontal { 1 } else { AXIS_W }, band: 1 };
    let n = grid.n();
    app.time_cells = n.max(1);
    let bucket = app.bucket_ms();
    let start = start_ms(app, lanes, bucket, n);
    draw_axis(buf, &grid, start, bucket);

    // Scale key in the corner the axis leaves free.
    let dim = Style::new().fg(theme().dim);
    let steps = if horizontal { 10 } else { 6 };
    let mut x = put_str(buf, body, body.x, body.y, if horizontal { "idle " } else { "0" }, dim);
    for i in 0..steps {
        x = put_str(buf, body, x, body.y, "█", Style::new().fg(theme().heat_color(i as f64 / (steps - 1) as f64)));
    }
    put_str(buf, body, x, body.y, if horizontal { " work" } else { "1" }, dim);

    let now = app.now_ms;
    grid.draw_lanes(buf, app, lanes, |lane, _, n| pts(lane, start, bucket, n, now));
}

#[cfg(test)]
mod tests {
    use super::super::tests::{render, row, sample_app};
    use super::super::{GUTTER, ViewKind};
    use super::*;

    #[test]
    fn cells_shade_by_working_share() {
        // Idle from 0, working 2000..5000, blocked from 5000; now 8500; 1 s buckets.
        let mut app = sample_app(ViewKind::Heat, Orient::Horizontal);
        let buf = render(&mut app, GUTTER + 12, 5);
        assert_eq!(row(&buf, 2, GUTTER, GUTTER + 12), "█████████", "one cell per second up to now");
        let fg = |i: u16| buf[(GUTTER + i, 2)].fg;
        assert_eq!(fg(0), theme().heat_color(0.0), "idle");
        assert_eq!(fg(3), theme().heat_color(1.0), "working the whole second");
        assert_eq!(fg(6), theme().heat_blocked, "blocked");
    }
}

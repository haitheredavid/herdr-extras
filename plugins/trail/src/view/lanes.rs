//! LANES: one lane per agent through the selected lens, on the time axis (status, heat, ctx) or the
//! steps axis (status only).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::time::{axis_w, cells, draw_axis, start_ms};
use super::{App, Axis, BAND, Grid, Lens, Orient, Scale, ctx, heat, row_height, sequence, time};

pub fn render(buf: &mut Buffer, body: Rect, app: &mut App, rows: &[usize]) {
    let timed = !app.steps();
    let horizontal = app.orient == Orient::Horizontal;
    if timed {
        app.fit_bucket(rows, cells(app, body));
    }
    let bucket = app.bucket_ms();
    let lead = match (timed, horizontal) {
        (false, _) => 0,
        (true, true) => 1,
        (true, false) => axis_w(bucket),
    };
    let all_fit = rows.len() * usize::from(BAND + 1) <= usize::from(body.height.saturating_sub(lead)) + 1;
    let heights: Vec<u16> = rows.iter().map(|&li| row_height(app, &app.state.lanes[li], all_fit)).collect();
    let scale = Scale::of(app, rows);
    if heights.iter().any(|&h| h > 1) {
        app.band_max = Some(scale.max);
    }
    let grid = Grid::new(app.orient, body, lead, heights);
    let n = grid.n();
    app.time_cells = n.max(1);
    let start = start_ms(app, bucket, n);
    if timed {
        draw_axis(buf, &grid, start, bucket);
    }

    let (now, orient, back) = (app.now_ms, app.orient, app.seq_back);
    match (app.lens, app.axis) {
        (Lens::Status, Axis::Time) => {
            grid.draw_lanes(buf, app, rows, |lane, s, n, band| time::pts(lane, s, scale, band, start, bucket, n, now, orient))
        }
        (Lens::Status, Axis::Steps) => {
            grid.draw_lanes(buf, app, rows, |lane, s, n, band| sequence::pts(lane, s, scale, band, now, n, back, orient))
        }
        (Lens::Heat, _) => grid.draw_lanes(buf, app, rows, |lane, _, n, _| heat::pts(lane, start, bucket, n, now, orient)),
        (Lens::Ctx, _) => {
            let max = ctx::max(app, rows, start, start + n as i64 * bucket);
            grid.draw_lanes(buf, app, rows, |lane, s, n, _| ctx::pts(lane, s, max, start, bucket, n, now));
            ctx::totals(buf, &grid, app, rows);
        }
    }
}

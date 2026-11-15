//! Sequence view: position is the step index, not time, so every lane starts at the near edge and
//! each state change gets the same room.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{App, Grid, Orient, Pt, Scale, head_cell, line_cell, node_glyph};
use crate::model::Lane;
use crate::theme::theme;
use crate::usage::Tracker;

/// Cells per step: the node, then connectors in the status it set.
const STEP_W: usize = 4;
const STEP_H: usize = 2;

/// Step k's node and connectors sit at the context in use when it happened; the head at now's.
fn pts(lane: &Lane, usage: &Tracker, scale: Scale, now_ms: i64, n: usize, seq_back: usize, orient: Orient) -> Vec<Pt> {
    let step = match orient {
        Orient::Horizontal => STEP_W,
        Orient::Vertical => STEP_H,
    };
    let len = lane.steps.len();
    if len == 0 {
        return Vec::new();
    }
    let back = seq_back.min(len - 1);
    let end = len - back;
    let head = lane.is_open() && back == 0;
    // A closed lane ends at its Closed/Exited node; every other step trails connectors.
    let width = |k: usize| if k == len - 1 && !lane.is_open() { 1 } else { step };
    let need = |s: usize| (s..end).map(width).sum::<usize>() + usize::from(head) + usize::from(s > 0);
    let first = (0..end).find(|&s| need(s) <= n).unwrap_or(end - 1);

    let level_at = |t_ms: i64| scale.level(usage.ctx_at(&lane.sessions, t_ms));
    let mut out: Vec<Pt> = Vec::with_capacity(n);
    if first > 0 {
        let s = &lane.steps[first];
        out.push(Pt { cell: Some(('…', theme().dim)), level: level_at(s.at_ms), status: Some(s.status) });
    }
    for k in first..end {
        let s = &lane.steps[k];
        let (level, status) = (level_at(s.at_ms), Some(s.status));
        out.push(Pt { cell: Some(node_glyph(s.mark)), level, status });
        if width(k) > 1 {
            out.extend(std::iter::repeat_n(Pt { cell: line_cell(s.status, orient), level, status }, step - 1));
        }
    }
    if head {
        out.push(Pt { cell: head_cell(lane.status, orient), level: level_at(now_ms), status: Some(lane.status) });
    }
    out
}

pub fn render(buf: &mut Buffer, body: Rect, app: &mut App, lanes: &[usize]) {
    let scale = Scale::of(app, lanes, body.height);
    if scale.band > 1 {
        app.band_max = Some(scale.max);
    }
    let grid = Grid::new(app.orient, body, 0, scale.band, lanes.len());
    let (back, orient, now) = (app.seq_back, app.orient, app.now_ms);
    grid.draw_lanes(buf, app, lanes, |lane, usage, n| pts(lane, usage, scale, now, n, back, orient));
}

#[cfg(test)]
mod tests {
    use super::super::ViewKind;
    use super::super::tests::{render, row, sample_app};
    use super::*;
    use crate::model::{AgentStatus, Mark};

    #[test]
    fn horizontal_steps_are_evenly_spaced() {
        let mut app = sample_app(ViewKind::Sequence, Orient::Horizontal);
        // Header, lane row: no axis row. The strip starts at column 33.
        let buf = render(&mut app, 60, 4);
        assert_eq!(row(&buf, 1, 33, 60), "◆───●━━━▲╍╍╍▸");
    }

    #[test]
    fn overflow_hides_oldest_steps_behind_ellipsis() {
        let mut app = sample_app(ViewKind::Sequence, Orient::Horizontal);
        for k in 0..10 {
            let s = if k % 2 == 0 { AgentStatus::Idle } else { AgentStatus::Working };
            app.state.set_status(0, s, 6000 + k);
        }
        let buf = render(&mut app, 60, 4);
        let lane = row(&buf, 1, 33, 60);
        assert!(lane.starts_with('…'), "{lane}");
        assert!(lane.ends_with('▸'), "{lane}");
    }

    #[test]
    fn seq_back_and_close_end_the_lane_at_a_node() {
        let mut app = sample_app(ViewKind::Sequence, Orient::Horizontal);
        app.seq_back = 1;
        let buf = render(&mut app, 60, 4);
        assert_eq!(row(&buf, 1, 33, 60), "◆───●━━━", "last step hidden, no head");

        app.seq_back = 0;
        app.state.close("w1:p1", Mark::Closed, 9000);
        app.query.show_closed = true;
        let buf = render(&mut app, 60, 4);
        assert_eq!(row(&buf, 1, 33, 60), "◆───●━━━▲╍╍╍✕");
    }
}

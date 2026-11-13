//! Sequence view: position is the step index, not time, so every lane starts at the near edge and
//! each state change gets the same room.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::{App, Cell, Grid, Orient, head_cell, line_cell, node_glyph};
use crate::model::Lane;

/// Cells per step: the node, then connectors in the status it set.
const STEP_W: usize = 4;
const STEP_H: usize = 2;

fn cells(lane: &Lane, n: usize, seq_back: usize, orient: Orient) -> Vec<Cell> {
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

    let mut out: Vec<Cell> = Vec::with_capacity(n);
    if first > 0 {
        out.push(Some(('…', Color::DarkGray)));
    }
    for k in first..end {
        let s = &lane.steps[k];
        out.push(Some(node_glyph(s.mark)));
        if width(k) > 1 {
            out.extend(std::iter::repeat_n(line_cell(s.status, orient), step - 1));
        }
    }
    if head {
        out.push(head_cell(lane.status, orient));
    }
    out
}

pub fn render(buf: &mut Buffer, body: Rect, app: &mut App, lanes: &[usize]) {
    let grid = Grid { orient: app.orient, body, lead: 0 };
    let (back, orient) = (app.seq_back, app.orient);
    grid.draw_lanes(buf, app, lanes, |lane, n| cells(lane, n, back, orient));
}

#[cfg(test)]
mod tests {
    use super::super::tests::{render, row, sample_app};
    use super::super::{GUTTER, ViewKind};
    use super::*;
    use crate::model::{AgentStatus, Mark};

    #[test]
    fn horizontal_steps_are_evenly_spaced() {
        let mut app = sample_app(ViewKind::Sequence, Orient::Horizontal);
        // Header, lane row, detail, footer: no axis row.
        let buf = render(&mut app, GUTTER + 20, 4);
        assert_eq!(row(&buf, 1, GUTTER, GUTTER + 20), "◆───●━━━●━━━▸");
    }

    #[test]
    fn overflow_hides_oldest_steps_behind_ellipsis() {
        let mut app = sample_app(ViewKind::Sequence, Orient::Horizontal);
        for k in 0..10 {
            let s = if k % 2 == 0 { AgentStatus::Idle } else { AgentStatus::Working };
            app.state.set_status(0, s, 6000 + k);
        }
        let buf = render(&mut app, GUTTER + 12, 4);
        let lane = row(&buf, 1, GUTTER, GUTTER + 12);
        assert!(lane.starts_with('…'), "{lane}");
        assert!(lane.ends_with('▸'), "{lane}");
    }

    #[test]
    fn seq_back_and_close_end_the_lane_at_a_node() {
        let mut app = sample_app(ViewKind::Sequence, Orient::Horizontal);
        app.seq_back = 1;
        let buf = render(&mut app, GUTTER + 20, 4);
        assert_eq!(row(&buf, 1, GUTTER, GUTTER + 20), "◆───●━━━", "last step hidden, no head");

        app.seq_back = 0;
        app.state.close("w1:p1", Mark::Closed, 9000);
        let buf = render(&mut app, GUTTER + 20, 4);
        assert_eq!(row(&buf, 1, GUTTER, GUTTER + 20), "◆───●━━━●━━━✕");
    }
}

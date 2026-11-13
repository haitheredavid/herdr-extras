//! Area view: context in use per agent, stacked over time. Each agent is a layer in its own color
//! (bright while working, dimmed idle or done, red when blocked), and the stack's edge is the total.
//! Layer boundaries use eighth blocks, so a cell shows the lower layer in front and the upper one
//! behind.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::time::{AXIS_W, draw_axis, start_ms};
use super::{App, GUTTER, Grid, Orient, put_str, truncate};
use crate::model::{AgentStatus, status_at};
use crate::usage::fmt_tokens;

const PALETTE: [(u8, u8, u8); 6] =
    [(97, 175, 239), (198, 120, 221), (229, 192, 123), (86, 182, 194), (152, 195, 121), (209, 154, 102)];
/// Partial cells, by eighths filled from the stack's base: lower blocks in ↔, left blocks in ↕.
const LOWER: [char; 8] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇'];
const LEFT: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];

/// Layer color for the `k`th visible lane in `status`.
fn layer_color(k: usize, status: AgentStatus) -> Color {
    let (r, g, b) = PALETTE[k % PALETTE.len()];
    let dim = |c: u8| (u16::from(c) * 45 / 100) as u8;
    match status {
        AgentStatus::Working => Color::Rgb(r, g, b),
        AgentStatus::Blocked => Color::Rgb(224, 108, 117),
        AgentStatus::Idle | AgentStatus::Done | AgentStatus::Unknown => Color::Rgb(dim(r), dim(g), dim(b)),
    }
}

/// Cells across the stack for one column: `layers` bottom-up as (color, tokens), scaled so `max`
/// fills `depth` cells. Each cell is (glyph, front color, back color).
fn stack(layers: &[(Color, u64)], max: u64, depth: usize, blocks: &[char; 8]) -> Vec<(char, Color, Color)> {
    let units = depth as u64 * 8;
    let mut cum = 0;
    let tops: Vec<(Color, u64)> = layers
        .iter()
        .map(|&(c, ctx)| {
            cum += ctx;
            (c, (cum * units + max / 2) / max.max(1))
        })
        .collect();
    let owner = |e: u64| tops.iter().find(|(_, top)| e < *top).map(|(c, _)| *c);
    let mut out = Vec::new();
    for r in 0..depth as u64 {
        let lo = r * 8;
        let Some(front) = owner(lo) else { break };
        let cell = match (lo + 1..lo + 8).find(|&e| owner(e) != Some(front)) {
            Some(e) => (blocks[(e - lo) as usize], front, owner(e).unwrap_or(Color::Reset)),
            None => ('█', front, Color::Reset),
        };
        out.push(cell);
    }
    out
}

pub fn render(buf: &mut Buffer, body: Rect, app: &mut App, lanes: &[usize]) {
    let horizontal = app.orient == Orient::Horizontal;
    let lead = if horizontal { 1 } else { AXIS_W };
    let grid = Grid { orient: app.orient, body, lead, band: 1 };
    let n = grid.n();
    let depth = if horizontal { body.height.saturating_sub(1) } else { body.width.saturating_sub(AXIS_W) } as usize;
    app.time_cells = n.max(1);
    let bucket = app.bucket_ms();
    let start = start_ms(app, lanes, bucket, n);
    let now = app.now_ms;

    let columns: Vec<Vec<(Color, u64)>> = (0..n)
        .map(|i| {
            let t0 = start + i as i64 * bucket;
            if t0 > now {
                return Vec::new();
            }
            let at = (t0 + bucket - 1).min(now);
            lanes
                .iter()
                .enumerate()
                .filter_map(|(k, &li)| {
                    let lane = &app.state.lanes[li];
                    let status = status_at(lane, at)?;
                    let ctx = app.usage.ctx_at(&lane.sessions, at).filter(|&c| c > 0)?;
                    Some((layer_color(k, status), ctx))
                })
                .collect()
        })
        .collect();
    let max = columns.iter().map(|c| c.iter().map(|l| l.1).sum::<u64>()).max().unwrap_or(0);

    draw_axis(buf, &grid, start, bucket);
    let dim = Style::new().fg(Color::DarkGray);
    put_str(buf, body, body.x, body.y, &format!("Σ ≤ {}", fmt_tokens(max)), dim);
    if max == 0 {
        let (x, y) = if horizontal { (body.x + GUTTER, body.y + 1) } else { (body.x + AXIS_W, body.y + 1) };
        put_str(buf, body, x, y, "no usage yet — needs omp session usage", dim);
    }

    // Legend: the top layer first in ↔ (matching the stack), left to right in ↕.
    let legend: Vec<(usize, usize)> =
        if horizontal { lanes.iter().copied().enumerate().rev().collect() } else { lanes.iter().copied().enumerate().collect() };
    for (row, &(k, li)) in legend.iter().enumerate() {
        let lane = &app.state.lanes[li];
        let swatch = Style::new().fg(layer_color(k, lane.status));
        let mut label = if lane.is_open() { Style::new() } else { dim };
        if k == app.selected {
            label = label.add_modifier(Modifier::REVERSED);
        }
        let ctx = app.usage.ctx_at(&lane.sessions, lane.closed_ms.unwrap_or(now)).map_or("-".into(), fmt_tokens);
        if horizontal {
            let y = body.y + 1 + row as u16;
            if y >= body.bottom() {
                break;
            }
            put_str(buf, body, body.x, y, "█", swatch);
            put_str(buf, body, body.x + 2, y, &format!("{:<12} {ctx:>6}", truncate(lane.label(), 12)), label);
        } else {
            let x = body.x + AXIS_W + row as u16 * 6;
            put_str(buf, body, x, body.y, "█", swatch);
            put_str(buf, body, x + 1, body.y, &truncate(lane.label(), 4), label);
        }
    }

    let blocks = if horizontal { &LOWER } else { &LEFT };
    for (i, layers) in columns.iter().enumerate() {
        for (r, (ch, fg, bg)) in stack(layers, max, depth, blocks).into_iter().enumerate() {
            let (x, y) = if horizontal {
                (body.x + GUTTER + i as u16, body.bottom() - 1 - r as u16)
            } else {
                (body.x + AXIS_W + r as u16, body.y + 1 + i as u16)
            };
            if x < body.right() && y < body.bottom() {
                buf[(x, y)].set_char(ch).set_style(Style::new().fg(fg).bg(bg));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::ViewKind;
    use super::super::tests::{render, sample_app};
    use super::*;
    use crate::model::Mark;
    use crate::usage::Sample;
    use std::path::PathBuf;

    /// Lane 0 from `sample_app` (idle, working at 2000, blocked at 5000) at 150 tokens, under a
    /// working lane 1 at 250.
    fn stacked(orient: Orient) -> App {
        let mut app = sample_app(ViewKind::Area, orient);
        let i = app.state.open("w1:p2", "w1", 0, Mark::Created, AgentStatus::Working, true);
        app.state.lanes[i].agent = Some("claude".into());
        for (li, ctx) in [(0, 150), (i, 250)] {
            let path = PathBuf::from(format!("/s{li}.jsonl"));
            app.state.lanes[li].sessions.push(path.clone());
            app.usage.insert(path, vec![Sample { at_ms: 0, ctx }]);
        }
        app
    }

    #[test]
    fn layers_stack_with_eighth_block_boundaries() {
        let mut app = stacked(Orient::Horizontal);
        // Header, axis row, 4 chart rows (y 2..=5), detail, footer.
        let buf = render(&mut app, GUTTER + 12, 8);
        let (idle0, blocked, work1) = (layer_color(0, AgentStatus::Idle), layer_color(0, AgentStatus::Blocked), layer_color(1, AgentStatus::Working));
        let x = GUTTER;
        // 150 of 400 over 32 eighths: lane 0 fills 12, so row 1 is half lane 0 in front of lane 1.
        assert_eq!((buf[(x, 5)].symbol(), buf[(x, 5)].fg), ("█", idle0));
        assert_eq!((buf[(x, 4)].symbol(), buf[(x, 4)].fg, buf[(x, 4)].bg), ("▄", idle0, work1));
        assert_eq!((buf[(x, 3)].symbol(), buf[(x, 3)].fg), ("█", work1));
        assert_eq!((buf[(x, 2)].symbol(), buf[(x, 2)].fg), ("█", work1));
        assert_eq!(buf[(x + 5, 5)].fg, blocked, "lane 0 blocked from 5000 ms");
        assert_eq!(buf[(x + 9, 2)].symbol(), " ", "nothing after now");
        assert_eq!(buf[(0, 2)].fg, work1, "legend lists the top layer first");
    }

    #[test]
    fn vertical_stack_grows_rightward() {
        let mut app = stacked(Orient::Vertical);
        // Header, legend row, 12 time rows, detail, footer; 4 cells of stack.
        let buf = render(&mut app, AXIS_W + 4, 16);
        let row: String = (AXIS_W..AXIS_W + 4).map(|x| buf[(x, 2)].symbol().to_string()).collect();
        assert_eq!(row, "█▌██");
    }
}

//! Dash view: one table row per agent over the last `HISTORY_MS`: status, name, workspace, a context
//! sparkline in the agent's color, current context, share of time working, spend, requests, and
//! the current status with how long it has held. Always a table, whatever the orientation.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::gutter::fmt_cost;
use super::{App, fmt_dur, put_str, status_glyph, truncate};
use crate::model::{HISTORY_MS, status_time};
use crate::theme::theme;
use crate::usage::fmt_tokens;

const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// Partial cells of the working bar, by eighths filled from the left.
const BAR: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
const BAR_W: usize = 8;

/// Fixed column widths (each followed by one space); the sparkline takes what is left.
const DOT_W: u16 = 2;
const NAME_W: u16 = 16;
const WS_W: u16 = 9;
const CTX_W: u16 = 5;
const WORK_W: u16 = BAR_W as u16 + 5;
const COST_W: u16 = 8;
const REQ_W: u16 = 5;
const STATE_W: u16 = 14;
const FIXED_W: u16 = DOT_W + (NAME_W + 1) + (WS_W + 1) + (CTX_W + 1) + (WORK_W + 1) + (COST_W + 1) + (REQ_W + 1) + STATE_W;
/// Narrower than this, the sparkline column is dropped.
const MIN_SPARK: u16 = 6;

/// `BAR_W` cells filled to `share` in eighths.
fn bar(share: f64) -> String {
    let eighths = (share.clamp(0.0, 1.0) * (BAR_W * 8) as f64).round() as usize;
    let mut s = "█".repeat(eighths / 8);
    if !eighths.is_multiple_of(8) {
        s.push(BAR[eighths % 8]);
    }
    format!("{s:<BAR_W$}")
}

pub fn render(buf: &mut Buffer, body: Rect, app: &mut App, lanes: &[usize]) {
    let now = app.now_ms;
    let from = now - HISTORY_MS;
    let dim = Style::new().fg(theme().dim);
    let spark_w = body.width.saturating_sub(FIXED_W + 1);
    let spark_w = if spark_w >= MIN_SPARK { spark_w } else { 0 };
    let cell_ms = HISTORY_MS / i64::from(spark_w.max(1));
    let spark_max = lanes
        .iter()
        .filter_map(|&li| {
            let l = &app.state.lanes[li];
            app.usage.max_between(&l.sessions, from, l.closed_ms.unwrap_or(now))
        })
        .max()
        .unwrap_or(0);

    // Column starts.
    let x_name = body.x + DOT_W;
    let x_ws = x_name + NAME_W + 1;
    let x_spark = x_ws + WS_W + 1;
    let x_ctx = x_spark + if spark_w > 0 { spark_w + 1 } else { 0 };
    let x_work = x_ctx + CTX_W + 1;
    let x_cost = x_work + WORK_W + 1;
    let x_req = x_cost + COST_W + 1;
    let x_state = x_req + REQ_W + 1;

    let y0 = body.y;
    put_str(buf, body, x_name, y0, "agent", dim);
    put_str(buf, body, x_ws, y0, "workspace", dim);
    if spark_w > 0 {
        put_str(buf, body, x_spark, y0, &format!("context {}h", HISTORY_MS / 3_600_000), dim);
    }
    put_str(buf, body, x_ctx, y0, "  ctx", dim);
    put_str(buf, body, x_work, y0, "working", dim);
    put_str(buf, body, x_cost, y0, "    cost", dim);
    put_str(buf, body, x_req, y0, " reqs", dim);
    put_str(buf, body, x_state, y0, "state", dim);

    let rows = body.height.saturating_sub(1) as usize;
    let sel = app.sel_row(lanes).unwrap_or(0);
    if sel < app.lane_scroll {
        app.lane_scroll = sel;
    } else if rows > 0 && sel >= app.lane_scroll + rows {
        app.lane_scroll = sel + 1 - rows;
    }
    let shown = rows.min(lanes.len().saturating_sub(app.lane_scroll));
    app.shown = (app.lane_scroll, shown, lanes.len().saturating_sub(app.lane_scroll + shown));
    for (row, (k, &li)) in lanes.iter().enumerate().skip(app.lane_scroll).take(rows).enumerate() {
        let lane = &app.state.lanes[li];
        let y = y0 + 1 + row as u16;
        let end = lane.closed_ms.unwrap_or(now);
        let mut label = if lane.is_open() { Style::new() } else { dim };
        if k == sel {
            label = label.add_modifier(Modifier::REVERSED);
        }
        let (dot, dot_style) =
            if lane.is_open() { (status_glyph(lane.status), Style::new().fg(theme().status(lane.status))) } else { ('✕', dim) };
        put_str(buf, body, body.x, y, &dot.to_string(), dot_style);
        put_str(buf, body, x_name, y, &format!("{:<w$}", truncate(lane.label(), NAME_W as usize), w = NAME_W as usize), label);
        put_str(buf, body, x_ws, y, &truncate(app.state.workspace_label(li), WS_W as usize), dim);

        if spark_w > 0 && spark_max > 0 {
            let color = theme().agent_color(lane.slot.unwrap_or(0));
            let spark: String = (0..spark_w)
                .map(|i| {
                    let at = (from + (i64::from(i) + 1) * cell_ms - 1).min(now);
                    if at >= end {
                        return ' ';
                    }
                    app.usage
                        .ctx_at(&lane.sessions, at)
                        .map_or(' ', |c| SPARK[((c as f64 / spark_max as f64) * 7.0).round().clamp(0.0, 7.0) as usize])
                })
                .collect();
            put_str(buf, body, x_spark, y, &spark, Style::new().fg(color));
        }

        let ctx = app.usage.ctx_at(&lane.sessions, end).map_or("–".into(), fmt_tokens);
        put_str(buf, body, x_ctx, y, &format!("{ctx:>5}"), Style::new());

        let st = status_time(lane, from, now);
        let share = if st.alive_ms > 0 { st.working_ms as f64 / st.alive_ms as f64 } else { 0.0 };
        put_str(buf, body, x_work, y, &bar(share), Style::new().fg(theme().working));
        put_str(buf, body, x_work + BAR_W as u16, y, &format!("{:>4}%", (share * 100.0).round() as u32), Style::new());

        let (reqs, cost) = app.usage.spend_between(&lane.sessions, from, now);
        let has_usage = app.usage.has_samples(&lane.sessions);
        put_str(buf, body, x_cost, y, &format!("{:>8}", fmt_cost(has_usage.then_some(cost))), Style::new());
        let reqs = if has_usage { reqs.to_string() } else { "–".into() };
        put_str(buf, body, x_req, y, &format!("{reqs:>5}"), Style::new());

        let state = match lane.closed_ms {
            Some(_) => "closed".to_string(),
            None => format!("{} {}", lane.status.as_str(), fmt_dur(now - lane.last_ms())),
        };
        put_str(buf, body, x_state, y, &truncate(&state, STATE_W as usize), Style::new().fg(theme().status(lane.status)));
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{render, row, sample_app};
    use super::super::{Orient, ViewKind};
    use super::*;
    use crate::usage::Sample;
    use std::path::PathBuf;

    #[test]
    fn bar_fills_by_eighths() {
        assert_eq!(bar(0.0), "        ");
        assert_eq!(bar(1.0), "████████");
        assert_eq!(bar(0.5), "████    ");
        assert_eq!(bar(1.0 / 16.0), "▌       ");
    }

    #[test]
    fn row_shows_share_spend_and_state() {
        // Idle 0..2000, working 2000..5000, blocked 5000..8500: 3000 of 8500 ms working, 23 eighths.
        let mut app = sample_app(ViewKind::Dash, Orient::Horizontal);
        let path = PathBuf::from("/s.jsonl");
        app.state.lanes[0].sessions.push(path.clone());
        app.usage.insert(
            path,
            vec![Sample { at_ms: 1000, ctx: 120_000, cost: 0.5 }, Sample { at_ms: 3000, ctx: 150_000, cost: 0.75 }],
        );
        let buf = render(&mut app, FIXED_W + 1, 6);
        let line = row(&buf, 2, 0, FIXED_W + 1);
        assert!(line.contains("150k"), "{line}");
        assert!(line.contains("██▉ ") && line.contains("35%"), "{line}");
        assert!(line.contains("$1.25") && line.contains("    2 "), "{line}");
        assert!(line.contains("blocked 3s"), "{line}");
    }
}

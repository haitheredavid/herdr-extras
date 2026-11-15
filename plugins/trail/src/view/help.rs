//! `?` overlay: every key, the glyph legend, and whatever `theme.toml` / `config.toml` couldn't use.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Widget};

use crate::config::config_problems;
use crate::theme::{theme, theme_problems};

/// Sections of (key, meaning).
const HELP: &[(&str, &[(&str, &str)])] = &[
    (
        "MOVE",
        &[
            ("j/k ↓/↑", "next/previous agent (l/h →/← in columns)"),
            ("h/l ←/→", "back/forward in time (k/j ↑/↓ in columns)"),
            ("f", "follow now"),
            ("0", "follow now and fit time to the history"),
        ],
    ),
    (
        "SHOW",
        &[
            ("Tab 1-5", "next view / TIME SEQUENCE AREA HEAT DASH"),
            ("o", "agents as rows or columns"),
            ("+/-", "zoom time in/out"),
            ("s", "AREA: stacked, overlaid or stream"),
            ("u", "usage bands on/off"),
            ("c", "show closed agents"),
        ],
    ),
    ("ACT", &[("Enter", "jump to the agent's pane"), ("r", "refresh now"), ("?", "this help"), ("q", "quit (Esc in a popup)")]),
];

const LEGEND: [&str; 2] = [
    "◆ created  ◇ seen  ● working  ▲ blocked  ✓ done  ○ idle  · unknown  ✕ closed  ◉ several  ▸ now",
    "━ working/done  ╍ blocked  ─ idle  ┈ unknown",
];

fn lines() -> Vec<Line<'static>> {
    let dim = Style::new().fg(theme().dim);
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let mut out = Vec::new();
    for (section, keys) in HELP {
        out.push(Line::styled(*section, bold));
        for (key, what) in *keys {
            out.push(Line::from(vec![Span::raw(format!("  {key:<10}")), Span::styled(*what, dim)]));
        }
    }
    out.push(Line::raw(""));
    out.extend(LEGEND.iter().map(|l| Line::styled(*l, dim)));
    let error = Style::new().fg(theme().error);
    for (file, problems) in [("theme.toml", theme_problems()), ("config.toml", config_problems())] {
        out.extend(problems.iter().map(|p| Line::styled(format!("{file} {p}"), error)));
    }
    out
}

/// Draws the overlay centered over `body`.
pub(super) fn draw(buf: &mut Buffer, body: Rect) {
    let lines = lines();
    let w = body.width.min(96);
    let h = (lines.len() as u16 + 2).min(body.height);
    let area = Rect::new(body.x + (body.width - w) / 2, body.y + (body.height - h) / 2, w, h);
    Clear.render(area, buf);
    Paragraph::new(lines).block(Block::bordered().title(" keys ")).render(area, buf);
}

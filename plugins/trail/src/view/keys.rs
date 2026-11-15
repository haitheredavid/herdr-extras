//! Keys. Letters mean the same in every orientation (`j`/`k` agents, `h`/`l` time); arrows are
//! spatial, so they follow the agents' axis: rows in ↔, columns in ↕.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{App, AreaMode, Axis, BUCKETS_S, Density, Lens, Mode, Orient, sequence};

impl App {
    pub fn on_key(&mut self, key: KeyEvent) {
        if self.help {
            match key.code {
                KeyCode::Char('?') | KeyCode::Esc => self.help = false,
                KeyCode::Char('q') => self.quit = true,
                _ => {}
            }
            return;
        }
        let rows = self.orient == Orient::Horizontal;
        let code = match (key.code, rows) {
            (KeyCode::Down, true) | (KeyCode::Right, false) => KeyCode::Char('j'),
            (KeyCode::Up, true) | (KeyCode::Left, false) => KeyCode::Char('k'),
            (KeyCode::Left, true) | (KeyCode::Up, false) => KeyCode::Char('h'),
            (KeyCode::Right, true) | (KeyCode::Down, false) => KeyCode::Char('l'),
            (c, _) => c,
        };
        match code {
            KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::PageDown => self.move_selection(self.shown.1.max(1) as isize),
            KeyCode::PageUp => self.move_selection(-(self.shown.1.max(1) as isize)),
            KeyCode::Home => self.move_selection(isize::MIN / 2),
            KeyCode::End => self.move_selection(isize::MAX / 2),
            KeyCode::Char('h') => self.scroll_time(true, false),
            KeyCode::Char('l') => self.scroll_time(false, false),
            KeyCode::Char('H') => self.scroll_time(true, true),
            KeyCode::Char('L') => self.scroll_time(false, true),
            KeyCode::Tab | KeyCode::BackTab => {
                self.mode = match self.mode {
                    Mode::Lanes => Mode::Usage,
                    Mode::Usage => Mode::Lanes,
                }
            }
            KeyCode::Char(c @ '1'..='5') => self.preset(c as u8 - b'0'),
            KeyCode::Char('v') => match self.mode {
                Mode::Lanes => {
                    self.lens = match self.lens {
                        Lens::Status => Lens::Heat,
                        Lens::Heat => Lens::Ctx,
                        Lens::Ctx => Lens::Status,
                    }
                }
                Mode::Usage => {
                    self.area_mode = match self.area_mode {
                        AreaMode::Stacked => AreaMode::Overlaid,
                        AreaMode::Overlaid => AreaMode::Stacked,
                    }
                }
            },
            KeyCode::Char('t') if self.mode == Mode::Lanes && self.lens == Lens::Status => {
                self.axis = match self.axis {
                    Axis::Time => Axis::Steps,
                    Axis::Steps => Axis::Time,
                }
            }
            KeyCode::Char(' ') => {
                if let Some(li) = self.selected_lane() {
                    let id = self.state.lanes[li].id;
                    if !self.expanded.remove(&id) {
                        self.expanded.insert(id);
                    }
                }
            }
            KeyCode::Char('Z') => {
                self.density = match self.density {
                    Density::Auto => Density::Compact,
                    Density::Compact => Density::Roomy,
                    Density::Roomy => Density::Auto,
                }
            }
            KeyCode::Char('o') => {
                self.orient = match self.orient {
                    Orient::Horizontal => Orient::Vertical,
                    Orient::Vertical => Orient::Horizontal,
                };
                self.lane_scroll = 0;
            }
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc => {
                // One layer at a time: an expanded lane collapses before the popup closes.
                let expanded = self.selected_lane().map(|li| self.state.lanes[li].id).filter(|id| self.expanded.contains(id));
                match expanded {
                    Some(id) => {
                        self.expanded.remove(&id);
                    }
                    None if self.popup => self.quit = true,
                    None => {}
                }
            }
            KeyCode::Char('r') => self.refresh_requested = true,
            KeyCode::Char('c') => self.query.show_closed = !self.query.show_closed,
            KeyCode::Enter => self.focus_requested = self.selected_lane().map(|i| self.state.lanes[i].pane_id.clone()),
            KeyCode::Char('f') => {
                self.follow = true;
                self.seq_back = 0;
            }
            KeyCode::Char('0') => {
                self.bucket_auto = true;
                self.follow = true;
                self.seq_back = 0;
            }
            KeyCode::Char('+') | KeyCode::Char('=') if !self.steps() => {
                self.bucket_auto = false;
                self.bucket_idx = self.bucket_idx.saturating_sub(1);
            }
            KeyCode::Char('-') if !self.steps() => {
                self.bucket_auto = false;
                self.bucket_idx = (self.bucket_idx + 1).min(BUCKETS_S.len() - 1);
            }
            _ => {}
        }
    }

    /// Moves the selection `by` rows, clamped to the first and last.
    fn move_selection(&mut self, by: isize) {
        let rows = self.rows();
        if let Some(r) = self.sel_row(&rows) {
            let r = (r as isize).saturating_add(by).clamp(0, rows.len() as isize - 1) as usize;
            self.select_row(&rows, r);
        }
    }

    /// Back or forward a quarter of the strip (`full`: a whole strip); on the steps axis, by steps.
    fn scroll_time(&mut self, back: bool, full: bool) {
        if self.steps() {
            let by = if full { (self.time_cells / sequence::step_cells(self.orient)).max(1) } else { 1 };
            if back {
                self.seq_back += by;
            } else {
                self.seq_back = self.seq_back.saturating_sub(by);
            }
            self.follow = self.seq_back == 0;
            return;
        }
        let cells = if full { self.time_cells } else { (self.time_cells / 4).max(1) };
        let step = cells as i64 * self.bucket_ms();
        if back {
            if self.follow {
                self.view_end_ms = self.now_ms;
            }
            self.view_end_ms -= step;
            self.follow = false;
        } else {
            self.view_end_ms += step;
            if self.view_end_ms >= self.now_ms {
                self.follow = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{key, press, sel, two_lanes};
    use super::*;

    #[test]
    fn letters_mean_the_same_in_both_orientations() {
        let mut app = two_lanes(Orient::Vertical);
        key(&mut app, 'j');
        assert_eq!(sel(&app), Some(1), "j is the next agent in columns too");
        key(&mut app, 'h');
        assert!(!app.follow && app.view_end_ms < app.now_ms, "h goes back in time");
        let back = app.view_end_ms;
        key(&mut app, 'k');
        assert_eq!(sel(&app), Some(0));
        press(&mut app, KeyCode::Right);
        assert_eq!(sel(&app), Some(1), "→ is the next column");
        press(&mut app, KeyCode::Up);
        assert!(app.view_end_ms < back, "↑ goes back in time down the columns");

        let mut app = two_lanes(Orient::Horizontal);
        press(&mut app, KeyCode::Down);
        assert_eq!(sel(&app), Some(1), "↓ is the next row");
        press(&mut app, KeyCode::Down);
        assert_eq!(sel(&app), Some(1), "clamped");
        press(&mut app, KeyCode::Home);
        assert_eq!(sel(&app), Some(0));
    }

    #[test]
    fn v_cycles_lenses_and_t_the_axis() {
        let mut app = two_lanes(Orient::Horizontal);
        key(&mut app, 't');
        assert_eq!(app.axis, Axis::Steps);
        key(&mut app, 'v');
        assert_eq!(app.lens, Lens::Heat);
        key(&mut app, 't');
        assert_eq!(app.axis, Axis::Steps, "t only applies to the status lens");
        press(&mut app, KeyCode::Tab);
        key(&mut app, 'v');
        assert_eq!((app.mode, app.area_mode), (Mode::Usage, AreaMode::Overlaid));
    }

    #[test]
    fn esc_collapses_an_expanded_lane_before_closing() {
        let mut app = two_lanes(Orient::Horizontal);
        app.popup = true;
        key(&mut app, ' ');
        assert_eq!(app.expanded.len(), 1);
        press(&mut app, KeyCode::Esc);
        assert!(app.expanded.is_empty() && !app.quit);
        press(&mut app, KeyCode::Esc);
        assert!(app.quit);
    }

    #[test]
    fn help_overlay_opens_and_closes() {
        use super::super::tests::{render, row, sample_app};
        let mut app = sample_app(1, Orient::Horizontal);
        key(&mut app, '?');
        let buf = render(&mut app, 100, 40);
        let body: String = (1..40).map(|y| row(&buf, y, 0, 100)).collect::<Vec<_>>().join("\n");
        assert!(body.contains(" keys ") && body.contains("quit"), "{body}");
        key(&mut app, 'j');
        assert!(app.help, "other keys do nothing while help is open");
        press(&mut app, KeyCode::Esc);
        assert!(!app.help);
        assert!(!app.quit);
    }
}

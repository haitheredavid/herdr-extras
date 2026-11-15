mod config;
mod follow;
mod herdr;
mod history;
mod model;
mod open;
mod prefs;
mod record;
mod session;
mod theme;
mod view;

use std::collections::{BTreeSet, HashSet};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;
use serde_json::json;

use config::config;
use herdr::{Msg, Subscriber};
use model::{HISTORY_MS, State, due_notifications};
use view::{App, Orient, fmt_dur, preset_from_name};

const SNAPSHOT_EVERY: Duration = Duration::from_secs(5);
const RETRY_EVERY: Duration = Duration::from_secs(2);
const RESUBSCRIBE_AFTER_LOSS: Duration = Duration::from_secs(1);
const REDRAW_EVERY: Duration = Duration::from_secs(1);
const USAGE: &str = "usage: herdr-trail [--popup] [--view time|sequence|area|heat|dash] [--orient h|v] | --dump [--width N --height N --keys \"j j Tab\" ...] | open split|popup | record | follow";

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn self_pane() -> Option<String> {
    std::env::var("HERDR_PANE_ID").ok().filter(|s| !s.is_empty())
}

struct Opts {
    popup: bool,
    dump: bool,
    /// `--view` as preset `1`-`5`; None leaves it to `config.toml` and the remembered settings.
    view: Option<u8>,
    orient: Option<Orient>,
    width: u16,
    height: u16,
    /// `--dump` presses these before drawing.
    keys: Vec<KeyEvent>,
}

/// Keys from `--keys`: space-separated single characters, key names, or `Space`.
fn parse_keys(s: &str) -> anyhow::Result<Vec<KeyEvent>> {
    s.split_whitespace()
        .map(|t| {
            let code = match t {
                "Tab" => KeyCode::Tab,
                "BackTab" => KeyCode::BackTab,
                "Esc" => KeyCode::Esc,
                "Enter" => KeyCode::Enter,
                "Backspace" => KeyCode::Backspace,
                "Up" => KeyCode::Up,
                "Down" => KeyCode::Down,
                "Left" => KeyCode::Left,
                "Right" => KeyCode::Right,
                "Home" => KeyCode::Home,
                "End" => KeyCode::End,
                "PgUp" => KeyCode::PageUp,
                "PgDn" => KeyCode::PageDown,
                "Space" => KeyCode::Char(' '),
                _ => {
                    let mut chars = t.chars();
                    match (chars.next(), chars.next()) {
                        (Some(c), None) => KeyCode::Char(c),
                        _ => bail!("unknown key {t:?}"),
                    }
                }
            };
            Ok(KeyEvent::new(code, KeyModifiers::NONE))
        })
        .collect()
}

fn parse_orient(v: &str) -> anyhow::Result<Orient> {
    match v {
        "h" => Ok(Orient::Horizontal),
        "v" => Ok(Orient::Vertical),
        v => bail!("unknown orientation {v:?}"),
    }
}

fn parse_opts(args: &[String]) -> anyhow::Result<Opts> {
    let mut o = Opts { popup: false, dump: false, view: None, orient: None, width: 120, height: 30, keys: Vec::new() };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--popup" => o.popup = true,
            "--dump" => o.dump = true,
            "--view" => {
                let v = value()?;
                o.view = Some(preset_from_name(v).with_context(|| format!("unknown view {v:?}"))?);
            }
            "--orient" => o.orient = Some(parse_orient(value()?)?),
            "--width" => o.width = value()?.parse().context("--width")?,
            "--height" => o.height = value()?.parse().context("--height")?,
            "--keys" => o.keys = parse_keys(value()?)?,
            other => bail!("unknown argument {other:?}"),
        }
    }
    if !o.dump && args.iter().any(|a| a == "--width" || a == "--height" || a == "--keys") {
        bail!("--width/--height/--keys only apply to --dump");
    }
    Ok(o)
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some(cmd @ ("open" | "record" | "follow")) => {
            let result = match cmd {
                "open" => open::run(&args[1..]),
                "record" => record::run(),
                _ => follow::run(),
            };
            if let Err(e) = result {
                eprintln!("herdr-trail {cmd}: {e:#}");
                std::process::exit(1);
            }
            Ok(())
        }
        _ => match parse_opts(&args) {
            Ok(o) if o.dump => {
                dump(&o);
                Ok(())
            }
            Ok(o) => run_tui(&o),
            Err(e) => {
                eprintln!("herdr-trail: {e:#}\n{USAGE}");
                std::process::exit(2);
            }
        },
    }
}

fn refresh(app: &mut App) -> bool {
    match herdr::snapshot() {
        Ok(snap) => {
            app.state.apply_snapshot(&snap, now_ms());
            app.refresh_usage();
            app.error = None;
            app.connected = true;
            true
        }
        Err(e) => {
            app.error = Some(format!("{e:#}"));
            app.connected = false;
            false
        }
    }
}

/// App with the logged history folded in, then the live snapshot on top. Settings stack from the
/// defaults through `config.toml [start]` and the remembered ones (not for `--dump`) to the flags.
fn start(o: &Opts) -> (App, bool) {
    let now = now_ms();
    let mut state = State::new(self_pane());
    for e in history::load_since(&history::log_dir(), &history::session_name(), now, now - HISTORY_MS) {
        if let Some(h) = e.host_event() {
            state.apply(&h, e.ts_ms, false);
        }
    }
    let cfg = config();
    let preset = cfg.start_view.as_deref().and_then(preset_from_name).unwrap_or(1);
    let orient = cfg.start_orient.as_deref().and_then(|v| parse_orient(v).ok()).unwrap_or(Orient::Horizontal);
    let mut app = App::new(state, preset, orient, now);
    if !o.dump && cfg.remember {
        prefs::load(&prefs::prefs_path(), &mut app);
    }
    if let Some(v) = o.view {
        app.preset(v);
    }
    if let Some(orient) = o.orient {
        app.orient = orient;
    }
    app.popup = o.popup;
    let connected = refresh(&mut app);
    (app, connected)
}

fn dump(o: &Opts) {
    let (mut app, _) = start(o);
    let area = Rect::new(0, 0, o.width, o.height);
    // A first draw sets the geometry keys depend on (time cells, scroll, layout).
    view::draw(&mut Buffer::empty(area), area, &mut app);
    for &k in &o.keys {
        app.on_key(k);
    }
    let mut buf = Buffer::empty(area);
    view::draw(&mut buf, area, &mut app);
    for y in 0..area.height {
        let line: String = (0..area.width).map(|x| buf[(x, y)].symbol()).collect();
        println!("{}", line.trim_end());
    }
}

/// Sends a herdr notification for each agent blocked past `[notify] blocked_after`, once per
/// episode; a failure shows in the header.
fn notify_blocked(app: &mut App, sent: &mut HashSet<(u64, usize)>) {
    let Some(after) = config().blocked_after_ms else { return };
    let now = now_ms();
    for key in due_notifications(&app.state, now, after, sent) {
        sent.insert(key);
        let Some(li) = app.state.lane_by_id(key.0) else { continue };
        let lane = &app.state.lanes[li];
        let params = json!({
            "title": format!("{} blocked {}", lane.label(), fmt_dur(now - lane.last_ms())),
            "body": format!("{} · {}", app.state.workspace_label(li), lane.agent.as_deref().unwrap_or("-")),
            "sound": "request",
        });
        if let Err(e) = herdr::request("notification.show", params) {
            app.error = Some(format!("notification.show: {e:#}"));
        }
    }
}

fn run_tui(o: &Opts) -> anyhow::Result<()> {
    let (mut app, connected) = start(o);
    let (tx, rx) = mpsc::channel::<Msg>();
    let mut generation = 0u64;
    let mut sub: Option<Subscriber> = None;
    let mut sub_ids: BTreeSet<String> = BTreeSet::new();
    let mut resubscribe_at: Option<Instant> = None;
    let mut notified: HashSet<(u64, usize)> = HashSet::new();

    let mut next_snapshot = Instant::now() + if connected { SNAPSHOT_EVERY } else { RETRY_EVERY };
    let mut terminal = ratatui::init();
    let mouse = config().mouse;
    if mouse {
        execute!(std::io::stdout(), EnableMouseCapture)?;
    }
    let result = (|| -> anyhow::Result<()> {
        // ratatui writes a style reset and cursor hide on every draw, even an unchanged one, and
        // each write makes herdr recomposite the pane; so draw only on change or once a second
        // for the clock.
        let mut dirty = true;
        let mut next_draw = Instant::now();
        loop {
            if dirty || Instant::now() >= next_draw {
                app.now_ms = now_ms();
                terminal.draw(|f| {
                    let area = f.area();
                    view::draw(f.buffer_mut(), area, &mut app)
                })?;
                dirty = false;
                next_draw = Instant::now() + REDRAW_EVERY;
            }

            let wait = next_draw.saturating_duration_since(Instant::now()).min(Duration::from_millis(100));
            if event::poll(wait)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                            return Ok(());
                        }
                        app.on_key(key);
                        dirty = true;
                    }
                    Event::Mouse(m) => {
                        app.on_mouse(m);
                        dirty = true;
                    }
                    _ => {}
                }
            }
            if let Some(pane_id) = app.focus_requested.take() {
                match herdr::request("pane.focus", json!({ "pane_id": pane_id })) {
                    Ok(_) if app.popup => return Ok(()),
                    Ok(_) => {}
                    Err(e) => {
                        app.error = Some(format!("focus {pane_id}: {e:#}"));
                        dirty = true;
                    }
                }
            }
            if app.quit {
                return Ok(());
            }
            notify_blocked(&mut app, &mut notified);

            while let Ok(msg) = rx.try_recv() {
                match msg {
                    Msg::SubStarted(g) if g == generation => {
                        refresh(&mut app);
                        dirty = true;
                    }
                    Msg::Event(g, env) if g == generation => {
                        if let Some(h) = herdr::host_event(&env) {
                            app.state.apply(&h, now_ms(), true);
                        }
                        app.refresh_usage();
                        dirty = true;
                    }
                    Msg::Lost(g) if g == generation => {
                        resubscribe_at = Some(Instant::now() + RESUBSCRIBE_AFTER_LOSS);
                    }
                    _ => {}
                }
            }

            let now = Instant::now();
            if app.refresh_requested || now >= next_snapshot {
                app.refresh_requested = false;
                let ok = refresh(&mut app);
                next_snapshot = now + if ok { SNAPSHOT_EVERY } else { RETRY_EVERY };
                dirty = true;
            }

            let ids = app.state.open_live_ids();
            let due = resubscribe_at.is_some_and(|t| now >= t);
            let need = due
                || (sub.is_none() && resubscribe_at.is_none() && app.error.is_none())
                || (sub.is_some() && resubscribe_at.is_none() && ids != sub_ids);
            if need {
                if let Some(s) = sub.take() {
                    s.stop();
                }
                generation += 1;
                let id_list: Vec<String> = ids.iter().cloned().collect();
                match herdr::subscribe(generation, &id_list, tx.clone()) {
                    Ok(s) => {
                        debug_assert_eq!(s.generation, generation);
                        sub = Some(s);
                        sub_ids = ids;
                        resubscribe_at = None;
                    }
                    Err(e) => {
                        app.error = Some(format!("{e:#}"));
                        resubscribe_at = Some(now + RETRY_EVERY);
                    }
                }
            }
        }
    })();
    if let Some(s) = sub {
        s.stop();
    }
    if mouse {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
    }
    ratatui::restore();
    if config().remember {
        let _ = prefs::save(&prefs::prefs_path(), &app);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split(' ').map(String::from).collect()
    }

    #[test]
    fn parse_opts_reads_keys() {
        let mut a = args("--dump");
        a.extend(["--keys".to_string(), "j Tab ? Space".to_string()]);
        let o = parse_opts(&a).unwrap();
        let codes: Vec<KeyCode> = o.keys.iter().map(|k| k.code).collect();
        assert_eq!(codes, [KeyCode::Char('j'), KeyCode::Tab, KeyCode::Char('?'), KeyCode::Char(' ')]);
        assert!(parse_opts(&args("--dump --keys Nope")).is_err());
        assert!(parse_opts(&args("--keys j")).is_err(), "--keys needs --dump");
    }
}

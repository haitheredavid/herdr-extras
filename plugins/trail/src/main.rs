mod config;
mod follow;
mod herdr;
mod history;
mod model;
mod open;
mod record;
mod theme;
mod usage;
mod view;

use std::collections::BTreeSet;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use serde_json::json;

use herdr::{Msg, Subscriber};
use model::{HISTORY_MS, State};
use view::{App, Orient, ViewKind};

const SNAPSHOT_EVERY: Duration = Duration::from_secs(5);
const RETRY_EVERY: Duration = Duration::from_secs(2);
const RESUBSCRIBE_AFTER_LOSS: Duration = Duration::from_secs(1);
const REDRAW_EVERY: Duration = Duration::from_secs(1);
const USAGE: &str = "usage: herdr-trail [--popup] [--view time|sequence|area|heat|dash] [--orient h|v] | --dump [--width N --height N ...] | open split|popup | record | follow";

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn self_pane() -> Option<String> {
    std::env::var("HERDR_PANE_ID").ok().filter(|s| !s.is_empty())
}

struct Opts {
    popup: bool,
    dump: bool,
    view: ViewKind,
    orient: Orient,
    width: u16,
    height: u16,
}

fn parse_orient(v: &str) -> anyhow::Result<Orient> {
    match v {
        "h" => Ok(Orient::Horizontal),
        "v" => Ok(Orient::Vertical),
        v => bail!("unknown orientation {v:?}"),
    }
}

fn parse_opts(args: &[String]) -> anyhow::Result<Opts> {
    let cfg = config::config();
    let view = cfg.start_view.as_deref().and_then(ViewKind::from_name).unwrap_or(ViewKind::Time);
    let orient = cfg.start_orient.as_deref().and_then(|o| parse_orient(o).ok()).unwrap_or(Orient::Horizontal);
    let mut o = Opts { popup: false, dump: false, view, orient, width: 120, height: 30 };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--popup" => o.popup = true,
            "--dump" => o.dump = true,
            "--view" => {
                let v = value()?;
                o.view = ViewKind::from_name(v).with_context(|| format!("unknown view {v:?}"))?;
            }
            "--orient" => o.orient = parse_orient(value()?)?,
            "--width" => o.width = value()?.parse().context("--width")?,
            "--height" => o.height = value()?.parse().context("--height")?,
            other => bail!("unknown argument {other:?}"),
        }
    }
    if !o.dump && args.iter().any(|a| a == "--width" || a == "--height") {
        bail!("--width/--height only apply to --dump");
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

/// App with the logged history folded in, then the live snapshot on top.
fn start(o: &Opts) -> (App, bool) {
    let now = now_ms();
    let mut state = State::new(self_pane());
    for e in history::load_since(&history::log_dir(), &history::session_name(), now, now - HISTORY_MS) {
        if let Some(h) = e.host_event() {
            state.apply(&h, e.ts_ms, false);
        }
    }
    let mut app = App::new(state, o.view, o.orient, now);
    app.popup = o.popup;
    let connected = refresh(&mut app);
    (app, connected)
}

fn dump(o: &Opts) {
    let (mut app, _) = start(o);
    let area = Rect::new(0, 0, o.width, o.height);
    let mut buf = Buffer::empty(area);
    view::draw(&mut buf, area, &mut app);
    for y in 0..area.height {
        let line: String = (0..area.width).map(|x| buf[(x, y)].symbol()).collect();
        println!("{}", line.trim_end());
    }
}

fn run_tui(o: &Opts) -> anyhow::Result<()> {
    let (mut app, connected) = start(o);
    let (tx, rx) = mpsc::channel::<Msg>();
    let mut generation = 0u64;
    let mut sub: Option<Subscriber> = None;
    let mut sub_ids: BTreeSet<String> = BTreeSet::new();
    let mut resubscribe_at: Option<Instant> = None;

    let mut next_snapshot = Instant::now() + if connected { SNAPSHOT_EVERY } else { RETRY_EVERY };
    let mut terminal = ratatui::init();
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
            if event::poll(wait)?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    return Ok(());
                }
                app.on_key(key);
                dirty = true;
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
    ratatui::restore();
    result
}

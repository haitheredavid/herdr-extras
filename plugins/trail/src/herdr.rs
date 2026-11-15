//! Herdr socket client: one-shot requests and a long-lived event subscriber.

use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use anyhow::{Context, anyhow};
use serde_json::{Value, json};

use crate::model::Snapshot;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
const IO_TIMEOUT: Duration = Duration::from_secs(2);
pub const PLUGIN_ID: &str = "herdr-trail";

/// Plugin id herdr launched us under, else the manifest id.
pub fn plugin_id() -> String {
    std::env::var("HERDR_PLUGIN_ID").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| PLUGIN_ID.into())
}

pub fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("HERDR_SOCKET_PATH") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/herdr/herdr.sock")
}

fn connect() -> anyhow::Result<UnixStream> {
    let path = socket_path();
    UnixStream::connect(&path).with_context(|| format!("connect {}", path.display()))
}

fn request_line(method: &str, params: Value) -> String {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut line = json!({ "id": format!("trail-{id}"), "method": method, "params": params }).to_string();
    line.push('\n');
    line
}

fn error_of(v: &Value) -> Option<(String, String)> {
    let e = v.get("error")?;
    let code = e.get("code").and_then(Value::as_str).unwrap_or("error").to_string();
    let msg = e.get("message").and_then(Value::as_str).unwrap_or("").to_string();
    Some((code, msg))
}

pub fn request(method: &str, params: Value) -> anyhow::Result<Value> {
    let mut stream = connect()?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.write_all(request_line(method, params).as_bytes())?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    if line.is_empty() {
        return Err(anyhow!("{method}: connection closed without response"));
    }
    let mut v: Value = serde_json::from_str(&line).context("parse response")?;
    if let Some((code, msg)) = error_of(&v) {
        return Err(anyhow!("{code}: {msg}"));
    }
    Ok(v.get_mut("result").map(Value::take).unwrap_or(Value::Null))
}

pub fn snapshot() -> anyhow::Result<Snapshot> {
    let mut result = request("session.snapshot", json!({}))?;
    let snap = result.get_mut("snapshot").map(Value::take).ok_or_else(|| anyhow!("snapshot missing"))?;
    Ok(serde_json::from_value(snap)?)
}

#[derive(Debug, Clone)]
pub struct Envelope {
    pub event: String,
    pub data: Value,
}

#[derive(Debug)]
pub enum Msg {
    SubStarted(u64),
    Event(u64, Envelope),
    Lost(u64),
}

pub struct Subscriber {
    pub generation: u64,
    stream: UnixStream,
}

impl Subscriber {
    pub fn stop(&self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

pub fn subscribe(generation: u64, pane_ids: &[String], tx: Sender<Msg>) -> anyhow::Result<Subscriber> {
    let mut subs: Vec<Value> =
        ["pane.created", "pane.closed", "pane.updated", "pane.exited", "pane.moved", "pane.agent_detected"]
            .iter()
            .map(|t| json!({ "type": t }))
            .collect();
    subs.extend(pane_ids.iter().map(|id| json!({ "type": "pane.agent_status_changed", "pane_id": id })));

    let mut stream = connect()?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.write_all(request_line("events.subscribe", json!({ "subscriptions": subs })).as_bytes())?;
    let reader = stream.try_clone()?;

    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let Ok(mut v) = serde_json::from_str::<Value>(&line) else { continue };
            if error_of(&v).is_some() {
                break;
            }
            if let Some(kind) = v.get("event").and_then(Value::as_str) {
                let event = kind.replace('.', "_");
                let data = v.get_mut("data").map(Value::take).unwrap_or(Value::Null);
                if tx.send(Msg::Event(generation, Envelope { event, data })).is_err() {
                    return;
                }
            } else if v.pointer("/result/type").and_then(Value::as_str) == Some("subscription_started")
                && tx.send(Msg::SubStarted(generation)).is_err()
            {
                return;
            }
        }
        let _ = tx.send(Msg::Lost(generation));
    });

    Ok(Subscriber { generation, stream })
}

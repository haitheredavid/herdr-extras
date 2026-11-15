//! Herdr socket client: one-shot requests, a long-lived event subscriber, and the wire types of
//! snapshots and events, mapped onto the model's `HostEvent`.

use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use anyhow::{Context, anyhow};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::model::{AgentStatus, HostEvent, PaneMeta};
use crate::session::{RefKind, SessionRef};

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Snapshot {
    #[serde(default)]
    pub panes: Vec<PaneInfo>,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceInfo>,
    #[serde(default)]
    pub tabs: Vec<TabInfo>,
    #[serde(default)]
    pub focused_workspace_id: Option<String>,
    #[serde(default)]
    pub focused_tab_id: Option<String>,
    #[serde(default)]
    pub protocol: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PaneInfo {
    pub pane_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub tab_id: Option<String>,
    /// Stable across pane moves, unlike `pane_id`.
    #[serde(default)]
    pub terminal_id: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// Latest OSC terminal title; agents like omp put their current task here.
    #[serde(default, rename = "terminal_title_stripped")]
    pub terminal_title: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub agent_status: AgentStatus,
    #[serde(default)]
    pub agent_session: Option<AgentSession>,
}

/// Agent session identity herdr reports, e.g. `{"agent":"omp","kind":"path","source":"herdr:omp","value":"….jsonl"}`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AgentSession {
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub value: String,
}

impl AgentSession {
    /// The ref, when `kind` is one trail knows and `value` is set; `agent` falls back to the pane's.
    fn session_ref(&self, pane_agent: Option<&str>) -> Option<SessionRef> {
        let kind = match self.kind.as_str() {
            "id" => RefKind::Id,
            "path" => RefKind::Path,
            _ => return None,
        };
        let agent = if self.agent.is_empty() { pane_agent.unwrap_or_default().to_string() } else { self.agent.clone() };
        (!self.value.is_empty()).then(|| SessionRef { agent, kind, value: self.value.clone() })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceInfo {
    pub workspace_id: String,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TabInfo {
    pub tab_id: String,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Deserialize)]
struct PanePayload {
    pane: PaneInfo,
}

#[derive(Deserialize)]
struct PaneRef {
    pane_id: String,
}

#[derive(Deserialize)]
struct DetectedPayload {
    pane_id: String,
    #[serde(default)]
    workspace_id: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    released: bool,
}

#[derive(Deserialize)]
struct StatusPayload {
    pane_id: String,
    #[serde(default)]
    workspace_id: String,
    agent_status: AgentStatus,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize)]
struct MovedPayload {
    previous_pane_id: String,
    pane: PaneInfo,
}

impl From<&PaneInfo> for PaneMeta {
    fn from(p: &PaneInfo) -> PaneMeta {
        PaneMeta {
            pane_id: p.pane_id.clone(),
            workspace_id: p.workspace_id.clone(),
            tab_id: p.tab_id.clone(),
            terminal_id: p.terminal_id.clone(),
            agent: p.agent.clone(),
            title: p.title.clone(),
            terminal_title: p.terminal_title.clone(),
            cwd: p.cwd.clone(),
            status: Some(p.agent_status),
            session: p.agent_session.as_ref().and_then(|s| s.session_ref(p.agent.as_deref())),
            ..PaneMeta::default()
        }
    }
}

/// The model event for a live envelope; None for events it doesn't track or payloads that don't parse.
pub fn host_event(env: &Envelope) -> Option<HostEvent> {
    let data = env.data.clone();
    let meta = |pane_id: String, workspace_id: String| PaneMeta { pane_id, workspace_id, ..PaneMeta::default() };
    Some(match env.event.replace('.', "_").as_str() {
        "pane_created" => HostEvent::Created((&serde_json::from_value::<PanePayload>(data).ok()?.pane).into()),
        "pane_updated" => HostEvent::Updated((&serde_json::from_value::<PanePayload>(data).ok()?.pane).into()),
        "pane_closed" => HostEvent::Closed { pane_id: serde_json::from_value::<PaneRef>(data).ok()?.pane_id },
        "pane_exited" => HostEvent::Exited { pane_id: serde_json::from_value::<PaneRef>(data).ok()?.pane_id },
        "pane_agent_detected" => {
            let p = serde_json::from_value::<DetectedPayload>(data).ok()?;
            HostEvent::Detected { meta: PaneMeta { agent: p.agent, ..meta(p.pane_id, p.workspace_id) }, released: p.released }
        }
        "pane_agent_status_changed" => {
            let p = serde_json::from_value::<StatusPayload>(data).ok()?;
            HostEvent::Status(PaneMeta {
                agent: p.agent,
                title: p.title,
                status: Some(p.agent_status),
                ..meta(p.pane_id, p.workspace_id)
            })
        }
        "pane_moved" => {
            let p = serde_json::from_value::<MovedPayload>(data).ok()?;
            HostEvent::Moved { previous_pane_id: p.previous_pane_id, pane: (&p.pane).into() }
        }
        _ => return None,
    })
}

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

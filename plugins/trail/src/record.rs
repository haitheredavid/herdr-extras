//! `herdr-trail record`: the herdr `[[events]]` hook. Appends one JSON line per pane event to the
//! monthly history log so the trail keeps its nodes, sessions and pane moves across restarts.

use std::fs::OpenOptions;
use std::io::Write;

use anyhow::Context;
use serde_json::{Value, json};

use crate::history::{LogEvent, log_dir, log_file, session_name};
use crate::model::AgentStatus;
use crate::session::SessionRef;

pub fn run() -> anyhow::Result<()> {
    let payload = env_json("HERDR_PLUGIN_EVENT_JSON");
    let context = env_json("HERDR_PLUGIN_CONTEXT_JSON");
    let event = std::env::var("HERDR_PLUGIN_EVENT")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| payload.get("event").and_then(Value::as_str).map(dotted))
        .unwrap_or_default();
    let ts_ms = chrono::Utc::now().timestamp_millis();
    let Some(mut record) = event_from(&event, &payload, &context, &session_name(), ts_ms) else {
        eprintln!("herdr-trail record: {event:?} payload has no pane_id; skipped");
        return Ok(());
    };
    // Status payloads carry no pane record; ask herdr for the session so a history lane can find
    // its usage without a live snapshot.
    if record.agent_session.is_none()
        && matches!(event.as_str(), "pane.agent_status_changed" | "pane.agent_detected")
        && let Ok(result) = crate::herdr::request("pane.get", json!({ "pane_id": record.pane_id }))
    {
        let pane = result.get("pane").unwrap_or(&Value::Null);
        record.agent_session = session_of(pane, record.agent.as_deref());
        record.terminal_id = record.terminal_id.or_else(|| str_at(pane, "terminal_id"));
        record.tab_id = record.tab_id.or_else(|| str_at(pane, "tab_id"));
    }
    let dir = log_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = log_file(&dir, ts_ms);
    let mut line = serde_json::to_string(&record)?;
    line.push('\n');
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()))
        .with_context(|| format!("append {}", path.display()))
}

fn env_json(key: &str) -> Value {
    std::env::var(key).ok().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or(Value::Null)
}

/// `pane_agent_status_changed` → `pane.agent_status_changed`.
fn dotted(event: &str) -> String {
    event.strip_prefix("pane_").map_or_else(|| event.to_string(), |rest| format!("pane.{rest}"))
}

fn str_at(v: &Value, key: &str) -> Option<String> {
    v.get(key)?.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

/// `pane.agent_session`, with its agent falling back to the pane's.
fn session_of(pane: &Value, agent: Option<&str>) -> Option<SessionRef> {
    let mut r = serde_json::from_value::<SessionRef>(pane.get("agent_session")?.clone()).ok().filter(|r| !r.value.is_empty())?;
    if r.agent.is_empty() {
        r.agent = agent.unwrap_or_default().to_string();
    }
    Some(r)
}

/// Log record for a hook payload; None when it names no pane. Context labels are only taken when
/// the context is about the event's workspace (and pane or tab), so events from background
/// workspaces don't pick up the focused one's labels.
fn event_from(event: &str, payload: &Value, context: &Value, session: &str, ts_ms: i64) -> Option<LogEvent> {
    let data = payload.get("data").unwrap_or(&Value::Null);
    let pane = data.get("pane").unwrap_or(&Value::Null);
    let field = |key: &str| str_at(data, key).or_else(|| str_at(pane, key));

    let pane_id = field("pane_id")?;
    let workspace_id = field("workspace_id").unwrap_or_default();
    let ctx_workspace = str_at(context, "workspace_id");
    let focused = str_at(context, "focused_pane_id").as_deref() == Some(pane_id.as_str());
    let same_workspace = workspace_id.is_empty() || ctx_workspace.as_deref().is_none_or(|w| w == workspace_id);
    let tab_id = field("tab_id");
    let same_tab = match (&tab_id, str_at(context, "tab_id")) {
        (Some(pane_tab), Some(ctx_tab)) => *pane_tab == ctx_tab,
        _ => focused,
    };

    let cwd = str_at(pane, "foreground_cwd")
        .or_else(|| str_at(pane, "cwd"))
        .or_else(|| focused.then(|| str_at(context, "focused_pane_cwd")).flatten())
        .or_else(|| same_workspace.then(|| str_at(context, "workspace_cwd")).flatten());
    let status = field("agent_status").and_then(|s| serde_json::from_value::<AgentStatus>(Value::String(s)).ok());
    let agent = field("agent");

    Some(LogEvent {
        ts_ms,
        session: session.to_string(),
        event: event.to_string(),
        workspace_label: same_workspace.then(|| str_at(context, "workspace_label")).flatten(),
        tab_label: (same_workspace && same_tab).then(|| str_at(context, "tab_label")).flatten(),
        cwd,
        agent_session: session_of(pane, agent.as_deref()),
        agent,
        status,
        tab_id,
        terminal_id: field("terminal_id"),
        previous_pane_id: str_at(data, "previous_pane_id"),
        pane_id,
        workspace_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::RefKind;
    use serde_json::json;

    fn context() -> Value {
        json!({
            "workspace_id": "w1", "workspace_label": "ctx-herdr", "workspace_cwd": "/ws",
            "tab_id": "w1:t1", "tab_label": "1", "focused_pane_id": "w1:p2", "focused_pane_cwd": "/focused",
        })
    }

    #[test]
    fn pane_created_reads_nested_pane() {
        let payload = json!({"event": "pane_created", "data": {"pane": {
            "pane_id": "w1:p3", "workspace_id": "w1", "tab_id": "w1:t1", "terminal_id": "term_1", "cwd": "/repo",
            "foreground_cwd": "/repo/sub", "agent_status": "working", "agent": "omp",
            "agent_session": {"agent": "omp", "kind": "path", "source": "herdr:omp", "value": "/s/a.jsonl"},
        }}});
        let e = event_from("pane.created", &payload, &context(), "main", 42).unwrap();
        assert_eq!(
            e,
            LogEvent {
                ts_ms: 42,
                session: "main".into(),
                event: "pane.created".into(),
                pane_id: "w1:p3".into(),
                workspace_id: "w1".into(),
                workspace_label: Some("ctx-herdr".into()),
                tab_label: Some("1".into()),
                cwd: Some("/repo/sub".into()),
                agent: Some("omp".into()),
                status: Some(AgentStatus::Working),
                tab_id: Some("w1:t1".into()),
                terminal_id: Some("term_1".into()),
                agent_session: Some(SessionRef { agent: "omp".into(), kind: RefKind::Path, value: "/s/a.jsonl".into() }),
                previous_pane_id: None,
            }
        );
    }

    #[test]
    fn moved_keeps_the_previous_pane_id() {
        let payload = json!({"data": {"previous_pane_id": "w1:p3", "pane": {"pane_id": "w2:p1", "workspace_id": "w2", "terminal_id": "term_1"}}});
        let e = event_from("pane.moved", &payload, &Value::Null, "default", 9).unwrap();
        assert_eq!((e.pane_id.as_str(), e.previous_pane_id.as_deref()), ("w2:p1", Some("w1:p3")));
        assert_eq!(e.terminal_id.as_deref(), Some("term_1"));
    }

    #[test]
    fn session_agent_falls_back_to_the_panes() {
        let pane = json!({"agent_session": {"kind": "id", "value": "abc"}});
        let r = session_of(&pane, Some("claude")).unwrap();
        assert_eq!((r.agent.as_str(), r.kind), ("claude", RefKind::Id));
        assert_eq!(session_of(&json!({"agent_session": {"kind": "weird", "value": "x"}}), None), None);
    }

    #[test]
    fn pane_created_cwd_falls_back_to_pane_cwd() {
        let payload = json!({"data": {"pane": {"pane_id": "w1:p3", "workspace_id": "w1", "cwd": "/repo", "foreground_cwd": ""}}});
        let e = event_from("pane.created", &payload, &context(), "main", 1).unwrap();
        assert_eq!(e.cwd.as_deref(), Some("/repo"));
    }

    #[test]
    fn status_changed_uses_focused_cwd_for_focused_pane() {
        let payload = json!({"event": "pane_agent_status_changed", "data": {
            "type": "pane_agent_status_changed", "pane_id": "w1:p2", "workspace_id": "w1",
            "agent_status": "blocked", "agent": "omp",
        }});
        let e = event_from("pane.agent_status_changed", &payload, &context(), "default", 7).unwrap();
        assert_eq!(e.pane_id, "w1:p2");
        assert_eq!(e.status, Some(AgentStatus::Blocked));
        assert_eq!(e.agent.as_deref(), Some("omp"));
        assert_eq!(e.cwd.as_deref(), Some("/focused"));
        assert_eq!(e.tab_label.as_deref(), Some("1"));
    }

    #[test]
    fn status_changed_other_pane_falls_back_to_workspace_cwd() {
        let payload = json!({"data": {"pane_id": "w1:p9", "workspace_id": "w1", "agent_status": "idle"}});
        let e = event_from("pane.agent_status_changed", &payload, &context(), "default", 7).unwrap();
        assert_eq!(e.cwd.as_deref(), Some("/ws"));
        assert_eq!(e.status, Some(AgentStatus::Idle));
        assert_eq!(e.workspace_label.as_deref(), Some("ctx-herdr"));
        assert_eq!(e.tab_label, None, "tab unknown for an unfocused pane");
    }

    #[test]
    fn other_workspace_ignores_context() {
        let payload = json!({"data": {"pane_id": "w2:p1", "workspace_id": "w2", "agent_status": "working"}});
        let e = event_from("pane.agent_status_changed", &payload, &context(), "default", 7).unwrap();
        assert_eq!(e.cwd, None);
        assert_eq!(e.workspace_label, None);
        assert_eq!(e.tab_label, None);
    }

    #[test]
    fn agent_detected_and_closed_shapes() {
        let detected = json!({"data": {"pane_id": "w1:p4", "workspace_id": "w1", "agent": "claude"}});
        let e = event_from("pane.agent_detected", &detected, &Value::Null, "default", 3).unwrap();
        assert_eq!(e.agent.as_deref(), Some("claude"));
        assert_eq!(e.status, None);
        assert_eq!(e.cwd, None);

        let closed = json!({"data": {"pane_id": "w1:p4", "workspace_id": "w1"}});
        let e = event_from("pane.closed", &closed, &context(), "default", 4).unwrap();
        assert_eq!((e.event.as_str(), e.pane_id.as_str(), e.workspace_id.as_str()), ("pane.closed", "w1:p4", "w1"));
        assert_eq!(e.agent, None);
        assert_eq!(e.status, None);
        let line = serde_json::to_string(&e).unwrap();
        assert!(!line.contains("agent") && !line.contains("status"), "{line}");
    }

    #[test]
    fn missing_pane_id_is_none() {
        assert_eq!(event_from("pane.closed", &json!({"data": {"workspace_id": "w1"}}), &context(), "d", 1), None);
        assert_eq!(event_from("pane.created", &Value::Null, &Value::Null, "d", 1), None);
    }

    #[test]
    fn snake_event_names_become_dotted() {
        assert_eq!(dotted("pane_agent_status_changed"), "pane.agent_status_changed");
        assert_eq!(dotted("pane.closed"), "pane.closed");
    }
}

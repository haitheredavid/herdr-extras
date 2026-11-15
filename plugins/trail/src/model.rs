//! The trail state: one lane per pane lifetime, one step per state change, folded from the history
//! log, snapshots and live events through one `HostEvent` fold.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::herdr::Snapshot;

/// Log window loaded at start; closed lanes older than this are pruned.
pub const HISTORY_MS: i64 = 2 * 3600 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    #[default]
    #[serde(other)]
    Unknown,
}

impl AgentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentStatus::Idle => "idle",
            AgentStatus::Working => "working",
            AgentStatus::Blocked => "blocked",
            AgentStatus::Done => "done",
            AgentStatus::Unknown => "unknown",
        }
    }
}

/// Task text from a terminal title: drops leading single-glyph tokens such as omp's `π` and
/// braille spinner frames, so `π ⠇ issues — plan` reads `issues — plan`.
pub fn task_text(title: &str) -> Option<&str> {
    let mut rest = title.trim_start();
    while let Some((head, tail)) = rest.split_once(char::is_whitespace) {
        if head.chars().count() != 1 {
            break;
        }
        rest = tail.trim_start();
    }
    let rest = rest.trim_end();
    (!rest.is_empty() && rest.chars().count() > 1).then_some(rest)
}

/// One pane fact from any source (live event, snapshot, history log), folded by `State::apply`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PaneMeta {
    pub pane_id: String,
    pub workspace_id: String,
    pub workspace_label: Option<String>,
    pub tab_id: Option<String>,
    pub tab_label: Option<String>,
    pub terminal_id: Option<String>,
    pub agent: Option<String>,
    pub title: Option<String>,
    pub terminal_title: Option<String>,
    pub cwd: Option<String>,
    pub status: Option<AgentStatus>,
    /// Agent session file herdr reports for the pane (`kind == "path"` only).
    pub session: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HostEvent {
    Created(PaneMeta),
    /// A full pane record (snapshot or `pane.updated`): its title replaces the lane's, even when empty.
    Updated(PaneMeta),
    Detected {
        meta: PaneMeta,
        released: bool,
    },
    /// `meta.status` is always `Some`.
    Status(PaneMeta),
    Moved {
        previous_pane_id: String,
        pane: PaneMeta,
    },
    Closed {
        pane_id: String,
    },
    Exited {
        pane_id: String,
    },
}

impl HostEvent {
    fn meta(&self) -> Option<&PaneMeta> {
        match self {
            HostEvent::Created(m) | HostEvent::Updated(m) | HostEvent::Status(m) => Some(m),
            HostEvent::Detected { meta, .. } => Some(meta),
            HostEvent::Moved { pane, .. } => Some(pane),
            HostEvent::Closed { .. } | HostEvent::Exited { .. } => None,
        }
    }

    fn pane_id(&self) -> &str {
        match self {
            HostEvent::Closed { pane_id } | HostEvent::Exited { pane_id } => pane_id,
            _ => self.meta().map_or("", |m| m.pane_id.as_str()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// First met already running (snapshot, or a status event with no prior lane).
    Seen,
    Created,
    Status(AgentStatus),
    Closed,
    Exited,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub at_ms: i64,
    pub mark: Mark,
    /// Status in effect after this step.
    pub status: AgentStatus,
}

/// One pane lifetime. A pane id reused after close starts a new lane.
#[derive(Debug, Clone)]
pub struct Lane {
    /// Unique for this run; selection and expansion follow it.
    pub id: u64,
    /// Agent palette slot, assigned once in first-seen order.
    pub slot: Option<usize>,
    pub pane_id: String,
    pub workspace_id: String,
    /// Latest non-empty label from the log.
    pub workspace_label: Option<String>,
    pub tab_id: Option<String>,
    /// Latest non-empty tab label from the log.
    pub tab_label: Option<String>,
    /// Stable across pane moves; a moved pane rejoins its lane by it.
    pub terminal_id: Option<String>,
    /// Latest non-empty agent name; never cleared (release is ignored).
    pub agent: Option<String>,
    pub title: Option<String>,
    pub terminal_title: Option<String>,
    pub cwd: Option<String>,
    pub status: AgentStatus,
    pub opened_ms: i64,
    pub closed_ms: Option<i64>,
    /// Seen in a snapshot or live event this run.
    pub live: bool,
    /// First is Seen|Created; Status steps only on change; Closed|Exited last.
    pub steps: Vec<Step>,
    /// Agent session files seen for this pane, oldest first; usage history comes from these.
    pub sessions: Vec<PathBuf>,
}

impl Lane {
    pub fn label(&self) -> &str {
        self.title.as_deref().filter(|t| !t.is_empty()).or(self.task()).or(self.agent.as_deref()).unwrap_or(&self.pane_id)
    }

    /// What the agent is doing, from its terminal title.
    pub fn task(&self) -> Option<&str> {
        self.terminal_title.as_deref().and_then(task_text)
    }

    pub fn visible(&self) -> bool {
        self.agent.is_some()
    }

    pub fn is_open(&self) -> bool {
        self.closed_ms.is_none()
    }

    pub fn last_ms(&self) -> i64 {
        self.steps.last().map_or(self.opened_ms, |s| s.at_ms)
    }
}

/// Status of `lane` at `t_ms`; None outside its lifetime.
pub fn status_at(lane: &Lane, t_ms: i64) -> Option<AgentStatus> {
    if t_ms < lane.opened_ms || lane.closed_ms.is_some_and(|c| t_ms >= c) {
        return None;
    }
    lane.steps.iter().take_while(|s| s.at_ms <= t_ms).last().map(|s| s.status)
}

/// Time a lane spent in each status within a window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusTime {
    pub working_ms: i64,
    pub blocked_ms: i64,
    /// Time the lane was open at all within the window.
    pub alive_ms: i64,
}

/// How `lane` spent `[from_ms, to_ms)`: each step's status holds until the next step, the lane's
/// close, or `to_ms`.
pub fn status_time(lane: &Lane, from_ms: i64, to_ms: i64) -> StatusTime {
    let end = lane.closed_ms.unwrap_or(i64::MAX).min(to_ms);
    let mut out = StatusTime::default();
    for (i, step) in lane.steps.iter().enumerate() {
        let next = lane.steps.get(i + 1).map_or(end, |s| s.at_ms.min(end));
        let span = (next.min(end) - step.at_ms.max(from_ms)).max(0);
        if span == 0 {
            continue;
        }
        out.alive_ms += span;
        match step.status {
            AgentStatus::Working => out.working_ms += span,
            AgentStatus::Blocked => out.blocked_ms += span,
            _ => {}
        }
    }
    out
}

#[derive(Debug, Default)]
pub struct State {
    pub lanes: Vec<Lane>,
    pub workspaces: BTreeMap<String, String>,
    /// Tab id → label, from the last snapshot.
    pub tabs: BTreeMap<String, String>,
    pub focused_workspace: Option<String>,
    pub focused_tab: Option<String>,
    /// herdr's protocol version, from the last snapshot.
    pub protocol: Option<u32>,
    pub self_pane: Option<String>,
    next_id: u64,
    next_slot: usize,
}

fn nonempty(s: &Option<String>) -> Option<String> {
    s.clone().filter(|s| !s.is_empty())
}

impl State {
    pub fn new(self_pane: Option<String>) -> Self {
        State { self_pane, ..Default::default() }
    }

    fn is_self(&self, pane_id: &str) -> bool {
        self.self_pane.as_deref() == Some(pane_id)
    }

    /// Index of the open lane for `pane_id`.
    fn open_lane(&self, pane_id: &str) -> Option<usize> {
        self.lanes.iter().rposition(|l| l.pane_id == pane_id && l.closed_ms.is_none())
    }

    pub fn lane_by_id(&self, id: u64) -> Option<usize> {
        self.lanes.iter().position(|l| l.id == id)
    }

    pub fn open(&mut self, pane_id: &str, workspace_id: &str, at_ms: i64, mark: Mark, status: AgentStatus, live: bool) -> usize {
        self.next_id += 1;
        self.lanes.push(Lane {
            id: self.next_id,
            slot: None,
            pane_id: pane_id.to_string(),
            workspace_id: workspace_id.to_string(),
            workspace_label: None,
            tab_id: None,
            tab_label: None,
            terminal_id: None,
            agent: None,
            title: None,
            terminal_title: None,
            cwd: None,
            status,
            opened_ms: at_ms,
            closed_ms: None,
            live,
            steps: vec![Step { at_ms, mark, status }],
            sessions: Vec::new(),
        });
        self.lanes.len() - 1
    }

    /// Open lane for `pane_id`, opening a `Seen` one with `status` if there is none.
    fn open_or_seen(&mut self, pane_id: &str, workspace_id: &str, at_ms: i64, status: AgentStatus, live: bool) -> usize {
        match self.open_lane(pane_id) {
            Some(i) => i,
            None => self.open(pane_id, workspace_id, at_ms, Mark::Seen, status, live),
        }
    }

    pub fn set_status(&mut self, idx: usize, status: AgentStatus, at_ms: i64) {
        let lane = &mut self.lanes[idx];
        if lane.status == status {
            return;
        }
        lane.status = status;
        lane.steps.push(Step { at_ms, mark: Mark::Status(status), status });
    }

    pub fn close(&mut self, pane_id: &str, mark: Mark, at_ms: i64) {
        let Some(i) = self.open_lane(pane_id) else { return };
        let lane = &mut self.lanes[i];
        lane.closed_ms = Some(at_ms);
        lane.steps.push(Step { at_ms, mark, status: lane.status });
    }

    /// A pane that moved comes back under a new pane id: when no open lane has `m.pane_id` but one
    /// has its terminal id, that lane takes the new pane id. True when it did.
    fn rejoin(&mut self, m: &PaneMeta) -> bool {
        if self.open_lane(&m.pane_id).is_some() {
            return false;
        }
        let Some(t) = m.terminal_id.as_deref() else { return false };
        let Some(i) = self.lanes.iter().position(|l| l.is_open() && l.terminal_id.as_deref() == Some(t)) else { return false };
        self.lanes[i].pane_id = m.pane_id.clone();
        true
    }

    /// Takes `m`'s fields onto lane `i`. A full record (`full`) replaces agent, titles and cwd
    /// whenever it carries them, so a title herdr cleared doesn't linger; partial facts only
    /// fill in non-empty values.
    fn merge(&mut self, i: usize, m: &PaneMeta, full: bool, live: bool) {
        let lane = &mut self.lanes[i];
        if live {
            lane.live = true;
        }
        if !m.workspace_id.is_empty() {
            lane.workspace_id = m.workspace_id.clone();
        }
        for (slot, value) in [
            (&mut lane.workspace_label, &m.workspace_label),
            (&mut lane.tab_id, &m.tab_id),
            (&mut lane.tab_label, &m.tab_label),
            (&mut lane.terminal_id, &m.terminal_id),
        ] {
            if let Some(v) = nonempty(value) {
                *slot = Some(v);
            }
        }
        for (slot, value) in [(&mut lane.agent, &m.agent), (&mut lane.terminal_title, &m.terminal_title), (&mut lane.cwd, &m.cwd)]
        {
            if full && value.is_some() {
                slot.clone_from(value);
            } else if let Some(v) = nonempty(value) {
                *slot = Some(v);
            }
        }
        if full {
            lane.title.clone_from(&m.title);
        } else if let Some(t) = nonempty(&m.title) {
            lane.title = Some(t);
        }
        if let Some(s) = &m.session
            && !lane.sessions.contains(s)
        {
            lane.sessions.push(s.clone());
        }
    }

    fn update(&mut self, m: &PaneMeta, at_ms: i64, live: bool) {
        let i = self.open_or_seen(&m.pane_id, &m.workspace_id, at_ms, m.status.unwrap_or_default(), live);
        self.merge(i, m, true, live);
        if let Some(s) = m.status {
            self.set_status(i, s, at_ms);
        }
    }

    /// Folds one pane fact at `at_ms`; `live` marks facts from this run (snapshot or subscription).
    pub fn apply(&mut self, e: &HostEvent, at_ms: i64, live: bool) {
        if let HostEvent::Moved { previous_pane_id, pane } = e
            && self.is_self(previous_pane_id)
        {
            // The pinned band followed focus to another workspace: keep skipping it.
            self.self_pane = Some(pane.pane_id.clone());
            return;
        }
        if self.is_self(e.pane_id()) {
            return;
        }
        let rejoined = e.meta().is_some_and(|m| self.rejoin(m));
        match e {
            // A move herdr reports as a create must not split the history.
            HostEvent::Created(m) if rejoined => self.update(m, at_ms, live),
            HostEvent::Created(m) => {
                self.close(&m.pane_id, Mark::Closed, at_ms);
                let i = self.open(&m.pane_id, &m.workspace_id, at_ms, Mark::Created, m.status.unwrap_or_default(), live);
                self.merge(i, m, false, live);
            }
            HostEvent::Updated(m) => self.update(m, at_ms, live),
            HostEvent::Detected { meta, released } => {
                let i = self.open_or_seen(&meta.pane_id, &meta.workspace_id, at_ms, meta.status.unwrap_or_default(), live);
                let agent = if *released { None } else { meta.agent.clone() };
                self.merge(i, &PaneMeta { agent, ..meta.clone() }, false, live);
                if let Some(s) = meta.status {
                    self.set_status(i, s, at_ms);
                }
            }
            HostEvent::Status(m) => {
                let status = m.status.unwrap_or_default();
                let i = self.open_or_seen(&m.pane_id, &m.workspace_id, at_ms, status, live);
                self.merge(i, m, false, live);
                self.set_status(i, status, at_ms);
            }
            HostEvent::Moved { previous_pane_id, pane } => {
                if let Some(i) = self.open_lane(previous_pane_id) {
                    self.lanes[i].pane_id = pane.pane_id.clone();
                }
                self.update(pane, at_ms, live);
            }
            HostEvent::Closed { pane_id } => self.close(pane_id, Mark::Closed, at_ms),
            HostEvent::Exited { pane_id } => self.close(pane_id, Mark::Exited, at_ms),
        }
        self.assign_slots();
    }

    pub fn apply_snapshot(&mut self, snap: &Snapshot, now_ms: i64) {
        self.workspaces = snap
            .workspaces
            .iter()
            .map(|w| (w.workspace_id.clone(), w.label.clone().unwrap_or_else(|| w.workspace_id.clone())))
            .collect();
        self.tabs = snap.tabs.iter().map(|t| (t.tab_id.clone(), t.label.clone().unwrap_or_else(|| t.tab_id.clone()))).collect();
        self.focused_workspace.clone_from(&snap.focused_workspace_id);
        self.focused_tab.clone_from(&snap.focused_tab_id);
        self.protocol = snap.protocol;
        for p in &snap.panes {
            self.apply(&HostEvent::Updated(p.into()), now_ms, true);
        }
        let present: BTreeSet<&str> = snap.panes.iter().map(|p| p.pane_id.as_str()).collect();
        let gone: Vec<(String, i64)> = self
            .lanes
            .iter()
            .filter(|l| l.is_open() && !present.contains(l.pane_id.as_str()))
            // A log-only lane's pane died while nothing was watching: end it at its last step.
            .map(|l| (l.pane_id.clone(), if l.live { now_ms } else { l.last_ms() }))
            .collect();
        for (id, at) in gone {
            self.close(&id, Mark::Closed, at);
        }
        self.lanes.retain(|l| l.closed_ms.is_none_or(|c| c >= now_ms - HISTORY_MS));
        self.assign_slots();
    }

    /// Gives each visible lane without a palette slot the next one, in first-seen (vector) order,
    /// so an agent keeps its color however many others come and go.
    pub fn assign_slots(&mut self) {
        for lane in &mut self.lanes {
            if lane.visible() && lane.slot.is_none() {
                lane.slot = Some(self.next_slot);
                self.next_slot += 1;
            }
        }
    }

    fn workspace_of<'a>(&'a self, lane: &'a Lane) -> &'a str {
        self.workspaces
            .get(&lane.workspace_id)
            .map(String::as_str)
            .or(lane.workspace_label.as_deref())
            .unwrap_or(&lane.workspace_id)
    }

    /// Workspace label for display: snapshot label, else the logged one, else the id.
    pub fn workspace_label(&self, idx: usize) -> &str {
        self.workspace_of(&self.lanes[idx])
    }

    /// Tab label for display: snapshot label, else the logged one.
    pub fn tab_label(&self, idx: usize) -> Option<&str> {
        let lane = &self.lanes[idx];
        lane.tab_id.as_ref().and_then(|t| self.tabs.get(t)).map(String::as_str).or(lane.tab_label.as_deref())
    }

    /// Indices of agent lanes, by workspace label, then open time, then pane id.
    pub fn visible_lanes(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.lanes.len()).filter(|&i| self.lanes[i].visible()).collect();
        idx.sort_by(|&a, &b| {
            let (la, lb) = (&self.lanes[a], &self.lanes[b]);
            (self.workspace_of(la), la.opened_ms, &la.pane_id).cmp(&(self.workspace_of(lb), lb.opened_ms, &lb.pane_id))
        });
        idx
    }

    /// Pane ids of open lanes seen live; the status subscription covers these.
    pub fn open_live_ids(&self) -> BTreeSet<String> {
        self.lanes.iter().filter(|l| l.is_open() && l.live).map(|l| l.pane_id.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::{Envelope, host_event};
    use crate::history::LogEvent;
    use serde_json::json;

    fn log(ts_ms: i64, event: &str, pane_id: &str, status: Option<AgentStatus>) -> LogEvent {
        LogEvent {
            ts_ms,
            session: "default".into(),
            event: event.into(),
            pane_id: pane_id.into(),
            workspace_id: "w1".into(),
            workspace_label: None,
            tab_label: None,
            cwd: None,
            agent: Some("omp".into()),
            status,
        }
    }

    fn fold(s: &mut State, e: &LogEvent) {
        if let Some(h) = e.host_event() {
            s.apply(&h, e.ts_ms, false);
        }
    }

    fn envelope(s: &mut State, event: &str, data: serde_json::Value, at_ms: i64) {
        let h = host_event(&Envelope { event: event.into(), data }).expect("tracked event");
        s.apply(&h, at_ms, true);
    }

    fn marks(lane: &Lane) -> Vec<Mark> {
        lane.steps.iter().map(|s| s.mark).collect()
    }

    #[test]
    fn log_fold_builds_lanes_per_lifetime() {
        use AgentStatus::*;
        let mut s = State::default();
        for e in [
            log(0, "pane.created", "p1", Some(Idle)),
            log(10, "pane.agent_status_changed", "p1", Some(Working)),
            log(11, "pane.agent_status_changed", "p1", Some(Working)),
            log(20, "pane.agent_status_changed", "p1", Some(Blocked)),
            log(30, "pane.closed", "p1", None),
            log(40, "pane.created", "p1", None),
        ] {
            fold(&mut s, &e);
        }
        assert_eq!(s.lanes.len(), 2);
        assert_eq!(marks(&s.lanes[0]), [Mark::Created, Mark::Status(Working), Mark::Status(Blocked), Mark::Closed]);
        assert_eq!(s.lanes[0].closed_ms, Some(30));
        assert!(s.lanes[1].is_open());
        assert_eq!(s.lanes[1].steps[0], Step { at_ms: 40, mark: Mark::Created, status: Unknown });
        assert_eq!(status_at(&s.lanes[0], 15), Some(Working));
        assert_eq!(status_at(&s.lanes[0], 30), None);
    }

    #[test]
    fn snapshot_merges_with_log_lanes() {
        use AgentStatus::*;
        let mut s = State::default();
        fold(&mut s, &log(0, "pane.created", "p1", Some(Idle)));
        fold(&mut s, &log(50, "pane.agent_status_changed", "p1", Some(Working)));
        fold(&mut s, &log(5, "pane.created", "p3", Some(Idle)));
        let snap: Snapshot = serde_json::from_value(json!({
            "workspaces": [{"workspace_id": "w1", "label": "repo"}],
            "tabs": [{"tab_id": "w1:t1", "workspace_id": "w1", "label": "main"}],
            "protocol": 22,
            "panes": [
                {"pane_id": "p2", "workspace_id": "w1", "agent": "omp", "agent_status": "idle"},
                {"pane_id": "p3", "workspace_id": "w1", "tab_id": "w1:t1", "agent": "omp", "agent_status": "working",
                 "terminal_title_stripped": "π ⠇ fix login",
                 "agent_session": {"kind": "path", "value": "/s/a.jsonl"}},
            ],
        }))
        .unwrap();
        s.apply_snapshot(&snap, 1000);

        let p1 = &s.lanes[s.lanes.iter().position(|l| l.pane_id == "p1").unwrap()];
        assert_eq!(p1.closed_ms, Some(50), "log-only lane ends at its last step");
        assert_eq!(p1.steps.last().unwrap().mark, Mark::Closed);

        let p2 = s.lanes.iter().find(|l| l.pane_id == "p2").unwrap();
        assert_eq!(p2.steps, [Step { at_ms: 1000, mark: Mark::Seen, status: Idle }]);
        assert!(p2.live);

        let i3 = s.lanes.iter().position(|l| l.pane_id == "p3").unwrap();
        let p3 = &s.lanes[i3];
        assert!(p3.live && p3.is_open());
        assert_eq!(p3.steps.last().unwrap(), &Step { at_ms: 1000, mark: Mark::Status(Working), status: Working });
        assert_eq!(p3.label(), "fix login");
        assert_eq!(p3.sessions, [PathBuf::from("/s/a.jsonl")]);
        assert_eq!(s.tab_label(i3), Some("main"));
        assert_eq!(s.protocol, Some(22));
        assert_eq!(s.open_live_ids().into_iter().collect::<Vec<_>>(), ["p2", "p3"]);
    }

    #[test]
    fn snapshot_closes_live_lane_now_and_prunes_old() {
        let mut s = State::default();
        let i = s.open("p1", "w1", 0, Mark::Seen, AgentStatus::Idle, true);
        s.lanes[i].agent = Some("omp".into());
        s.apply_snapshot(&Snapshot::default(), 500);
        assert_eq!(s.lanes[0].closed_ms, Some(500));
        s.apply_snapshot(&Snapshot::default(), 500 + HISTORY_MS + 1);
        assert!(s.lanes.is_empty());
    }

    #[test]
    fn dotted_and_snake_status_events_match() {
        for kind in ["pane.agent_status_changed", "pane_agent_status_changed"] {
            let mut s = State::default();
            s.open("p1", "w1", 0, Mark::Seen, AgentStatus::Idle, true);
            envelope(&mut s, kind, json!({"pane_id": "p1", "workspace_id": "w1", "agent_status": "working"}), 10);
            assert_eq!(s.lanes[0].status, AgentStatus::Working, "{kind}");
            assert_eq!(s.lanes[0].steps.last().unwrap().mark, Mark::Status(AgentStatus::Working));
        }
    }

    #[test]
    fn moved_renames_open_lane() {
        let mut s = State::default();
        s.open("w1:p1", "w1", 0, Mark::Seen, AgentStatus::Idle, true);
        let data = json!({"previous_pane_id": "w1:p1",
                          "pane": {"pane_id": "w2:p4", "workspace_id": "w2", "agent": "omp", "agent_status": "idle"}});
        envelope(&mut s, "pane_moved", data, 5);
        assert_eq!(s.lanes.len(), 1);
        assert_eq!((s.lanes[0].pane_id.as_str(), s.lanes[0].workspace_id.as_str()), ("w2:p4", "w2"));
        assert_eq!(s.lanes[0].agent.as_deref(), Some("omp"));
    }

    #[test]
    fn moved_self_stays_self() {
        let mut s = State::new(Some("w1:p2".into()));
        let data = json!({"previous_pane_id": "w1:p2", "pane": {"pane_id": "w3:p5", "workspace_id": "w3"}});
        envelope(&mut s, "pane_moved", data, 5);
        assert!(s.lanes.is_empty());
        envelope(&mut s, "pane_updated", json!({"pane": {"pane_id": "w3:p5", "workspace_id": "w3"}}), 6);
        assert!(s.lanes.is_empty(), "the band's new pane id is still itself");
    }

    #[test]
    fn moved_pane_rejoins_by_terminal_id() {
        let mut s = State::default();
        let first = PaneMeta {
            pane_id: "w1:p1".into(),
            workspace_id: "w1".into(),
            terminal_id: Some("t1".into()),
            agent: Some("omp".into()),
            ..PaneMeta::default()
        };
        s.apply(&HostEvent::Updated(PaneMeta { status: Some(AgentStatus::Idle), ..first.clone() }), 0, true);
        let moved = PaneMeta { pane_id: "w2:p4".into(), workspace_id: "w2".into(), status: Some(AgentStatus::Working), ..first };
        s.apply(&HostEvent::Status(moved), 10, true);
        assert_eq!(s.lanes.len(), 1);
        assert_eq!((s.lanes[0].pane_id.as_str(), s.lanes[0].workspace_id.as_str()), ("w2:p4", "w2"));
        assert_eq!(s.lanes[0].status, AgentStatus::Working);
    }

    #[test]
    fn agent_slots_are_assigned_once_in_first_seen_order() {
        let mut s = State::default();
        for (t, p) in [(0, "p1"), (1, "p2"), (2, "p3")] {
            fold(&mut s, &log(t, "pane.created", p, Some(AgentStatus::Idle)));
        }
        fold(&mut s, &log(10, "pane.closed", "p1", None));
        let pane = |id: &str| json!({"pane_id": id, "workspace_id": "w1", "agent": "omp", "agent_status": "idle"});
        let snap: Snapshot = serde_json::from_value(json!({"panes": [pane("p2"), pane("p3")]})).unwrap();
        s.apply_snapshot(&snap, 10 + HISTORY_MS + 1);
        assert_eq!(s.lanes.len(), 2, "p1 pruned");
        fold(&mut s, &log(20 + HISTORY_MS, "pane.created", "p4", Some(AgentStatus::Idle)));
        let slots: Vec<_> = s.lanes.iter().map(|l| (l.pane_id.as_str(), l.slot)).collect();
        assert_eq!(slots, [("p2", Some(1)), ("p3", Some(2)), ("p4", Some(3))]);
    }

    #[test]
    fn status_time_splits_a_window_by_status() {
        let mut s = State::default();
        let i = s.open("p1", "w1", 0, Mark::Created, AgentStatus::Idle, true);
        s.set_status(i, AgentStatus::Working, 100);
        s.set_status(i, AgentStatus::Blocked, 300);
        s.close("p1", Mark::Closed, 400);
        let l = &s.lanes[0];
        assert_eq!(status_time(l, 0, 1000), StatusTime { working_ms: 200, blocked_ms: 100, alive_ms: 400 });
        assert_eq!(status_time(l, 150, 350), StatusTime { working_ms: 150, blocked_ms: 50, alive_ms: 200 });
        assert_eq!(status_time(l, 500, 600), StatusTime::default(), "after close");
    }

    #[test]
    fn task_text_drops_leading_glyph_tokens() {
        assert_eq!(task_text("π ⠇ issues — plan for issue 4115"), Some("issues — plan for issue 4115"));
        assert_eq!(task_text("  ● build  "), Some("build"));
        assert_eq!(task_text("david@host:~/x"), Some("david@host:~/x"));
        assert_eq!(task_text("π ⠦"), None);
        assert_eq!(task_text("π ⠦ x"), None);
        assert_eq!(task_text(""), None);
    }
}

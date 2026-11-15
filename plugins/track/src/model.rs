//! Wire types and the track state: one lane per pane lifetime, one step per state change, folded
//! from the history log, snapshots and live events.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::herdr::Envelope;
use crate::history::LogEvent;

/// Log window loaded at start; closed lanes older than this are pruned.
pub const HISTORY_MS: i64 = 2 * 3600 * 1000;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Snapshot {
    #[serde(default)]
    pub panes: Vec<PaneInfo>,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceInfo>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PaneInfo {
    pub pane_id: String,
    pub workspace_id: String,
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

/// Agent session identity herdr reports; omp reports `kind = "path"` with its session JSONL.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AgentSession {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceInfo {
    pub workspace_id: String,
    #[serde(default)]
    pub label: Option<String>,
}

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
    pub pane_id: String,
    pub workspace_id: String,
    /// Latest non-empty label from the log.
    pub workspace_label: Option<String>,
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
        self.title
            .as_deref()
            .filter(|t| !t.is_empty())
            .or(self.task())
            .or(self.agent.as_deref())
            .unwrap_or(&self.pane_id)
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
    pub self_pane: Option<String>,
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

    pub fn open(&mut self, pane_id: &str, workspace_id: &str, at_ms: i64, mark: Mark, status: AgentStatus, live: bool) -> usize {
        self.lanes.push(Lane {
            pane_id: pane_id.to_string(),
            workspace_id: workspace_id.to_string(),
            workspace_label: None,
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

    pub fn apply_log(&mut self, e: &LogEvent) {
        if self.is_self(&e.pane_id) {
            return;
        }
        if let Some(i) = self.open_lane(&e.pane_id) {
            let lane = &mut self.lanes[i];
            if let Some(l) = nonempty(&e.workspace_label) {
                lane.workspace_label = Some(l);
            }
            if let Some(a) = nonempty(&e.agent) {
                lane.agent = Some(a);
            }
            if let Some(c) = nonempty(&e.cwd) {
                lane.cwd = Some(c);
            }
        }
        let status = e.status.unwrap_or_default();
        match e.event.as_str() {
            "pane.created" => {
                self.close(&e.pane_id, Mark::Closed, e.ts_ms);
                let i = self.open(&e.pane_id, &e.workspace_id, e.ts_ms, Mark::Created, status, false);
                let lane = &mut self.lanes[i];
                lane.workspace_label = nonempty(&e.workspace_label);
                lane.agent = nonempty(&e.agent);
                lane.cwd = nonempty(&e.cwd);
            }
            "pane.agent_detected" | "pane.agent_status_changed" => {
                let fresh = self.open_lane(&e.pane_id).is_none();
                let i = self.open_or_seen(&e.pane_id, &e.workspace_id, e.ts_ms, status, false);
                if fresh {
                    let lane = &mut self.lanes[i];
                    lane.workspace_label = nonempty(&e.workspace_label);
                    lane.agent = nonempty(&e.agent);
                    lane.cwd = nonempty(&e.cwd);
                }
                if let Some(s) = e.status {
                    self.set_status(i, s, e.ts_ms);
                }
            }
            "pane.closed" => self.close(&e.pane_id, Mark::Closed, e.ts_ms),
            "pane.exited" => self.close(&e.pane_id, Mark::Exited, e.ts_ms),
            _ => {}
        }
    }

    /// Opens a `Seen` lane if needed, then takes the pane's metadata and status as current.
    fn update_pane(&mut self, p: &PaneInfo, now_ms: i64) {
        if self.is_self(&p.pane_id) {
            return;
        }
        let i = self.open_or_seen(&p.pane_id, &p.workspace_id, now_ms, p.agent_status, true);
        let lane = &mut self.lanes[i];
        lane.live = true;
        lane.workspace_id = p.workspace_id.clone();
        lane.title = p.title.clone();
        if p.terminal_title.is_some() {
            lane.terminal_title = p.terminal_title.clone();
        }
        if p.cwd.is_some() {
            lane.cwd = p.cwd.clone();
        }
        if p.agent.is_some() {
            lane.agent = p.agent.clone();
        }
        if let Some(s) = p.agent_session.as_ref().filter(|s| s.kind == "path" && !s.value.is_empty()) {
            let path = PathBuf::from(&s.value);
            if !lane.sessions.contains(&path) {
                lane.sessions.push(path);
            }
        }
        self.set_status(i, p.agent_status, now_ms);
    }

    pub fn apply_snapshot(&mut self, snap: &Snapshot, now_ms: i64) {
        self.workspaces = snap
            .workspaces
            .iter()
            .map(|w| (w.workspace_id.clone(), w.label.clone().unwrap_or_else(|| w.workspace_id.clone())))
            .collect();
        for p in &snap.panes {
            self.update_pane(p, now_ms);
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
    }

    pub fn apply_envelope(&mut self, env: &Envelope, now_ms: i64) {
        let data = env.data.clone();
        match env.event.replace('.', "_").as_str() {
            "pane_created" => {
                let Ok(p) = serde_json::from_value::<PanePayload>(data) else { return };
                if self.is_self(&p.pane.pane_id) {
                    return;
                }
                self.close(&p.pane.pane_id, Mark::Closed, now_ms);
                self.open(&p.pane.pane_id, &p.pane.workspace_id, now_ms, Mark::Created, p.pane.agent_status, true);
                self.update_pane(&p.pane, now_ms);
            }
            "pane_updated" => {
                let Ok(p) = serde_json::from_value::<PanePayload>(data) else { return };
                self.update_pane(&p.pane, now_ms);
            }
            "pane_closed" => {
                let Ok(p) = serde_json::from_value::<PaneRef>(data) else { return };
                self.close(&p.pane_id, Mark::Closed, now_ms);
            }
            "pane_exited" => {
                let Ok(p) = serde_json::from_value::<PaneRef>(data) else { return };
                self.close(&p.pane_id, Mark::Exited, now_ms);
            }
            "pane_agent_detected" => {
                let Ok(p) = serde_json::from_value::<DetectedPayload>(data) else { return };
                if self.is_self(&p.pane_id) {
                    return;
                }
                let i = self.open_or_seen(&p.pane_id, &p.workspace_id, now_ms, AgentStatus::Unknown, true);
                let lane = &mut self.lanes[i];
                lane.live = true;
                if !p.released
                    && let Some(a) = nonempty(&p.agent)
                {
                    lane.agent = Some(a);
                }
            }
            "pane_agent_status_changed" => {
                let Ok(p) = serde_json::from_value::<StatusPayload>(data) else { return };
                if self.is_self(&p.pane_id) {
                    return;
                }
                let i = self.open_or_seen(&p.pane_id, &p.workspace_id, now_ms, p.agent_status, true);
                let lane = &mut self.lanes[i];
                lane.live = true;
                if p.agent.is_some() {
                    lane.agent = p.agent;
                }
                if p.title.is_some() {
                    lane.title = p.title;
                }
                self.set_status(i, p.agent_status, now_ms);
            }
            "pane_moved" => {
                let Ok(p) = serde_json::from_value::<MovedPayload>(data) else { return };
                if self.is_self(&p.previous_pane_id) {
                    // The pinned band followed focus to another workspace: keep skipping it.
                    self.self_pane = Some(p.pane.pane_id);
                    return;
                }
                if let Some(i) = self.open_lane(&p.previous_pane_id) {
                    self.lanes[i].pane_id = p.pane.pane_id.clone();
                }
                self.update_pane(&p.pane, now_ms);
            }
            _ => {}
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
            s.apply_log(&e);
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
        s.apply_log(&log(0, "pane.created", "p1", Some(Idle)));
        s.apply_log(&log(50, "pane.agent_status_changed", "p1", Some(Working)));
        s.apply_log(&log(5, "pane.created", "p3", Some(Idle)));
        let snap: Snapshot = serde_json::from_value(json!({
            "workspaces": [{"workspace_id": "w1", "label": "repo"}],
            "panes": [
                {"pane_id": "p2", "workspace_id": "w1", "agent": "omp", "agent_status": "idle"},
                {"pane_id": "p3", "workspace_id": "w1", "agent": "omp", "agent_status": "working",
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

        let p3 = s.lanes.iter().find(|l| l.pane_id == "p3").unwrap();
        assert!(p3.live && p3.is_open());
        assert_eq!(p3.steps.last().unwrap(), &Step { at_ms: 1000, mark: Mark::Status(Working), status: Working });
        assert_eq!(p3.label(), "fix login");
        assert_eq!(p3.sessions, [PathBuf::from("/s/a.jsonl")]);
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
            let env = Envelope {
                event: kind.into(),
                data: json!({"pane_id": "p1", "workspace_id": "w1", "agent_status": "working"}),
            };
            s.apply_envelope(&env, 10);
            assert_eq!(s.lanes[0].status, AgentStatus::Working, "{kind}");
            assert_eq!(s.lanes[0].steps.last().unwrap().mark, Mark::Status(AgentStatus::Working));
        }
    }

    #[test]
    fn moved_renames_open_lane() {
        let mut s = State::default();
        s.open("w1:p1", "w1", 0, Mark::Seen, AgentStatus::Idle, true);
        let env = Envelope {
            event: "pane_moved".into(),
            data: json!({"previous_pane_id": "w1:p1",
                         "pane": {"pane_id": "w2:p4", "workspace_id": "w2", "agent": "omp", "agent_status": "idle"}}),
        };
        s.apply_envelope(&env, 5);
        assert_eq!(s.lanes.len(), 1);
        assert_eq!((s.lanes[0].pane_id.as_str(), s.lanes[0].workspace_id.as_str()), ("w2:p4", "w2"));
        assert_eq!(s.lanes[0].agent.as_deref(), Some("omp"));
    }

    #[test]
    fn moved_self_stays_self() {
        let mut s = State::new(Some("w1:p2".into()));
        let env = Envelope {
            event: "pane_moved".into(),
            data: json!({"previous_pane_id": "w1:p2", "pane": {"pane_id": "w3:p5", "workspace_id": "w3"}}),
        };
        s.apply_envelope(&env, 5);
        assert!(s.lanes.is_empty());
        s.apply_envelope(&Envelope { event: "pane_updated".into(), data: json!({"pane": {"pane_id": "w3:p5", "workspace_id": "w3"}}) }, 6);
        assert!(s.lanes.is_empty(), "the band's new pane id is still itself");
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

//! The part of a Teams snapshot the UI shows, and what can be derived from it without I/O.

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub revision: u64,
    /// Who is looking: decides which actions the UI offers.
    #[serde(default)]
    pub viewer: Viewer,
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub teams: Vec<Team>,
    #[serde(default)]
    pub actors: Vec<Actor>,
    #[serde(default)]
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub handoffs: Vec<Handoff>,
    #[serde(default)]
    pub decisions: Vec<Decision>,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default)]
    pub approvals: Vec<Approval>,
    #[serde(default)]
    pub recruitments: Vec<Recruitment>,
    #[serde(default)]
    pub conflicts: Vec<Conflict>,
    #[serde(default)]
    pub notices: Vec<Notice>,
    #[serde(default)]
    pub attention: Attention,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Viewer {
    #[serde(default)]
    pub role: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Project {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Team {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub status: Value,
    #[serde(default)]
    pub lead_id: String,
    #[serde(default)]
    pub developer_id: String,
    #[serde(default)]
    pub qa_id: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Actor {
    pub id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub team_id: Option<String>,
    #[serde(default)]
    pub run_generation: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Task {
    pub id: String,
    #[serde(default)]
    pub team_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub developer_id: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub status: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Evidence {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub reference: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Handoff {
    pub id: String,
    #[serde(default)]
    pub task_id: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Decision {
    pub id: String,
    #[serde(default)]
    pub team_id: String,
    #[serde(default)]
    pub task_id: String,
    #[serde(default)]
    pub actor_id: String,
    #[serde(default)]
    pub kind: Value,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub team_id: String,
    #[serde(default)]
    pub sender_id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub kind: Value,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProtectedAction {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub parameters: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Approval {
    pub id: String,
    #[serde(default)]
    pub team_id: String,
    #[serde(default)]
    pub requested_by: String,
    #[serde(default)]
    pub action: ProtectedAction,
    #[serde(default)]
    pub action_digest: String,
    #[serde(default)]
    pub justification: String,
    #[serde(default)]
    pub ttl_seconds: u64,
    #[serde(default)]
    pub status: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Recruitment {
    #[serde(default)]
    pub team_id: String,
    #[serde(default)]
    pub specialty: String,
    #[serde(default)]
    pub status: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Conflict {
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub team_ids: Vec<String>,
    #[serde(default)]
    pub detail: Vec<String>,
    #[serde(default)]
    pub status: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Notice {
    #[serde(default)]
    pub from_team_id: String,
    #[serde(default)]
    pub to_team_id: String,
    #[serde(default)]
    pub status: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Attention {
    #[serde(default)]
    pub pending_approvals: Vec<String>,
    #[serde(default)]
    pub open_conflicts: Vec<String>,
    #[serde(default)]
    pub merge_unknown: Vec<String>,
}

/// The `status` tag of a tagged status value (`{"status": "in_progress", ...}`).
pub fn tag(status: &Value) -> &str {
    status.get("status").and_then(Value::as_str).unwrap_or("")
}

/// What an agent is doing, in words and glyphs so no state depends on color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Working,
    Waiting,
    Approval,
    Blocked,
    Idle,
    Suspended,
}

impl AgentStatus {
    pub fn word(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::Approval => "approval",
            Self::Blocked => "blocked",
            Self::Idle => "idle",
            Self::Suspended => "paused",
        }
    }
}

impl Snapshot {
    pub fn team(&self, id: &str) -> Option<&Team> {
        self.teams.iter().find(|team| team.id == id)
    }

    pub fn actor(&self, id: &str) -> Option<&Actor> {
        self.actors.iter().find(|actor| actor.id == id)
    }

    pub fn team_label(&self, id: &str) -> String {
        self.team(id)
            .map_or_else(|| id.to_owned(), |team| team.name.clone())
    }

    pub fn team_agents(&self, team_id: &str) -> Vec<&Actor> {
        let mut agents: Vec<&Actor> = self
            .actors
            .iter()
            .filter(|actor| actor.team_id.as_deref() == Some(team_id))
            .collect();
        agents.sort_by_key(|actor| (role_rank(&actor.role), actor.id.clone()));
        agents
    }

    pub fn agent_status(&self, actor: &Actor) -> AgentStatus {
        let team_state = actor
            .team_id
            .as_deref()
            .and_then(|id| self.team(id))
            .map(|team| tag(&team.status));
        if matches!(team_state, Some("suspended" | "cancelled")) {
            return AgentStatus::Suspended;
        }
        let team_id = actor.team_id.as_deref().unwrap_or("");
        let owns = |statuses: &[&str]| {
            self.tasks
                .iter()
                .any(|task| task.developer_id == actor.id && statuses.contains(&tag(&task.status)))
        };
        let pending = self.approvals.iter().any(|approval| {
            approval.requested_by == actor.id && tag(&approval.status) == "pending"
        });
        match actor.role.as_str() {
            "developer" => {
                if owns(&["in_progress", "rework_requested"]) {
                    AgentStatus::Working
                } else if owns(&["awaiting_qa"]) {
                    AgentStatus::Waiting
                } else if self.tasks.iter().any(|task| {
                    task.developer_id == actor.id
                        && tag(&task.status) == "assigned"
                        && self.blocked_by(task).is_some()
                }) {
                    AgentStatus::Blocked
                } else if pending {
                    AgentStatus::Approval
                } else {
                    AgentStatus::Idle
                }
            }
            "qa" => {
                let reviewing = self
                    .tasks
                    .iter()
                    .any(|task| task.team_id == team_id && tag(&task.status) == "awaiting_qa");
                if reviewing {
                    AgentStatus::Working
                } else if pending {
                    AgentStatus::Approval
                } else {
                    AgentStatus::Idle
                }
            }
            "specialist" => {
                if self
                    .recruitments
                    .iter()
                    .any(|item| item.team_id == team_id && tag(&item.status) == "active")
                {
                    AgentStatus::Working
                } else {
                    AgentStatus::Waiting
                }
            }
            _ => {
                if pending {
                    AgentStatus::Approval
                } else if self.tasks.iter().any(|task| {
                    task.team_id == team_id
                        && !matches!(tag(&task.status), "finalized" | "cancelled")
                }) {
                    AgentStatus::Working
                } else {
                    AgentStatus::Idle
                }
            }
        }
    }

    /// The first unfinished dependency of `task`, if any.
    pub fn blocked_by(&self, task: &Task) -> Option<&str> {
        task.dependencies.iter().find_map(|id| {
            self.tasks
                .iter()
                .find(|other| &other.id == id && tag(&other.status) != "finalized")
                .map(|other| other.id.as_str())
        })
    }

    pub fn handoff_for(&self, task: &Task) -> Option<&Handoff> {
        let id = ["handoff_id"]
            .iter()
            .find_map(|key| task.status.get(*key).and_then(Value::as_str))?;
        self.handoffs.iter().find(|handoff| handoff.id == id)
    }

    pub fn open_work(&self, team_id: &str) -> usize {
        self.tasks
            .iter()
            .filter(|task| {
                task.team_id == team_id && !matches!(tag(&task.status), "finalized" | "cancelled")
            })
            .count()
    }

    pub fn pending_for(&self, team_id: Option<&str>) -> usize {
        self.approvals
            .iter()
            .filter(|item| {
                tag(&item.status) == "pending" && team_id.is_none_or(|id| item.team_id == id)
            })
            .count()
    }
}

fn role_rank(role: &str) -> u8 {
    match role {
        "lead" => 0,
        "developer" => 1,
        "qa" => 2,
        _ => 3,
    }
}

/// (steps done, steps total, label) for a task's place in assign -> accept -> handoff -> QA -> final.
pub fn task_progress(task: &Task) -> (u8, u8, &'static str) {
    match tag(&task.status) {
        "assigned" => (0, 5, "assigned"),
        "in_progress" => (1, 5, "in progress"),
        "awaiting_qa" => (2, 5, "awaiting QA"),
        "rework_requested" => (1, 5, "rework"),
        "qa_accepted" => (3, 5, "QA accepted"),
        "finalized" => (5, 5, "done"),
        "cancelled" => (0, 5, "cancelled"),
        _ => (0, 5, "unknown"),
    }
}

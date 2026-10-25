use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{Error, Result};

pub const STATE_VERSION: u32 = 1;
pub const MAX_RECORDS: usize = 10_000;
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024;
const MAX_NAME_BYTES: usize = 128;
const MAX_TITLE_BYTES: usize = 256;
const MAX_DEPENDENCIES: usize = 64;
const MAX_EVIDENCE_ITEMS: usize = 64;
const MAX_EVIDENCE_REFERENCE_BYTES: usize = 2048;
const MAX_DISPATCH_ATTEMPTS: usize = 3;
const MAX_EVENT_PAGE: usize = 100;
const OWNER_ID: &str = "owner";
const RECOVERY_ACTOR_ID: &str = "system:recovery";

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Lead,
    Developer,
    Qa,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub id: String,
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_generation: Option<u64>,
    auth_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    pub actor_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_generation: Option<u64>,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TeamSpecStatus {
    Draft,
    Launched { team_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enrollment {
    pub actor_id: String,
    pub credential_digest: String,
    pub run_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamSpec {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub status: TeamSpecStatus,
    pub enrollments: BTreeMap<Role, Enrollment>,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub project_id: String,
    pub spec_id: String,
    pub name: String,
    pub lead_id: String,
    pub developer_id: String,
    pub qa_id: String,
    pub revision: u64,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Commit,
    Artifact,
    Test,
    ChangedPath,
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeveloperAcceptance {
    pub id: String,
    pub team_id: String,
    pub task_id: String,
    pub developer_id: String,
    pub note: String,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TaskStatus {
    Assigned,
    InProgress {
        acceptance_id: String,
    },
    AwaitingQa {
        handoff_id: String,
    },
    ReworkRequested {
        handoff_id: String,
        decision_id: String,
    },
    QaAccepted {
        handoff_id: String,
        decision_id: String,
    },
    Finalized {
        handoff_id: String,
        qa_decision_id: String,
        outcome_decision_id: String,
    },
    Cancelled {
        decision_id: String,
    },
}

impl TaskStatus {
    fn is_finalized(&self) -> bool {
        matches!(self, Self::Finalized { .. })
    }

    fn is_terminal(&self) -> bool {
        matches!(self, Self::Finalized { .. } | Self::Cancelled { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub team_id: String,
    pub title: String,
    pub description: String,
    pub developer_id: String,
    pub dependencies: BTreeSet<String>,
    pub status: TaskStatus,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    pub id: String,
    pub team_id: String,
    pub task_id: String,
    pub developer_id: String,
    pub summary: String,
    pub evidence: Vec<Evidence>,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QaVerdict {
    Accept,
    Rework,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DecisionKind {
    QaReview {
        handoff_id: String,
        verdict: QaVerdict,
    },
    Outcome {
        qa_decision_id: String,
    },
    Cancellation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub team_id: String,
    pub task_id: String,
    pub actor_id: String,
    pub kind: DecisionKind,
    pub body: String,
    pub evidence: Vec<Evidence>,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MessageKind {
    Text,
    TaskAssignment {
        task_id: String,
    },
    Handoff {
        task_id: String,
        handoff_id: String,
    },
    QaDecision {
        task_id: String,
        decision_id: String,
    },
    Outcome {
        task_id: String,
        decision_id: String,
    },
    Cancellation {
        task_id: String,
        decision_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub team_id: String,
    pub sender_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub kind: MessageKind,
    pub body: String,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
pub enum KnowledgeScope {
    Project { project_id: String },
    Team { team_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub id: String,
    pub team_id: String,
    pub actor_id: String,
    pub scope: KnowledgeScope,
    pub title: String,
    pub body: String,
    pub evidence: Vec<Evidence>,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DispatchIntent {
    pub attempt_id: String,
    pub run_generation: u64,
    pub content_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InFlightDispatchState {
    Attempted,
    Submitted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DispatchState {
    Queued,
    Attempted,
    Submitted {
        provider_session: String,
    },
    Acknowledged {
        provider_session: String,
    },
    Accepted {
        provider_session: String,
    },
    Rejected {
        provider_session: String,
        reason: String,
    },
    Completed {
        provider_session: String,
        evidence: Vec<Evidence>,
    },
    Failed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_session: Option<String>,
        reason: String,
    },
    Cancelled,
    Unknown {
        previous: InFlightDispatchState,
    },
}

impl DispatchState {
    fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Rejected { .. } | Self::Failed { .. } | Self::Cancelled | Self::Unknown { .. }
        )
    }

    fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Rejected { .. }
                | Self::Completed { .. }
                | Self::Failed { .. }
                | Self::Cancelled
                | Self::Unknown { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchAttempt {
    pub id: String,
    pub team_id: String,
    pub task_id: String,
    pub message_id: String,
    pub requested_by: String,
    pub target_actor_id: String,
    pub run_generation: u64,
    pub content_digest: String,
    pub attempt_number: u8,
    pub state: DispatchState,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DispatchReceiptKind {
    Attempted,
    Submitted,
    Acknowledged,
    Accepted,
    Rejected,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub sequence: u64,
    pub revision: u64,
    pub actor_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    pub kind: String,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredReceipt {
    pub digest: String,
    pub result: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandClass {
    Owner,
    Lead,
    Developer,
    Qa,
    Shared,
    Dispatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    CreateProject {
        project_id: String,
        name: String,
    },
    CreateTeamSpec {
        spec_id: String,
        project_id: String,
        name: String,
    },
    EnrollActor {
        spec_id: String,
        actor_id: String,
        role: Role,
        credential_digest: String,
        run_generation: u64,
    },
    LaunchTeam {
        spec_id: String,
        team_id: String,
    },
    CreateTask {
        team_id: String,
        task_id: String,
        title: String,
        description: String,
        developer_id: String,
        #[serde(default)]
        dependencies: Vec<String>,
        assignment_message_id: String,
        dispatch: DispatchIntent,
    },
    SetTaskDependencies {
        team_id: String,
        task_id: String,
        dependencies: Vec<String>,
    },
    AcceptTask {
        team_id: String,
        task_id: String,
        acceptance_id: String,
        note: String,
    },
    SubmitHandoff {
        team_id: String,
        task_id: String,
        handoff_id: String,
        message_id: String,
        summary: String,
        evidence: Vec<Evidence>,
        dispatch: DispatchIntent,
    },
    ReviewHandoff {
        team_id: String,
        task_id: String,
        handoff_id: String,
        decision_id: String,
        message_id: String,
        verdict: QaVerdict,
        rationale: String,
        #[serde(default)]
        evidence: Vec<Evidence>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rework_dispatch: Option<DispatchIntent>,
    },
    FinalizeTask {
        team_id: String,
        task_id: String,
        decision_id: String,
        message_id: String,
        outcome: String,
        #[serde(default)]
        evidence: Vec<Evidence>,
    },
    CancelTask {
        team_id: String,
        task_id: String,
        decision_id: String,
        message_id: String,
        reason: String,
    },
    AppendChat {
        team_id: String,
        message_id: String,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
    },
    PutKnowledge {
        team_id: String,
        knowledge_id: String,
        scope: KnowledgeScope,
        title: String,
        body: String,
        #[serde(default)]
        evidence: Vec<Evidence>,
    },
    RecordDispatchReceipt {
        team_id: String,
        attempt_id: String,
        task_id: String,
        message_id: String,
        run_generation: u64,
        content_digest: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_session: Option<String>,
        receipt: DispatchReceiptKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default)]
        evidence: Vec<Evidence>,
    },
    RetryDispatch {
        team_id: String,
        previous_attempt_id: String,
        dispatch: DispatchIntent,
    },
}

impl Command {
    pub fn method_class(&self) -> CommandClass {
        match self {
            Self::CreateProject { .. }
            | Self::CreateTeamSpec { .. }
            | Self::EnrollActor { .. }
            | Self::LaunchTeam { .. } => CommandClass::Owner,
            Self::CreateTask { .. }
            | Self::SetTaskDependencies { .. }
            | Self::FinalizeTask { .. }
            | Self::CancelTask { .. } => CommandClass::Lead,
            Self::AcceptTask { .. } | Self::SubmitHandoff { .. } => CommandClass::Developer,
            Self::ReviewHandoff { .. } => CommandClass::Qa,
            Self::AppendChat { .. } | Self::PutKnowledge { .. } => CommandClass::Shared,
            Self::RecordDispatchReceipt { .. } | Self::RetryDispatch { .. } => {
                CommandClass::Dispatch
            }
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::CreateProject { .. } => "create_project",
            Self::CreateTeamSpec { .. } => "create_team_spec",
            Self::EnrollActor { .. } => "enroll_actor",
            Self::LaunchTeam { .. } => "launch_team",
            Self::CreateTask { .. } => "create_task",
            Self::SetTaskDependencies { .. } => "set_task_dependencies",
            Self::AcceptTask { .. } => "accept_task",
            Self::SubmitHandoff { .. } => "submit_handoff",
            Self::ReviewHandoff { .. } => "review_handoff",
            Self::FinalizeTask { .. } => "finalize_task",
            Self::CancelTask { .. } => "cancel_task",
            Self::AppendChat { .. } => "append_chat",
            Self::PutKnowledge { .. } => "put_knowledge",
            Self::RecordDispatchReceipt { .. } => "record_dispatch_receipt",
            Self::RetryDispatch { .. } => "retry_dispatch",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    pub revision: u64,
    pub stream_id: String,
    pub next_sequence: u64,
    pub events: Vec<Event>,
    pub receipts: BTreeMap<String, StoredReceipt>,
    pub projects: BTreeMap<String, Project>,
    pub team_specs: BTreeMap<String, TeamSpec>,
    pub teams: BTreeMap<String, Team>,
    pub actors: BTreeMap<String, Actor>,
    pub credentials: BTreeMap<String, Credential>,
    pub tasks: BTreeMap<String, Task>,
    pub acceptances: BTreeMap<String, DeveloperAcceptance>,
    pub handoffs: BTreeMap<String, Handoff>,
    pub decisions: BTreeMap<String, Decision>,
    pub messages: BTreeMap<String, Message>,
    pub knowledge: BTreeMap<String, KnowledgeEntry>,
    pub dispatches: BTreeMap<String, DispatchAttempt>,
}

struct PendingEvent {
    project_id: Option<String>,
    team_id: Option<String>,
    kind: &'static str,
    payload: Value,
}

struct Applied {
    result: Value,
    events: Vec<PendingEvent>,
}

impl State {
    pub fn new(stream_id: String, owner_digest: String) -> Self {
        let owner = Actor {
            id: OWNER_ID.to_owned(),
            role: Role::Owner,
            team_id: None,
            run_generation: None,
            auth_digest: owner_digest.clone(),
        };
        Self {
            version: STATE_VERSION,
            revision: 0,
            stream_id,
            next_sequence: 1,
            events: Vec::new(),
            receipts: BTreeMap::new(),
            projects: BTreeMap::new(),
            team_specs: BTreeMap::new(),
            teams: BTreeMap::new(),
            actors: BTreeMap::from([(owner.id.clone(), owner)]),
            credentials: BTreeMap::from([(
                owner_digest,
                Credential {
                    actor_id: OWNER_ID.to_owned(),
                    run_generation: None,
                    active: true,
                },
            )]),
            tasks: BTreeMap::new(),
            acceptances: BTreeMap::new(),
            handoffs: BTreeMap::new(),
            decisions: BTreeMap::new(),
            messages: BTreeMap::new(),
            knowledge: BTreeMap::new(),
            dispatches: BTreeMap::new(),
        }
    }

    pub fn authenticate(&self, digest: &str) -> Result<Actor> {
        let credential = self
            .credentials
            .get(digest)
            .filter(|credential| credential.active)
            .ok_or_else(|| Error::new("unauthorized", "invalid or revoked credential"))?;
        let actor = self
            .actors
            .get(&credential.actor_id)
            .ok_or_else(|| Error::new("corrupt_state", "credential actor is missing"))?;
        if actor.auth_digest != digest || actor.run_generation != credential.run_generation {
            return Err(Error::new(
                "unauthorized",
                "credential does not match the current actor run",
            ));
        }
        Ok(actor.clone())
    }

    pub fn execute(&mut self, actor: &Actor, command: Command) -> Result<Value> {
        self.ensure_current_actor(actor)?;
        let commit_revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::new("capacity_exceeded", "state revision is exhausted"))?;
        let mut candidate = self.clone();
        let applied = candidate.apply(actor, command, commit_revision)?;
        if applied.events.is_empty() {
            return Err(Error::new(
                "corrupt_state",
                "a successful command produced no durable event",
            ));
        }
        candidate.revision = commit_revision;
        for pending in applied.events {
            candidate.append_event(actor.id.clone(), commit_revision, pending)?;
        }
        candidate.validate()?;
        *self = candidate;
        Ok(applied.result)
    }

    pub fn snapshot(&self, actor: &Actor, team_id: Option<&str>) -> Result<Value> {
        self.validate()?;
        self.ensure_current_actor(actor)?;
        let selected_team = self.resolve_read_team(actor, team_id)?;
        let selected_ids: BTreeSet<&str> = match selected_team {
            Some(team_id) => BTreeSet::from([team_id]),
            None => self.teams.keys().map(String::as_str).collect(),
        };
        let projects: Vec<&Project> = self
            .projects
            .values()
            .filter(|project| {
                actor.role == Role::Owner
                    || self.teams.values().any(|team| {
                        selected_ids.contains(team.id.as_str()) && team.project_id == project.id
                    })
            })
            .collect();
        let specs: Vec<Value> = self
            .team_specs
            .values()
            .filter(|spec| match &spec.status {
                TeamSpecStatus::Draft => actor.role == Role::Owner && selected_team.is_none(),
                TeamSpecStatus::Launched { team_id } => selected_ids.contains(team_id.as_str()),
            })
            .map(team_spec_view)
            .collect();
        let teams: Vec<&Team> = self
            .teams
            .values()
            .filter(|team| selected_ids.contains(team.id.as_str()))
            .collect();
        let actors: Vec<Value> = self
            .actors
            .values()
            .filter(|candidate| {
                candidate.role != Role::Owner
                    && candidate
                        .team_id
                        .as_deref()
                        .is_some_and(|id| selected_ids.contains(id))
            })
            .map(actor_view)
            .collect();
        let tasks = filtered_values(&self.tasks, |value| {
            selected_ids.contains(value.team_id.as_str())
        });
        let acceptances = filtered_values(&self.acceptances, |value| {
            selected_ids.contains(value.team_id.as_str())
        });
        let handoffs = filtered_values(&self.handoffs, |value| {
            selected_ids.contains(value.team_id.as_str())
        });
        let decisions = filtered_values(&self.decisions, |value| {
            selected_ids.contains(value.team_id.as_str())
        });
        let messages = filtered_values(&self.messages, |value| {
            selected_ids.contains(value.team_id.as_str())
        });
        let knowledge = filtered_values(&self.knowledge, |value| {
            selected_ids.contains(value.team_id.as_str())
        });
        let dispatches = filtered_values(&self.dispatches, |value| {
            selected_ids.contains(value.team_id.as_str())
        });
        serialize_value(&json!({
            "version": self.version,
            "revision": self.revision,
            "stream_id": self.stream_id,
            "last_event_sequence": self.next_sequence.saturating_sub(1),
            "projects": projects,
            "team_specs": specs,
            "teams": teams,
            "actors": actors,
            "tasks": tasks,
            "acceptances": acceptances,
            "handoffs": handoffs,
            "decisions": decisions,
            "messages": messages,
            "knowledge": knowledge,
            "dispatches": dispatches,
        }))
    }

    pub fn event_page(
        &self,
        actor: &Actor,
        stream_id: &str,
        after: u64,
        limit: usize,
    ) -> Result<Value> {
        self.validate()?;
        self.ensure_current_actor(actor)?;
        if stream_id != self.stream_id {
            return Err(Error::new(
                "cursor_expired",
                "event cursor belongs to another stream",
            ));
        }
        if limit == 0 || limit > MAX_EVENT_PAGE {
            return Err(Error::new(
                "invalid_params",
                format!("event page limit must be between 1 and {MAX_EVENT_PAGE}"),
            ));
        }
        let last_sequence = self.next_sequence.saturating_sub(1);
        if after > last_sequence {
            return Err(Error::new(
                "cursor_expired",
                "event cursor is ahead of the durable stream",
            ));
        }

        let mut page = Vec::new();
        let mut next_after = after;
        for event in self.events.iter().filter(|event| event.sequence > after) {
            if page.len() == limit {
                break;
            }
            next_after = event.sequence;
            if self.actor_can_read_event(actor, event) {
                page.push(event);
            }
        }
        let has_more = self
            .events
            .iter()
            .any(|event| event.sequence > next_after && self.actor_can_read_event(actor, event));
        serialize_value(&json!({
            "stream_id": self.stream_id,
            "after": after,
            "next_after": next_after,
            "has_more": has_more,
            "events": page,
        }))
    }

    pub fn recover(&mut self) -> Result<bool> {
        let affected: Vec<String> = self
            .dispatches
            .iter()
            .filter_map(|(id, attempt)| {
                matches!(
                    attempt.state,
                    DispatchState::Attempted | DispatchState::Submitted { .. }
                )
                .then(|| id.clone())
            })
            .collect();
        if affected.is_empty() {
            return Ok(false);
        }

        let mut candidate = self.clone();
        let revision = candidate
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::new("capacity_exceeded", "state revision is exhausted"))?;
        for attempt_id in &affected {
            let attempt = candidate.dispatches.get_mut(attempt_id).ok_or_else(|| {
                Error::new("corrupt_state", "dispatch disappeared during recovery")
            })?;
            let previous = match attempt.state {
                DispatchState::Attempted => InFlightDispatchState::Attempted,
                DispatchState::Submitted { .. } => InFlightDispatchState::Submitted,
                _ => continue,
            };
            attempt.state = DispatchState::Unknown { previous };
            attempt.revision = revision;
        }
        candidate.revision = revision;
        candidate.append_event(
            RECOVERY_ACTOR_ID.to_owned(),
            revision,
            PendingEvent {
                project_id: None,
                team_id: None,
                kind: "dispatch_recovery_unknown",
                payload: json!({ "attempt_ids": affected }),
            },
        )?;
        candidate.validate()?;
        *self = candidate;
        Ok(true)
    }

    fn apply(&mut self, actor: &Actor, command: Command, revision: u64) -> Result<Applied> {
        match command {
            Command::CreateProject { project_id, name } => {
                self.create_project(actor, project_id, name, revision)
            }
            Command::CreateTeamSpec {
                spec_id,
                project_id,
                name,
            } => self.create_team_spec(actor, spec_id, project_id, name, revision),
            Command::EnrollActor {
                spec_id,
                actor_id,
                role,
                credential_digest,
                run_generation,
            } => self.enroll_actor(
                actor,
                spec_id,
                actor_id,
                role,
                credential_digest,
                run_generation,
                revision,
            ),
            Command::LaunchTeam { spec_id, team_id } => {
                self.launch_team(actor, spec_id, team_id, revision)
            }
            Command::CreateTask {
                team_id,
                task_id,
                title,
                description,
                developer_id,
                dependencies,
                assignment_message_id,
                dispatch,
            } => self.create_task(
                actor,
                team_id,
                task_id,
                title,
                description,
                developer_id,
                dependencies,
                assignment_message_id,
                dispatch,
                revision,
            ),
            Command::SetTaskDependencies {
                team_id,
                task_id,
                dependencies,
            } => self.set_task_dependencies(actor, team_id, task_id, dependencies, revision),
            Command::AcceptTask {
                team_id,
                task_id,
                acceptance_id,
                note,
            } => self.accept_task(actor, team_id, task_id, acceptance_id, note, revision),
            Command::SubmitHandoff {
                team_id,
                task_id,
                handoff_id,
                message_id,
                summary,
                evidence,
                dispatch,
            } => self.submit_handoff(
                actor, team_id, task_id, handoff_id, message_id, summary, evidence, dispatch,
                revision,
            ),
            Command::ReviewHandoff {
                team_id,
                task_id,
                handoff_id,
                decision_id,
                message_id,
                verdict,
                rationale,
                evidence,
                rework_dispatch,
            } => self.review_handoff(
                actor,
                team_id,
                task_id,
                handoff_id,
                decision_id,
                message_id,
                verdict,
                rationale,
                evidence,
                rework_dispatch,
                revision,
            ),
            Command::FinalizeTask {
                team_id,
                task_id,
                decision_id,
                message_id,
                outcome,
                evidence,
            } => self.finalize_task(
                actor,
                team_id,
                task_id,
                decision_id,
                message_id,
                outcome,
                evidence,
                revision,
            ),
            Command::CancelTask {
                team_id,
                task_id,
                decision_id,
                message_id,
                reason,
            } => self.cancel_task(
                actor,
                team_id,
                task_id,
                decision_id,
                message_id,
                reason,
                revision,
            ),
            Command::AppendChat {
                team_id,
                message_id,
                body,
                task_id,
            } => self.append_chat(actor, team_id, message_id, body, task_id, revision),
            Command::PutKnowledge {
                team_id,
                knowledge_id,
                scope,
                title,
                body,
                evidence,
            } => self.put_knowledge(
                actor,
                team_id,
                knowledge_id,
                scope,
                title,
                body,
                evidence,
                revision,
            ),
            Command::RecordDispatchReceipt {
                team_id,
                attempt_id,
                task_id,
                message_id,
                run_generation,
                content_digest,
                provider_session,
                receipt,
                detail,
                evidence,
            } => self.record_dispatch_receipt(
                actor,
                team_id,
                attempt_id,
                task_id,
                message_id,
                run_generation,
                content_digest,
                provider_session,
                receipt,
                detail,
                evidence,
                revision,
            ),
            Command::RetryDispatch {
                team_id,
                previous_attempt_id,
                dispatch,
            } => self.retry_dispatch(actor, team_id, previous_attempt_id, dispatch, revision),
        }
    }

    fn append_event(&mut self, actor_id: String, revision: u64, event: PendingEvent) -> Result<()> {
        let sequence = self.next_sequence;
        self.next_sequence = sequence
            .checked_add(1)
            .ok_or_else(|| Error::new("capacity_exceeded", "event sequence is exhausted"))?;
        self.events.push(Event {
            sequence,
            revision,
            actor_id,
            project_id: event.project_id,
            team_id: event.team_id,
            kind: event.kind.to_owned(),
            payload: event.payload,
        });
        Ok(())
    }

    fn ensure_current_actor(&self, actor: &Actor) -> Result<()> {
        let current = self
            .actors
            .get(&actor.id)
            .ok_or_else(|| Error::new("unauthorized", "actor is not enrolled"))?;
        if current != actor {
            return Err(Error::new(
                "unauthorized",
                "actor identity or run generation is stale",
            ));
        }
        let credential = self
            .credentials
            .get(&actor.auth_digest)
            .filter(|credential| credential.active)
            .ok_or_else(|| Error::new("unauthorized", "actor credential is revoked"))?;
        if credential.actor_id != actor.id || credential.run_generation != actor.run_generation {
            return Err(Error::new(
                "unauthorized",
                "actor credential scope does not match",
            ));
        }
        Ok(())
    }

    fn resolve_read_team<'a>(
        &'a self,
        actor: &'a Actor,
        requested: Option<&'a str>,
    ) -> Result<Option<&'a str>> {
        if actor.role == Role::Owner {
            if let Some(team_id) = requested {
                self.require_team(team_id)?;
            }
            return Ok(requested);
        }
        let actor_team = actor
            .team_id
            .as_deref()
            .ok_or_else(|| Error::new("forbidden", "actor has no team scope"))?;
        if requested.is_some_and(|requested| requested != actor_team) {
            return Err(Error::new("forbidden", "actor cannot read another team"));
        }
        Ok(Some(actor_team))
    }

    fn actor_can_read_event(&self, actor: &Actor, event: &Event) -> bool {
        actor.role == Role::Owner
            || actor
                .team_id
                .as_deref()
                .zip(event.team_id.as_deref())
                .is_some_and(|(actor_team, event_team)| actor_team == event_team)
    }
}

impl State {
    fn create_project(
        &mut self,
        actor: &Actor,
        project_id: String,
        name: String,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        validate_id("project_id", &project_id)?;
        validate_text("project name", &name, MAX_NAME_BYTES, false)?;
        if !self.projects.is_empty() {
            return Err(Error::new(
                "phase1_limit",
                "Phase 1 supports one project per session",
            ));
        }
        if self.projects.contains_key(&project_id) {
            return Err(Error::new("already_exists", "project already exists"));
        }
        let project = Project {
            id: project_id.clone(),
            name,
            revision,
        };
        self.projects.insert(project_id.clone(), project.clone());
        Ok(applied(
            serialize_value(&project)?,
            None,
            Some(project_id.clone()),
            "project_created",
            json!({ "project_id": project_id }),
        ))
    }

    fn create_team_spec(
        &mut self,
        actor: &Actor,
        spec_id: String,
        project_id: String,
        name: String,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        validate_id("spec_id", &spec_id)?;
        validate_text("team name", &name, MAX_NAME_BYTES, false)?;
        self.require_project(&project_id)?;
        if self.team_specs.contains_key(&spec_id) {
            return Err(Error::new("already_exists", "team spec already exists"));
        }
        if self
            .team_specs
            .values()
            .any(|spec| matches!(spec.status, TeamSpecStatus::Draft))
        {
            return Err(Error::new(
                "phase1_limit",
                "Phase 1 supports one draft team spec",
            ));
        }
        let spec = TeamSpec {
            id: spec_id.clone(),
            project_id: project_id.clone(),
            name,
            status: TeamSpecStatus::Draft,
            enrollments: BTreeMap::new(),
            revision,
        };
        let result = team_spec_view(&spec);
        self.team_specs.insert(spec_id.clone(), spec);
        Ok(applied(
            result,
            None,
            Some(project_id),
            "team_spec_created",
            json!({ "spec_id": spec_id }),
        ))
    }

    #[allow(clippy::too_many_arguments)] // The command fields are the authenticated enrollment record.
    fn enroll_actor(
        &mut self,
        actor: &Actor,
        spec_id: String,
        actor_id: String,
        role: Role,
        credential_digest: String,
        run_generation: u64,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        if role == Role::Owner {
            return Err(Error::new(
                "invalid_params",
                "owner is not a team role enrollment",
            ));
        }
        validate_id("actor_id", &actor_id)?;
        validate_digest("credential_digest", &credential_digest)?;
        if run_generation == 0 {
            return Err(Error::new(
                "invalid_params",
                "run_generation must be greater than zero",
            ));
        }
        if self.actors.contains_key(&actor_id)
            || self
                .team_specs
                .values()
                .flat_map(|spec| spec.enrollments.values())
                .any(|enrollment| enrollment.actor_id == actor_id)
        {
            return Err(Error::new("already_exists", "actor id is already enrolled"));
        }
        if self.credentials.contains_key(&credential_digest)
            || self
                .team_specs
                .values()
                .flat_map(|spec| spec.enrollments.values())
                .any(|enrollment| enrollment.credential_digest == credential_digest)
        {
            return Err(Error::new(
                "already_exists",
                "credential digest is already enrolled",
            ));
        }
        let spec = self
            .team_specs
            .get_mut(&spec_id)
            .ok_or_else(|| Error::new("not_found", "team spec not found"))?;
        if !matches!(spec.status, TeamSpecStatus::Draft) {
            return Err(Error::new(
                "invalid_state",
                "launched team spec cannot be edited",
            ));
        }
        if spec.enrollments.contains_key(&role) {
            return Err(Error::new(
                "already_exists",
                format!("{role:?} is already enrolled"),
            ));
        }
        spec.enrollments.insert(
            role,
            Enrollment {
                actor_id: actor_id.clone(),
                credential_digest,
                run_generation,
            },
        );
        spec.revision = revision;
        let project_id = spec.project_id.clone();
        let result = team_spec_view(spec);
        Ok(applied(
            result,
            None,
            Some(project_id),
            "team_actor_enrolled",
            json!({ "spec_id": spec_id, "actor_id": actor_id, "role": role }),
        ))
    }

    fn launch_team(
        &mut self,
        actor: &Actor,
        spec_id: String,
        team_id: String,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        validate_id("team_id", &team_id)?;
        if !self.teams.is_empty() {
            return Err(Error::new(
                "phase1_limit",
                "Phase 1 supports one launched team",
            ));
        }
        if self.teams.contains_key(&team_id) {
            return Err(Error::new("already_exists", "team already exists"));
        }
        let spec = self
            .team_specs
            .get(&spec_id)
            .cloned()
            .ok_or_else(|| Error::new("not_found", "team spec not found"))?;
        if !matches!(spec.status, TeamSpecStatus::Draft) {
            return Err(Error::new("invalid_state", "team spec is already launched"));
        }
        let lead = required_enrollment(&spec, Role::Lead)?;
        let developer = required_enrollment(&spec, Role::Developer)?;
        let qa = required_enrollment(&spec, Role::Qa)?;
        if spec.enrollments.len() != 3 {
            return Err(Error::new(
                "invalid_state",
                "team spec must contain exactly Lead, Developer and QA",
            ));
        }

        let team = Team {
            id: team_id.clone(),
            project_id: spec.project_id.clone(),
            spec_id: spec.id.clone(),
            name: spec.name.clone(),
            lead_id: lead.actor_id.clone(),
            developer_id: developer.actor_id.clone(),
            qa_id: qa.actor_id.clone(),
            revision,
        };
        for (role, enrollment) in [
            (Role::Lead, lead),
            (Role::Developer, developer),
            (Role::Qa, qa),
        ] {
            let enrolled_actor = Actor {
                id: enrollment.actor_id.clone(),
                role,
                team_id: Some(team_id.clone()),
                run_generation: Some(enrollment.run_generation),
                auth_digest: enrollment.credential_digest.clone(),
            };
            self.credentials.insert(
                enrollment.credential_digest.clone(),
                Credential {
                    actor_id: enrolled_actor.id.clone(),
                    run_generation: enrolled_actor.run_generation,
                    active: true,
                },
            );
            self.actors
                .insert(enrolled_actor.id.clone(), enrolled_actor);
        }
        self.teams.insert(team_id.clone(), team.clone());
        let stored_spec = self
            .team_specs
            .get_mut(&spec_id)
            .ok_or_else(|| Error::new("corrupt_state", "team spec disappeared"))?;
        stored_spec.status = TeamSpecStatus::Launched {
            team_id: team_id.clone(),
        };
        stored_spec.revision = revision;
        Ok(applied(
            serialize_value(&team)?,
            Some(team_id.clone()),
            Some(team.project_id.clone()),
            "team_launched",
            json!({
                "team_id": team_id,
                "spec_id": spec_id,
                "roles": {
                    "lead": team.lead_id,
                    "developer": team.developer_id,
                    "qa": team.qa_id,
                }
            }),
        ))
    }
}

impl State {
    #[allow(clippy::too_many_arguments)] // The command atomically creates the task, message and dispatch intent.
    fn create_task(
        &mut self,
        actor: &Actor,
        team_id: String,
        task_id: String,
        title: String,
        description: String,
        developer_id: String,
        dependencies: Vec<String>,
        assignment_message_id: String,
        dispatch: DispatchIntent,
        revision: u64,
    ) -> Result<Applied> {
        let team = self
            .require_team_actor(actor, &team_id, Role::Lead)?
            .clone();
        validate_id("task_id", &task_id)?;
        validate_id("assignment_message_id", &assignment_message_id)?;
        validate_text("task title", &title, MAX_TITLE_BYTES, false)?;
        validate_text("task description", &description, MAX_MESSAGE_BYTES, false)?;
        if developer_id != team.developer_id {
            return Err(Error::new(
                "invalid_reference",
                "task owner must be the team's Developer",
            ));
        }
        if self.tasks.contains_key(&task_id) {
            return Err(Error::new("already_exists", "task already exists"));
        }
        if self.messages.contains_key(&assignment_message_id) {
            return Err(Error::new("already_exists", "message already exists"));
        }
        let dependencies = self.validated_dependencies(&team_id, &task_id, dependencies)?;
        self.ensure_dispatch_intent_available(&dispatch)?;
        self.ensure_target_run(&developer_id, dispatch.run_generation)?;

        let task = Task {
            id: task_id.clone(),
            team_id: team_id.clone(),
            title,
            description: description.clone(),
            developer_id: developer_id.clone(),
            dependencies,
            status: TaskStatus::Assigned,
            revision,
        };
        let message = Message {
            id: assignment_message_id.clone(),
            team_id: team_id.clone(),
            sender_id: actor.id.clone(),
            task_id: Some(task_id.clone()),
            kind: MessageKind::TaskAssignment {
                task_id: task_id.clone(),
            },
            body: description,
            revision,
        };
        let attempt = self.new_dispatch_attempt(
            actor,
            &team_id,
            &task_id,
            &assignment_message_id,
            &developer_id,
            dispatch,
            1,
            revision,
        )?;
        self.tasks.insert(task_id.clone(), task.clone());
        self.messages.insert(assignment_message_id.clone(), message);
        self.dispatches.insert(attempt.id.clone(), attempt.clone());
        Ok(applied(
            serialize_value(&task)?,
            Some(team_id.clone()),
            Some(team.project_id),
            "task_created",
            json!({
                "team_id": team_id,
                "task_id": task_id,
                "developer_id": developer_id,
                "message_id": assignment_message_id,
                "dispatch_attempt_id": attempt.id,
            }),
        ))
    }

    fn set_task_dependencies(
        &mut self,
        actor: &Actor,
        team_id: String,
        task_id: String,
        dependencies: Vec<String>,
        revision: u64,
    ) -> Result<Applied> {
        let team = self
            .require_team_actor(actor, &team_id, Role::Lead)?
            .clone();
        let current = self
            .tasks
            .get(&task_id)
            .ok_or_else(|| Error::new("not_found", "task not found"))?;
        if current.team_id != team_id {
            return Err(Error::new(
                "invalid_reference",
                "task belongs to another team",
            ));
        }
        if !matches!(current.status, TaskStatus::Assigned) {
            return Err(Error::new(
                "invalid_state",
                "dependencies can change only before Developer acceptance",
            ));
        }
        let dependencies = self.validated_dependencies(&team_id, &task_id, dependencies)?;
        let previous = self
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| Error::new("corrupt_state", "task disappeared"))?;
        previous.dependencies = dependencies;
        previous.revision = revision;
        if self.task_graph_has_cycle(&team_id) {
            return Err(Error::new(
                "dependency_cycle",
                "task dependencies contain a cycle",
            ));
        }
        let previous = self
            .tasks
            .get(&task_id)
            .ok_or_else(|| Error::new("corrupt_state", "task disappeared"))?;
        let result = serialize_value(previous)?;
        Ok(applied(
            result,
            Some(team_id.clone()),
            Some(team.project_id),
            "task_dependencies_changed",
            json!({ "team_id": team_id, "task_id": task_id }),
        ))
    }

    fn accept_task(
        &mut self,
        actor: &Actor,
        team_id: String,
        task_id: String,
        acceptance_id: String,
        note: String,
        revision: u64,
    ) -> Result<Applied> {
        let team = self
            .require_team_actor(actor, &team_id, Role::Developer)?
            .clone();
        validate_id("acceptance_id", &acceptance_id)?;
        validate_text("acceptance note", &note, MAX_MESSAGE_BYTES, false)?;
        if self.acceptances.contains_key(&acceptance_id) {
            return Err(Error::new("already_exists", "acceptance already exists"));
        }
        let task = self
            .tasks
            .get(&task_id)
            .ok_or_else(|| Error::new("not_found", "task not found"))?;
        self.ensure_task_team_and_owner(task, &team_id, actor)?;
        if !matches!(task.status, TaskStatus::Assigned) {
            return Err(Error::new(
                "invalid_state",
                "task is not awaiting Developer acceptance",
            ));
        }
        let blocked: Vec<&str> = task
            .dependencies
            .iter()
            .filter_map(|dependency_id| {
                self.tasks
                    .get(dependency_id)
                    .filter(|dependency| !dependency.status.is_finalized())
                    .map(|_| dependency_id.as_str())
            })
            .collect();
        if !blocked.is_empty() {
            return Err(Error::new(
                "dependency_blocked",
                format!(
                    "task dependencies are not finalized: {}",
                    blocked.join(", ")
                ),
            ));
        }
        let acceptance = DeveloperAcceptance {
            id: acceptance_id.clone(),
            team_id: team_id.clone(),
            task_id: task_id.clone(),
            developer_id: actor.id.clone(),
            note,
            revision,
        };
        self.acceptances.insert(acceptance_id.clone(), acceptance);
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| Error::new("corrupt_state", "task disappeared"))?;
        task.status = TaskStatus::InProgress {
            acceptance_id: acceptance_id.clone(),
        };
        task.revision = revision;
        Ok(applied(
            serialize_value(task)?,
            Some(team_id.clone()),
            Some(team.project_id),
            "task_accepted",
            json!({
                "team_id": team_id,
                "task_id": task_id,
                "acceptance_id": acceptance_id,
            }),
        ))
    }

    #[allow(clippy::too_many_arguments)] // The handoff and its QA dispatch must commit together.
    fn submit_handoff(
        &mut self,
        actor: &Actor,
        team_id: String,
        task_id: String,
        handoff_id: String,
        message_id: String,
        summary: String,
        evidence: Vec<Evidence>,
        dispatch: DispatchIntent,
        revision: u64,
    ) -> Result<Applied> {
        let team = self
            .require_team_actor(actor, &team_id, Role::Developer)?
            .clone();
        validate_id("handoff_id", &handoff_id)?;
        validate_id("message_id", &message_id)?;
        validate_text("handoff summary", &summary, MAX_MESSAGE_BYTES, false)?;
        validate_evidence(&evidence, true)?;
        if self.handoffs.contains_key(&handoff_id) {
            return Err(Error::new("already_exists", "handoff already exists"));
        }
        if self.messages.contains_key(&message_id) {
            return Err(Error::new("already_exists", "message already exists"));
        }
        self.ensure_dispatch_intent_available(&dispatch)?;
        self.ensure_target_run(&team.qa_id, dispatch.run_generation)?;
        let task = self
            .tasks
            .get(&task_id)
            .ok_or_else(|| Error::new("not_found", "task not found"))?;
        self.ensure_task_team_and_owner(task, &team_id, actor)?;
        if !matches!(
            task.status,
            TaskStatus::InProgress { .. } | TaskStatus::ReworkRequested { .. }
        ) {
            return Err(Error::new(
                "invalid_state",
                "task is not ready for a Developer handoff",
            ));
        }
        let handoff = Handoff {
            id: handoff_id.clone(),
            team_id: team_id.clone(),
            task_id: task_id.clone(),
            developer_id: actor.id.clone(),
            summary: summary.clone(),
            evidence,
            revision,
        };
        let message = Message {
            id: message_id.clone(),
            team_id: team_id.clone(),
            sender_id: actor.id.clone(),
            task_id: Some(task_id.clone()),
            kind: MessageKind::Handoff {
                task_id: task_id.clone(),
                handoff_id: handoff_id.clone(),
            },
            body: summary,
            revision,
        };
        let attempt = self.new_dispatch_attempt(
            actor,
            &team_id,
            &task_id,
            &message_id,
            &team.qa_id,
            dispatch,
            1,
            revision,
        )?;
        self.handoffs.insert(handoff_id.clone(), handoff);
        self.messages.insert(message_id.clone(), message);
        self.dispatches.insert(attempt.id.clone(), attempt.clone());
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| Error::new("corrupt_state", "task disappeared"))?;
        task.status = TaskStatus::AwaitingQa {
            handoff_id: handoff_id.clone(),
        };
        task.revision = revision;
        Ok(applied(
            serialize_value(task)?,
            Some(team_id.clone()),
            Some(team.project_id),
            "handoff_submitted",
            json!({
                "team_id": team_id,
                "task_id": task_id,
                "handoff_id": handoff_id,
                "message_id": message_id,
                "dispatch_attempt_id": attempt.id,
            }),
        ))
    }
}

impl State {
    #[allow(clippy::too_many_arguments)] // The review, chat record and optional rework dispatch are one command.
    fn review_handoff(
        &mut self,
        actor: &Actor,
        team_id: String,
        task_id: String,
        handoff_id: String,
        decision_id: String,
        message_id: String,
        verdict: QaVerdict,
        rationale: String,
        evidence: Vec<Evidence>,
        rework_dispatch: Option<DispatchIntent>,
        revision: u64,
    ) -> Result<Applied> {
        let team = self.require_team_actor(actor, &team_id, Role::Qa)?.clone();
        validate_id("decision_id", &decision_id)?;
        validate_id("message_id", &message_id)?;
        validate_text("QA rationale", &rationale, MAX_MESSAGE_BYTES, false)?;
        validate_evidence(&evidence, false)?;
        if self.decisions.contains_key(&decision_id) {
            return Err(Error::new("already_exists", "decision already exists"));
        }
        if self.messages.contains_key(&message_id) {
            return Err(Error::new("already_exists", "message already exists"));
        }
        let task = self
            .tasks
            .get(&task_id)
            .ok_or_else(|| Error::new("not_found", "task not found"))?;
        if task.team_id != team_id {
            return Err(Error::new(
                "invalid_reference",
                "task belongs to another team",
            ));
        }
        match &task.status {
            TaskStatus::AwaitingQa {
                handoff_id: latest_handoff,
            } if latest_handoff == &handoff_id => {}
            TaskStatus::AwaitingQa { .. } => {
                return Err(Error::new(
                    "stale_handoff",
                    "QA decision does not bind the latest handoff",
                ));
            }
            _ => {
                return Err(Error::new(
                    "invalid_state",
                    "task is not awaiting QA review",
                ));
            }
        }
        let handoff = self
            .handoffs
            .get(&handoff_id)
            .ok_or_else(|| Error::new("not_found", "handoff not found"))?;
        if handoff.task_id != task_id || handoff.team_id != team_id {
            return Err(Error::new(
                "invalid_reference",
                "handoff does not belong to the task",
            ));
        }

        let rework_attempt = match (verdict, rework_dispatch) {
            (QaVerdict::Accept, None) => None,
            (QaVerdict::Accept, Some(_)) => {
                return Err(Error::new(
                    "invalid_params",
                    "accepted review cannot queue a rework dispatch",
                ));
            }
            (QaVerdict::Rework, None) => {
                return Err(Error::new(
                    "invalid_params",
                    "rework review requires a Developer dispatch intent",
                ));
            }
            (QaVerdict::Rework, Some(dispatch)) => {
                self.ensure_dispatch_intent_available(&dispatch)?;
                self.ensure_target_run(&team.developer_id, dispatch.run_generation)?;
                Some(self.new_dispatch_attempt(
                    actor,
                    &team_id,
                    &task_id,
                    &message_id,
                    &team.developer_id,
                    dispatch,
                    1,
                    revision,
                )?)
            }
        };

        let decision = Decision {
            id: decision_id.clone(),
            team_id: team_id.clone(),
            task_id: task_id.clone(),
            actor_id: actor.id.clone(),
            kind: DecisionKind::QaReview {
                handoff_id: handoff_id.clone(),
                verdict,
            },
            body: rationale.clone(),
            evidence,
            revision,
        };
        let message = Message {
            id: message_id.clone(),
            team_id: team_id.clone(),
            sender_id: actor.id.clone(),
            task_id: Some(task_id.clone()),
            kind: MessageKind::QaDecision {
                task_id: task_id.clone(),
                decision_id: decision_id.clone(),
            },
            body: rationale,
            revision,
        };
        self.decisions.insert(decision_id.clone(), decision);
        self.messages.insert(message_id.clone(), message);
        if let Some(attempt) = &rework_attempt {
            self.dispatches.insert(attempt.id.clone(), attempt.clone());
        }
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| Error::new("corrupt_state", "task disappeared"))?;
        task.status = match verdict {
            QaVerdict::Accept => TaskStatus::QaAccepted {
                handoff_id: handoff_id.clone(),
                decision_id: decision_id.clone(),
            },
            QaVerdict::Rework => TaskStatus::ReworkRequested {
                handoff_id: handoff_id.clone(),
                decision_id: decision_id.clone(),
            },
        };
        task.revision = revision;
        Ok(applied(
            serialize_value(task)?,
            Some(team_id.clone()),
            Some(team.project_id),
            "qa_review_recorded",
            json!({
                "team_id": team_id,
                "task_id": task_id,
                "handoff_id": handoff_id,
                "decision_id": decision_id,
                "verdict": verdict,
                "message_id": message_id,
                "dispatch_attempt_id": rework_attempt.map(|attempt| attempt.id),
            }),
        ))
    }

    #[allow(clippy::too_many_arguments)] // Outcome evidence and its typed chat record commit together.
    fn finalize_task(
        &mut self,
        actor: &Actor,
        team_id: String,
        task_id: String,
        decision_id: String,
        message_id: String,
        outcome: String,
        evidence: Vec<Evidence>,
        revision: u64,
    ) -> Result<Applied> {
        let team = self
            .require_team_actor(actor, &team_id, Role::Lead)?
            .clone();
        validate_id("decision_id", &decision_id)?;
        validate_id("message_id", &message_id)?;
        validate_text("outcome", &outcome, MAX_MESSAGE_BYTES, false)?;
        validate_evidence(&evidence, true)?;
        if self.decisions.contains_key(&decision_id) {
            return Err(Error::new("already_exists", "decision already exists"));
        }
        if self.messages.contains_key(&message_id) {
            return Err(Error::new("already_exists", "message already exists"));
        }
        let task = self
            .tasks
            .get(&task_id)
            .ok_or_else(|| Error::new("not_found", "task not found"))?;
        if task.team_id != team_id {
            return Err(Error::new(
                "invalid_reference",
                "task belongs to another team",
            ));
        }
        let (handoff_id, qa_decision_id) = match &task.status {
            TaskStatus::QaAccepted {
                handoff_id,
                decision_id,
            } => (handoff_id.clone(), decision_id.clone()),
            _ => {
                return Err(Error::new(
                    "invalid_state",
                    "Lead can finalize only the latest QA-accepted handoff",
                ));
            }
        };
        let decision = Decision {
            id: decision_id.clone(),
            team_id: team_id.clone(),
            task_id: task_id.clone(),
            actor_id: actor.id.clone(),
            kind: DecisionKind::Outcome {
                qa_decision_id: qa_decision_id.clone(),
            },
            body: outcome.clone(),
            evidence,
            revision,
        };
        let message = Message {
            id: message_id.clone(),
            team_id: team_id.clone(),
            sender_id: actor.id.clone(),
            task_id: Some(task_id.clone()),
            kind: MessageKind::Outcome {
                task_id: task_id.clone(),
                decision_id: decision_id.clone(),
            },
            body: outcome,
            revision,
        };
        self.decisions.insert(decision_id.clone(), decision);
        self.messages.insert(message_id.clone(), message);
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| Error::new("corrupt_state", "task disappeared"))?;
        task.status = TaskStatus::Finalized {
            handoff_id,
            qa_decision_id,
            outcome_decision_id: decision_id.clone(),
        };
        task.revision = revision;
        Ok(applied(
            serialize_value(task)?,
            Some(team_id.clone()),
            Some(team.project_id),
            "task_finalized",
            json!({
                "team_id": team_id,
                "task_id": task_id,
                "decision_id": decision_id,
                "message_id": message_id,
            }),
        ))
    }

    #[allow(clippy::too_many_arguments)] // Cancellation decision, message and dispatch tombstones are atomic.
    fn cancel_task(
        &mut self,
        actor: &Actor,
        team_id: String,
        task_id: String,
        decision_id: String,
        message_id: String,
        reason: String,
        revision: u64,
    ) -> Result<Applied> {
        let team = self
            .require_team_actor(actor, &team_id, Role::Lead)?
            .clone();
        validate_id("decision_id", &decision_id)?;
        validate_id("message_id", &message_id)?;
        validate_text("cancellation reason", &reason, MAX_MESSAGE_BYTES, false)?;
        if self.decisions.contains_key(&decision_id) {
            return Err(Error::new("already_exists", "decision already exists"));
        }
        if self.messages.contains_key(&message_id) {
            return Err(Error::new("already_exists", "message already exists"));
        }
        let task = self
            .tasks
            .get(&task_id)
            .ok_or_else(|| Error::new("not_found", "task not found"))?;
        if task.team_id != team_id {
            return Err(Error::new(
                "invalid_reference",
                "task belongs to another team",
            ));
        }
        if task.status.is_terminal() {
            return Err(Error::new("invalid_state", "task is already terminal"));
        }
        let decision = Decision {
            id: decision_id.clone(),
            team_id: team_id.clone(),
            task_id: task_id.clone(),
            actor_id: actor.id.clone(),
            kind: DecisionKind::Cancellation,
            body: reason.clone(),
            evidence: Vec::new(),
            revision,
        };
        let message = Message {
            id: message_id.clone(),
            team_id: team_id.clone(),
            sender_id: actor.id.clone(),
            task_id: Some(task_id.clone()),
            kind: MessageKind::Cancellation {
                task_id: task_id.clone(),
                decision_id: decision_id.clone(),
            },
            body: reason,
            revision,
        };
        self.decisions.insert(decision_id.clone(), decision);
        self.messages.insert(message_id.clone(), message);
        for attempt in self
            .dispatches
            .values_mut()
            .filter(|attempt| attempt.task_id == task_id && !attempt.state.is_terminal())
        {
            attempt.state = DispatchState::Cancelled;
            attempt.revision = revision;
        }
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| Error::new("corrupt_state", "task disappeared"))?;
        task.status = TaskStatus::Cancelled {
            decision_id: decision_id.clone(),
        };
        task.revision = revision;
        Ok(applied(
            serialize_value(task)?,
            Some(team_id.clone()),
            Some(team.project_id),
            "task_cancelled",
            json!({
                "team_id": team_id,
                "task_id": task_id,
                "decision_id": decision_id,
                "message_id": message_id,
            }),
        ))
    }
}

impl State {
    fn append_chat(
        &mut self,
        actor: &Actor,
        team_id: String,
        message_id: String,
        body: String,
        task_id: Option<String>,
        revision: u64,
    ) -> Result<Applied> {
        let team = self.require_team_member_or_owner(actor, &team_id)?.clone();
        validate_id("message_id", &message_id)?;
        validate_text("message body", &body, MAX_MESSAGE_BYTES, false)?;
        if self.messages.contains_key(&message_id) {
            return Err(Error::new("already_exists", "message already exists"));
        }
        if let Some(task_id) = task_id.as_deref() {
            let task = self
                .tasks
                .get(task_id)
                .ok_or_else(|| Error::new("not_found", "message task not found"))?;
            if task.team_id != team_id {
                return Err(Error::new(
                    "invalid_reference",
                    "message task belongs to another team",
                ));
            }
        }
        let message = Message {
            id: message_id.clone(),
            team_id: team_id.clone(),
            sender_id: actor.id.clone(),
            task_id,
            kind: MessageKind::Text,
            body,
            revision,
        };
        self.messages.insert(message_id.clone(), message.clone());
        Ok(applied(
            serialize_value(&message)?,
            Some(team_id.clone()),
            Some(team.project_id),
            "chat_message_appended",
            json!({ "team_id": team_id, "message_id": message_id }),
        ))
    }

    #[allow(clippy::too_many_arguments)] // Knowledge content and provenance form one durable entry.
    fn put_knowledge(
        &mut self,
        actor: &Actor,
        team_id: String,
        knowledge_id: String,
        scope: KnowledgeScope,
        title: String,
        body: String,
        evidence: Vec<Evidence>,
        revision: u64,
    ) -> Result<Applied> {
        let team = self.require_team_member_or_owner(actor, &team_id)?.clone();
        if actor.role != Role::Owner && actor.role != Role::Lead {
            return Err(Error::new(
                "forbidden",
                "only Owner or Lead may record project knowledge",
            ));
        }
        validate_id("knowledge_id", &knowledge_id)?;
        validate_text("knowledge title", &title, MAX_TITLE_BYTES, false)?;
        validate_text("knowledge body", &body, MAX_MESSAGE_BYTES, false)?;
        validate_evidence(&evidence, false)?;
        if self.knowledge.contains_key(&knowledge_id) {
            return Err(Error::new(
                "already_exists",
                "knowledge entry already exists",
            ));
        }
        match &scope {
            KnowledgeScope::Project { project_id } => {
                if actor.role != Role::Owner {
                    return Err(Error::new(
                        "forbidden",
                        "only Owner may write project-scoped knowledge",
                    ));
                }
                if project_id != &team.project_id {
                    return Err(Error::new(
                        "invalid_reference",
                        "knowledge project does not own the team",
                    ));
                }
            }
            KnowledgeScope::Team {
                team_id: scope_team,
            } if scope_team == &team_id => {}
            KnowledgeScope::Team { .. } => {
                return Err(Error::new(
                    "invalid_reference",
                    "knowledge scope names another team",
                ));
            }
        }
        let entry = KnowledgeEntry {
            id: knowledge_id.clone(),
            team_id: team_id.clone(),
            actor_id: actor.id.clone(),
            scope,
            title,
            body,
            evidence,
            revision,
        };
        self.knowledge.insert(knowledge_id.clone(), entry.clone());
        Ok(applied(
            serialize_value(&entry)?,
            Some(team_id.clone()),
            Some(team.project_id),
            "knowledge_recorded",
            json!({ "team_id": team_id, "knowledge_id": knowledge_id }),
        ))
    }
}

impl State {
    #[allow(clippy::too_many_arguments)] // The receipt binds the exact attempt, message, run and content it reports on.
    fn record_dispatch_receipt(
        &mut self,
        actor: &Actor,
        team_id: String,
        attempt_id: String,
        task_id: String,
        message_id: String,
        run_generation: u64,
        content_digest: String,
        provider_session: Option<String>,
        receipt: DispatchReceiptKind,
        detail: Option<String>,
        evidence: Vec<Evidence>,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        let team = self.require_team(&team_id)?.clone();
        validate_evidence(&evidence, false)?;
        if let Some(session) = provider_session.as_deref() {
            validate_text("provider session", session, MAX_NAME_BYTES, false)?;
        }
        if let Some(detail) = detail.as_deref() {
            validate_text("receipt detail", detail, MAX_MESSAGE_BYTES, true)?;
        }
        let attempt = self
            .dispatches
            .get(&attempt_id)
            .ok_or_else(|| Error::new("not_found", "dispatch attempt not found"))?;
        if attempt.team_id != team_id
            || attempt.task_id != task_id
            || attempt.message_id != message_id
            || attempt.run_generation != run_generation
            || attempt.content_digest != content_digest
        {
            return Err(Error::new(
                "invalid_reference",
                "receipt does not bind the exact dispatch attempt",
            ));
        }
        self.ensure_target_run(&attempt.target_actor_id, attempt.run_generation)?;
        let session_matches = |current: &str| {
            provider_session
                .as_deref()
                .map_or(true, |reported| reported == current)
        };
        let required_session = || {
            provider_session
                .clone()
                .ok_or_else(|| Error::new("invalid_params", "receipt requires a provider session"))
        };
        let next = match (&attempt.state, receipt) {
            (DispatchState::Queued, DispatchReceiptKind::Attempted) => DispatchState::Attempted,
            (DispatchState::Attempted, DispatchReceiptKind::Submitted) => {
                DispatchState::Submitted {
                    provider_session: required_session()?,
                }
            }
            (
                DispatchState::Submitted {
                    provider_session: current,
                },
                DispatchReceiptKind::Acknowledged,
            ) if session_matches(current) => DispatchState::Acknowledged {
                provider_session: current.clone(),
            },
            (
                DispatchState::Submitted {
                    provider_session: current,
                }
                | DispatchState::Acknowledged {
                    provider_session: current,
                },
                DispatchReceiptKind::Accepted,
            ) if session_matches(current) => DispatchState::Accepted {
                provider_session: current.clone(),
            },
            (
                DispatchState::Submitted {
                    provider_session: current,
                }
                | DispatchState::Acknowledged {
                    provider_session: current,
                },
                DispatchReceiptKind::Rejected,
            ) if session_matches(current) => DispatchState::Rejected {
                provider_session: current.clone(),
                reason: detail.clone().unwrap_or_default(),
            },
            (
                DispatchState::Accepted {
                    provider_session: current,
                },
                DispatchReceiptKind::Completed,
            ) if session_matches(current) => {
                if evidence.is_empty() {
                    return Err(Error::new(
                        "invalid_params",
                        "completed receipt requires evidence",
                    ));
                }
                DispatchState::Completed {
                    provider_session: current.clone(),
                    evidence,
                }
            }
            (state, DispatchReceiptKind::Failed) if !state.is_terminal() => DispatchState::Failed {
                provider_session: match state {
                    DispatchState::Submitted { provider_session }
                    | DispatchState::Acknowledged { provider_session }
                    | DispatchState::Accepted { provider_session } => {
                        Some(provider_session.clone())
                    }
                    _ => None,
                },
                reason: detail.clone().unwrap_or_default(),
            },
            _ => {
                return Err(Error::new(
                    "invalid_state",
                    "receipt is not a valid transition for this dispatch attempt",
                ));
            }
        };
        let attempt = self
            .dispatches
            .get_mut(&attempt_id)
            .ok_or_else(|| Error::new("corrupt_state", "dispatch disappeared"))?;
        attempt.state = next;
        attempt.revision = revision;
        let result = serialize_value(attempt)?;
        Ok(applied(
            result,
            Some(team_id.clone()),
            Some(team.project_id),
            "dispatch_receipt_recorded",
            json!({
                "team_id": team_id,
                "attempt_id": attempt_id,
                "task_id": task_id,
                "receipt": receipt,
            }),
        ))
    }

    fn retry_dispatch(
        &mut self,
        actor: &Actor,
        team_id: String,
        previous_attempt_id: String,
        dispatch: DispatchIntent,
        revision: u64,
    ) -> Result<Applied> {
        let team = self
            .require_team_actor(actor, &team_id, Role::Lead)?
            .clone();
        let previous = self
            .dispatches
            .get(&previous_attempt_id)
            .cloned()
            .ok_or_else(|| Error::new("not_found", "dispatch attempt not found"))?;
        if previous.team_id != team_id {
            return Err(Error::new(
                "invalid_reference",
                "dispatch attempt belongs to another team",
            ));
        }
        if !previous.state.is_retryable() {
            return Err(Error::new(
                "invalid_state",
                "only a rejected, failed, cancelled or unknown dispatch can be retried",
            ));
        }
        if usize::from(previous.attempt_number) >= MAX_DISPATCH_ATTEMPTS {
            return Err(Error::new(
                "retry_exhausted",
                format!("dispatch already made {MAX_DISPATCH_ATTEMPTS} attempts"),
            ));
        }
        if self.dispatches.values().any(|other| {
            other.message_id == previous.message_id
                && other.attempt_number > previous.attempt_number
        }) {
            return Err(Error::new(
                "stale_dispatch",
                "a later attempt already exists for this message",
            ));
        }
        let task = self
            .tasks
            .get(&previous.task_id)
            .ok_or_else(|| Error::new("corrupt_state", "dispatch task disappeared"))?;
        if task.status.is_terminal() {
            return Err(Error::new("invalid_state", "task is already terminal"));
        }
        if dispatch.content_digest != previous.content_digest {
            return Err(Error::new(
                "invalid_params",
                "a retry must resend the same message content",
            ));
        }
        self.ensure_dispatch_intent_available(&dispatch)?;
        self.ensure_target_run(&previous.target_actor_id, dispatch.run_generation)?;
        let attempt = self.new_dispatch_attempt(
            actor,
            &team_id,
            &previous.task_id,
            &previous.message_id,
            &previous.target_actor_id,
            dispatch,
            previous.attempt_number + 1,
            revision,
        )?;
        self.dispatches.insert(attempt.id.clone(), attempt.clone());
        Ok(applied(
            serialize_value(&attempt)?,
            Some(team_id.clone()),
            Some(team.project_id),
            "dispatch_retried",
            json!({
                "team_id": team_id,
                "previous_attempt_id": previous_attempt_id,
                "attempt_id": attempt.id,
                "attempt_number": attempt.attempt_number,
            }),
        ))
    }

    fn require_project(&self, project_id: &str) -> Result<&Project> {
        self.projects
            .get(project_id)
            .ok_or_else(|| Error::new("not_found", "project not found"))
    }

    fn require_team(&self, team_id: &str) -> Result<&Team> {
        self.teams
            .get(team_id)
            .ok_or_else(|| Error::new("not_found", "team not found"))
    }

    fn require_team_actor(&self, actor: &Actor, team_id: &str, role: Role) -> Result<&Team> {
        require_role(actor, role)?;
        self.require_team_member_or_owner(actor, team_id)
    }

    fn require_team_member_or_owner(&self, actor: &Actor, team_id: &str) -> Result<&Team> {
        let team = self.require_team(team_id)?;
        if actor.role != Role::Owner && actor.team_id.as_deref() != Some(team_id) {
            return Err(Error::new(
                "forbidden",
                "actor does not belong to this team",
            ));
        }
        Ok(team)
    }

    fn ensure_task_team_and_owner(&self, task: &Task, team_id: &str, actor: &Actor) -> Result<()> {
        if task.team_id != team_id {
            return Err(Error::new(
                "invalid_reference",
                "task belongs to another team",
            ));
        }
        if task.developer_id != actor.id {
            return Err(Error::new(
                "forbidden",
                "task is owned by another Developer",
            ));
        }
        Ok(())
    }

    fn validated_dependencies(
        &self,
        team_id: &str,
        task_id: &str,
        dependencies: Vec<String>,
    ) -> Result<BTreeSet<String>> {
        if dependencies.len() > MAX_DEPENDENCIES {
            return Err(Error::new(
                "invalid_params",
                format!("a task has at most {MAX_DEPENDENCIES} dependencies"),
            ));
        }
        let mut validated = BTreeSet::new();
        for dependency_id in dependencies {
            if dependency_id == task_id {
                return Err(Error::new(
                    "dependency_cycle",
                    "a task cannot depend on itself",
                ));
            }
            let dependency = self
                .tasks
                .get(&dependency_id)
                .ok_or_else(|| Error::new("not_found", "dependency task not found"))?;
            if dependency.team_id != team_id {
                return Err(Error::new(
                    "invalid_reference",
                    "dependency belongs to another team",
                ));
            }
            if !validated.insert(dependency_id) {
                return Err(Error::new("invalid_params", "dependencies repeat a task"));
            }
        }
        Ok(validated)
    }

    fn task_graph_has_cycle(&self, team_id: &str) -> bool {
        // Kahn's algorithm: whatever cannot be peeled off lies on or behind a cycle.
        let mut pending: BTreeMap<&str, BTreeSet<&str>> = self
            .tasks
            .values()
            .filter(|task| task.team_id == team_id)
            .map(|task| {
                (
                    task.id.as_str(),
                    task.dependencies.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        loop {
            let ready: Vec<&str> = pending
                .iter()
                .filter(|(_, dependencies)| dependencies.is_empty())
                .map(|(id, _)| *id)
                .collect();
            if ready.is_empty() {
                return !pending.is_empty();
            }
            for id in ready {
                pending.remove(id);
                for dependencies in pending.values_mut() {
                    dependencies.remove(id);
                }
            }
        }
    }

    fn ensure_dispatch_intent_available(&self, dispatch: &DispatchIntent) -> Result<()> {
        validate_id("attempt_id", &dispatch.attempt_id)?;
        validate_digest("content_digest", &dispatch.content_digest)?;
        if dispatch.run_generation == 0 {
            return Err(Error::new(
                "invalid_params",
                "run_generation must be greater than zero",
            ));
        }
        if self.dispatches.contains_key(&dispatch.attempt_id) {
            return Err(Error::new(
                "already_exists",
                "dispatch attempt already exists",
            ));
        }
        Ok(())
    }

    fn ensure_target_run(&self, actor_id: &str, run_generation: u64) -> Result<()> {
        let target = self
            .actors
            .get(actor_id)
            .ok_or_else(|| Error::new("invalid_reference", "dispatch target is not enrolled"))?;
        if target.run_generation != Some(run_generation) {
            return Err(Error::new(
                "stale_run",
                "dispatch names a run generation the target no longer has",
            ));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // The attempt record is the dispatch's full provenance.
    fn new_dispatch_attempt(
        &self,
        actor: &Actor,
        team_id: &str,
        task_id: &str,
        message_id: &str,
        target_actor_id: &str,
        dispatch: DispatchIntent,
        attempt_number: u8,
        revision: u64,
    ) -> Result<DispatchAttempt> {
        Ok(DispatchAttempt {
            id: dispatch.attempt_id,
            team_id: team_id.to_owned(),
            task_id: task_id.to_owned(),
            message_id: message_id.to_owned(),
            requested_by: actor.id.clone(),
            target_actor_id: target_actor_id.to_owned(),
            run_generation: dispatch.run_generation,
            content_digest: dispatch.content_digest,
            attempt_number,
            state: DispatchState::Queued,
            revision,
        })
    }

    /// Whole-state invariants. Run on every candidate before it is committed or loaded.
    pub fn validate(&self) -> Result<()> {
        let corrupt = |message: &str| Error::new("corrupt_state", message.to_owned());
        if self.version != STATE_VERSION {
            return Err(corrupt("unsupported state version"));
        }
        let records = self.projects.len()
            + self.team_specs.len()
            + self.teams.len()
            + self.actors.len()
            + self.tasks.len()
            + self.acceptances.len()
            + self.handoffs.len()
            + self.decisions.len()
            + self.messages.len()
            + self.knowledge.len()
            + self.dispatches.len()
            + self.events.len();
        if records > MAX_RECORDS {
            return Err(Error::new(
                "capacity_exceeded",
                "Teams record limit reached",
            ));
        }
        if self.next_sequence != self.events.len() as u64 + 1 {
            return Err(corrupt("event sequence does not match the event log"));
        }
        let mut previous_revision = 0;
        for (index, event) in self.events.iter().enumerate() {
            if event.sequence != index as u64 + 1 {
                return Err(corrupt("event log has a gap"));
            }
            if event.revision < previous_revision || event.revision > self.revision {
                return Err(corrupt("event revision is out of order"));
            }
            previous_revision = event.revision;
        }
        match self.actors.get(OWNER_ID) {
            Some(owner) if owner.role == Role::Owner => {}
            _ => return Err(corrupt("owner actor is missing")),
        }
        for (digest, credential) in &self.credentials {
            let actor = self
                .actors
                .get(&credential.actor_id)
                .ok_or_else(|| corrupt("credential names an unknown actor"))?;
            if credential.active && &actor.auth_digest != digest {
                return Err(corrupt("active credential does not match its actor"));
            }
        }
        for actor in self.actors.values() {
            if let Some(team_id) = &actor.team_id {
                if !self.teams.contains_key(team_id) {
                    return Err(corrupt("actor names an unknown team"));
                }
            } else if actor.role != Role::Owner {
                return Err(corrupt("team actor has no team"));
            }
        }
        for team in self.teams.values() {
            if !self.projects.contains_key(&team.project_id) {
                return Err(corrupt("team names an unknown project"));
            }
            for (actor_id, role) in [
                (&team.lead_id, Role::Lead),
                (&team.developer_id, Role::Developer),
                (&team.qa_id, Role::Qa),
            ] {
                match self.actors.get(actor_id) {
                    Some(actor)
                        if actor.role == role && actor.team_id.as_deref() == Some(&team.id) => {}
                    _ => return Err(corrupt("team role is not held by its actor")),
                }
            }
        }
        for spec in self.team_specs.values() {
            if !self.projects.contains_key(&spec.project_id) {
                return Err(corrupt("team spec names an unknown project"));
            }
            if let TeamSpecStatus::Launched { team_id } = &spec.status {
                if !self.teams.contains_key(team_id) {
                    return Err(corrupt("launched spec names an unknown team"));
                }
            }
        }
        for task in self.tasks.values() {
            if !self.teams.contains_key(&task.team_id) {
                return Err(corrupt("task names an unknown team"));
            }
            for dependency in &task.dependencies {
                match self.tasks.get(dependency) {
                    Some(other) if other.team_id == task.team_id => {}
                    _ => return Err(corrupt("task dependency is missing or crosses teams")),
                }
            }
        }
        for team_id in self.teams.keys() {
            if self.task_graph_has_cycle(team_id) {
                return Err(Error::new(
                    "dependency_cycle",
                    "task dependencies contain a cycle",
                ));
            }
        }
        let known_team = |team_id: &str| self.teams.contains_key(team_id);
        let known_task = |team_id: &str, task_id: &str| {
            self.tasks
                .get(task_id)
                .is_some_and(|task| task.team_id == team_id)
        };
        if !self
            .acceptances
            .values()
            .all(|v| known_task(&v.team_id, &v.task_id))
            || !self
                .handoffs
                .values()
                .all(|v| known_task(&v.team_id, &v.task_id))
            || !self
                .decisions
                .values()
                .all(|v| known_task(&v.team_id, &v.task_id))
            || !self
                .dispatches
                .values()
                .all(|v| known_task(&v.team_id, &v.task_id))
            || !self.messages.values().all(|v| {
                known_team(&v.team_id)
                    && v.task_id
                        .as_deref()
                        .map_or(true, |task_id| known_task(&v.team_id, task_id))
            })
            || !self.knowledge.values().all(|v| known_team(&v.team_id))
        {
            return Err(corrupt("a record names an unknown team or task"));
        }
        for attempt in self.dispatches.values() {
            if !self.messages.contains_key(&attempt.message_id)
                || !self.actors.contains_key(&attempt.target_actor_id)
            {
                return Err(corrupt("dispatch names an unknown message or target"));
            }
        }
        for task in self.tasks.values() {
            let handoff_ok = |id: &String| self.handoffs.contains_key(id);
            let decision_ok = |id: &String| self.decisions.contains_key(id);
            let consistent = match &task.status {
                TaskStatus::Assigned => true,
                TaskStatus::InProgress { acceptance_id } => {
                    self.acceptances.contains_key(acceptance_id)
                }
                TaskStatus::AwaitingQa { handoff_id } => handoff_ok(handoff_id),
                TaskStatus::ReworkRequested {
                    handoff_id,
                    decision_id,
                }
                | TaskStatus::QaAccepted {
                    handoff_id,
                    decision_id,
                } => handoff_ok(handoff_id) && decision_ok(decision_id),
                TaskStatus::Finalized {
                    handoff_id,
                    qa_decision_id,
                    outcome_decision_id,
                } => {
                    handoff_ok(handoff_id)
                        && decision_ok(qa_decision_id)
                        && decision_ok(outcome_decision_id)
                }
                TaskStatus::Cancelled { decision_id } => decision_ok(decision_id),
            };
            if !consistent {
                return Err(corrupt("task status names a missing record"));
            }
        }
        Ok(())
    }
}

fn applied(
    result: Value,
    team_id: Option<String>,
    project_id: Option<String>,
    kind: &'static str,
    payload: Value,
) -> Applied {
    Applied {
        result,
        events: vec![PendingEvent {
            project_id,
            team_id,
            kind,
            payload,
        }],
    }
}

fn serialize_value<T: Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value)
        .map_err(|_| Error::new("corrupt_state", "a record could not be encoded"))
}

fn require_role(actor: &Actor, role: Role) -> Result<()> {
    if actor.role != role {
        return Err(Error::new(
            "forbidden",
            format!("this command requires the {role:?} role"),
        ));
    }
    Ok(())
}

fn required_enrollment(spec: &TeamSpec, role: Role) -> Result<&Enrollment> {
    spec.enrollments.get(&role).ok_or_else(|| {
        Error::new(
            "invalid_state",
            format!("team spec has no {role:?} enrolled"),
        )
    })
}

/// A spec as clients see it: who holds each role, never the credential digests.
fn team_spec_view(spec: &TeamSpec) -> Value {
    json!({
        "id": spec.id,
        "project_id": spec.project_id,
        "name": spec.name,
        "status": spec.status,
        "enrollments": spec
            .enrollments
            .iter()
            .map(|(role, enrollment)| {
                (
                    format!("{role:?}").to_lowercase(),
                    json!({
                        "actor_id": enrollment.actor_id,
                        "run_generation": enrollment.run_generation,
                    }),
                )
            })
            .collect::<BTreeMap<_, _>>(),
        "revision": spec.revision,
    })
}

fn actor_view(actor: &Actor) -> Value {
    json!({
        "id": actor.id,
        "role": actor.role,
        "team_id": actor.team_id,
        "run_generation": actor.run_generation,
    })
}

fn filtered_values<T>(map: &BTreeMap<String, T>, keep: impl Fn(&T) -> bool) -> Vec<&T> {
    map.values().filter(|value| keep(value)).collect()
}

fn validate_id(field: &str, value: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value.len() <= MAX_NAME_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'));
    if !valid {
        return Err(Error::new(
            "invalid_params",
            format!("{field} must be 1 to {MAX_NAME_BYTES} letters, digits, '.', '_', '-' or ':'"),
        ));
    }
    Ok(())
}

fn validate_text(field: &str, value: &str, max_bytes: usize, allow_empty: bool) -> Result<()> {
    if !allow_empty && value.trim().is_empty() {
        return Err(Error::new("invalid_params", format!("{field} is required")));
    }
    if value.len() > max_bytes {
        return Err(Error::new(
            "invalid_params",
            format!("{field} must be at most {max_bytes} bytes"),
        ));
    }
    if value
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\t'))
    {
        return Err(Error::new(
            "invalid_params",
            format!("{field} contains control characters"),
        ));
    }
    Ok(())
}

fn validate_digest(field: &str, value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(Error::new(
            "invalid_params",
            format!("{field} must be 64 lowercase hex characters"),
        ));
    }
    Ok(())
}

fn validate_evidence(evidence: &[Evidence], required: bool) -> Result<()> {
    if required && evidence.is_empty() {
        return Err(Error::new("invalid_params", "evidence is required"));
    }
    if evidence.len() > MAX_EVIDENCE_ITEMS {
        return Err(Error::new(
            "invalid_params",
            format!("at most {MAX_EVIDENCE_ITEMS} evidence items are allowed"),
        ));
    }
    for item in evidence {
        validate_text(
            "evidence reference",
            &item.reference,
            MAX_EVIDENCE_REFERENCE_BYTES,
            false,
        )?;
        if let Some(digest) = &item.digest {
            validate_digest("evidence digest", digest)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(seed: char) -> String {
        seed.to_string().repeat(64)
    }

    fn evidence() -> Vec<Evidence> {
        vec![Evidence {
            kind: EvidenceKind::Test,
            reference: "cargo test".into(),
            digest: None,
        }]
    }

    fn intent(attempt: &str, target_generation: u64) -> DispatchIntent {
        DispatchIntent {
            attempt_id: attempt.into(),
            run_generation: target_generation,
            content_digest: digest('d'),
        }
    }

    /// One launched team: owner, Lead `lead`, Developer `dev`, QA `qa`.
    fn launched() -> (State, Actor, Actor, Actor, Actor) {
        let mut state = State::new("stream".into(), digest('0'));
        let owner = state.authenticate(&digest('0')).unwrap();
        for command in [
            Command::CreateProject {
                project_id: "p".into(),
                name: "P".into(),
            },
            Command::CreateTeamSpec {
                spec_id: "s".into(),
                project_id: "p".into(),
                name: "T".into(),
            },
        ] {
            state.execute(&owner, command).unwrap();
        }
        for (id, role, seed) in [
            ("lead", Role::Lead, '1'),
            ("dev", Role::Developer, '2'),
            ("qa", Role::Qa, '3'),
        ] {
            state
                .execute(
                    &owner,
                    Command::EnrollActor {
                        spec_id: "s".into(),
                        actor_id: id.into(),
                        role,
                        credential_digest: digest(seed),
                        run_generation: 1,
                    },
                )
                .unwrap();
        }
        state
            .execute(
                &owner,
                Command::LaunchTeam {
                    spec_id: "s".into(),
                    team_id: "t".into(),
                },
            )
            .unwrap();
        let lead = state.authenticate(&digest('1')).unwrap();
        let dev = state.authenticate(&digest('2')).unwrap();
        let qa = state.authenticate(&digest('3')).unwrap();
        (state, owner, lead, dev, qa)
    }

    fn create_task(
        state: &mut State,
        lead: &Actor,
        id: &str,
        dependencies: &[&str],
    ) -> Result<Value> {
        state.execute(
            lead,
            Command::CreateTask {
                team_id: "t".into(),
                task_id: id.into(),
                title: id.into(),
                description: "do it".into(),
                developer_id: "dev".into(),
                dependencies: dependencies.iter().map(|d| d.to_string()).collect(),
                assignment_message_id: format!("m-{id}"),
                dispatch: intent(&format!("a-{id}"), 1),
            },
        )
    }

    #[test]
    fn task_runs_from_assignment_through_qa_to_finalization() {
        let (mut state, _owner, lead, dev, qa) = launched();
        create_task(&mut state, &lead, "task", &[]).unwrap();
        // Roles are enforced: QA cannot accept, and Lead cannot finalize before QA.
        assert_eq!(
            state
                .execute(
                    &qa,
                    Command::AcceptTask {
                        team_id: "t".into(),
                        task_id: "task".into(),
                        acceptance_id: "x".into(),
                        note: "n".into()
                    }
                )
                .unwrap_err()
                .code,
            "forbidden"
        );
        state
            .execute(
                &dev,
                Command::AcceptTask {
                    team_id: "t".into(),
                    task_id: "task".into(),
                    acceptance_id: "acc".into(),
                    note: "n".into(),
                },
            )
            .unwrap();
        state
            .execute(
                &dev,
                Command::SubmitHandoff {
                    team_id: "t".into(),
                    task_id: "task".into(),
                    handoff_id: "h".into(),
                    message_id: "m-h".into(),
                    summary: "done".into(),
                    evidence: evidence(),
                    dispatch: intent("a-h", 1),
                },
            )
            .unwrap();
        assert_eq!(
            state
                .execute(
                    &lead,
                    Command::FinalizeTask {
                        team_id: "t".into(),
                        task_id: "task".into(),
                        decision_id: "o".into(),
                        message_id: "m-o".into(),
                        outcome: "ok".into(),
                        evidence: evidence()
                    }
                )
                .unwrap_err()
                .code,
            "invalid_state"
        );
        state
            .execute(
                &qa,
                Command::ReviewHandoff {
                    team_id: "t".into(),
                    task_id: "task".into(),
                    handoff_id: "h".into(),
                    decision_id: "q".into(),
                    message_id: "m-q".into(),
                    verdict: QaVerdict::Accept,
                    rationale: "good".into(),
                    evidence: vec![],
                    rework_dispatch: None,
                },
            )
            .unwrap();
        state
            .execute(
                &lead,
                Command::FinalizeTask {
                    team_id: "t".into(),
                    task_id: "task".into(),
                    decision_id: "o".into(),
                    message_id: "m-o".into(),
                    outcome: "ok".into(),
                    evidence: evidence(),
                },
            )
            .unwrap();
        assert!(state.tasks["task"].status.is_finalized());
        state.validate().unwrap();
    }

    #[test]
    fn dependencies_block_acceptance_and_cycles_are_rejected() {
        let (mut state, _owner, lead, dev, _qa) = launched();
        create_task(&mut state, &lead, "a", &[]).unwrap();
        create_task(&mut state, &lead, "b", &["a"]).unwrap();
        assert_eq!(
            create_task(&mut state, &lead, "c", &["c"])
                .unwrap_err()
                .code,
            "dependency_cycle"
        );
        assert_eq!(
            state
                .execute(
                    &lead,
                    Command::SetTaskDependencies {
                        team_id: "t".into(),
                        task_id: "a".into(),
                        dependencies: vec!["b".into()]
                    }
                )
                .unwrap_err()
                .code,
            "dependency_cycle"
        );
        assert_eq!(
            state
                .execute(
                    &dev,
                    Command::AcceptTask {
                        team_id: "t".into(),
                        task_id: "b".into(),
                        acceptance_id: "x".into(),
                        note: "n".into()
                    }
                )
                .unwrap_err()
                .code,
            "dependency_blocked"
        );
        state.validate().unwrap();
    }

    #[test]
    fn dispatch_receipts_follow_the_lifecycle_and_retries_are_bounded() {
        let (mut state, owner, lead, _dev, _qa) = launched();
        create_task(&mut state, &lead, "task", &[]).unwrap();
        let receipt = |kind, session: Option<&str>, evidence| Command::RecordDispatchReceipt {
            team_id: "t".into(),
            attempt_id: "a-task".into(),
            task_id: "task".into(),
            message_id: "m-task".into(),
            run_generation: 1,
            content_digest: digest('d'),
            provider_session: session.map(Into::into),
            receipt: kind,
            detail: Some("why".into()),
            evidence,
        };
        // Skipping a step is refused; so is a non-owner reporting.
        assert_eq!(
            state
                .execute(
                    &owner,
                    receipt(DispatchReceiptKind::Accepted, Some("s"), vec![])
                )
                .unwrap_err()
                .code,
            "invalid_state"
        );
        assert_eq!(
            state
                .execute(&lead, receipt(DispatchReceiptKind::Attempted, None, vec![]))
                .unwrap_err()
                .code,
            "forbidden"
        );
        for (kind, evidence) in [
            (DispatchReceiptKind::Attempted, vec![]),
            (DispatchReceiptKind::Submitted, vec![]),
            (DispatchReceiptKind::Rejected, vec![]),
        ] {
            state
                .execute(&owner, receipt(kind, Some("s"), evidence))
                .unwrap();
        }
        for attempt in 2..=MAX_DISPATCH_ATTEMPTS {
            let previous = if attempt == 2 {
                "a-task".to_owned()
            } else {
                format!("r{}", attempt - 1)
            };
            let id = format!("r{attempt}");
            state
                .execute(
                    &lead,
                    Command::RetryDispatch {
                        team_id: "t".into(),
                        previous_attempt_id: previous.clone(),
                        dispatch: intent(&id, 1),
                    },
                )
                .unwrap();
            state.dispatches.get_mut(&id).unwrap().state = DispatchState::Failed {
                provider_session: None,
                reason: "x".into(),
            };
        }
        let last = format!("r{MAX_DISPATCH_ATTEMPTS}");
        assert_eq!(
            state
                .execute(
                    &lead,
                    Command::RetryDispatch {
                        team_id: "t".into(),
                        previous_attempt_id: last,
                        dispatch: intent("r9", 1)
                    }
                )
                .unwrap_err()
                .code,
            "retry_exhausted"
        );
        state.validate().unwrap();
    }

    #[test]
    fn recovery_marks_in_flight_dispatches_unknown_and_viewers_see_only_their_team() {
        let (mut state, owner, lead, _dev, _qa) = launched();
        create_task(&mut state, &lead, "task", &[]).unwrap();
        state.dispatches.get_mut("a-task").unwrap().state = DispatchState::Attempted;
        assert!(state.recover().unwrap());
        assert!(matches!(
            state.dispatches["a-task"].state,
            DispatchState::Unknown { .. }
        ));
        assert!(!state.recover().unwrap());
        assert_eq!(
            state.snapshot(&lead, Some("other")).unwrap_err().code,
            "forbidden"
        );
        let snapshot = state.snapshot(&owner, None).unwrap();
        assert!(
            !snapshot.to_string().contains(&digest('2')),
            "credential digests must not leave the server"
        );
    }
}

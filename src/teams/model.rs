use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use sha2::{Digest, Sha256};

mod bindings;
mod governance;
mod memory;
mod oversight;
mod planning;
mod portfolio;

use super::{Error, Result};
pub use bindings::PaneBinding;
use governance::RecruitmentRequest;
pub use governance::{Approval, Authority, ProtectedAction, Recruitment};
use memory::{ContextNote, KnowledgeKind, KnowledgeStatus, NoteKind, SourceKind, SourceRef};
pub use memory::{DEFAULT_BUDGET, MAX_BUDGET};
use oversight::{AgentRunState, AgentState, Dispute, TeamLimits, TeamUsage, UsageBasis};
pub use oversight::{
    DEFAULT_STUCK_SECONDS, MAX_ACTIVE_TASKS, MAX_OPEN_DISPUTES, MAX_OPEN_TASKS, MAX_POSITIONS,
    MAX_QUEUED_DISPATCHES,
};
use planning::{Metering, ModelDecl, Plan, PlanEdit, Preference, Provider, RoleAssignment};
pub use planning::{PlanStatus, PlanTask as PlanTaskView, MAX_DEPENDENCY_DEPTH, MAX_PLAN_TASKS};
use portfolio::Submission;
pub use portfolio::{
    ChangeNotice, Conflict, Integration, SharedSurface, TeamStatus, WorktreeClaim,
};

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
pub const RETAIN_EVENTS_MAX: usize = 4_000;
pub const RETAIN_EVENTS_KEEP: usize = 2_000;
pub const MAX_RECEIPTS: usize = 4_000;
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
    /// A recruited, temporary specialist; never enrolled through a team spec.
    Specialist,
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
    #[serde(default)]
    pub status: TeamStatus,
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
    /// Server time (unix seconds) of the last change; 0 for tasks written before this was tracked.
    #[serde(default)]
    pub updated_at: u64,
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

/// Who a chat message is for. `Team` is the shared channel; the others are scoped: only the
/// sender, the addressee and the Owner can read them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "to", rename_all = "snake_case", deny_unknown_fields)]
pub enum Audience {
    #[default]
    Team,
    Role {
        role: Role,
    },
    Actor {
        actor_id: String,
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
    #[serde(default)]
    pub audience: Audience,
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
    #[serde(default)]
    pub kind: KnowledgeKind,
    #[serde(default)]
    pub status: KnowledgeStatus,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
    /// The authority class the command ran under; `None` for system events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<Authority>,
    /// Server clock, unix seconds.
    #[serde(default)]
    pub at: u64,
    /// Set for events about scoped messages: who besides the Owner may see them at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restricted_to: Option<Vec<String>>,
    /// Hash chain: each event commits to the one before it, so edits to history are detectable.
    #[serde(default)]
    pub prev_hash: String,
    #[serde(default)]
    pub hash: String,
}

impl Event {
    fn compute_hash(&self) -> String {
        let mut fields = vec![
            json!(self.prev_hash),
            json!(self.sequence),
            json!(self.revision),
            json!(self.actor_id),
            json!(self.project_id),
            json!(self.team_id),
            json!(self.kind),
            self.payload.clone(),
            json!(self.authority),
            json!(self.at),
        ];
        // Only restricted events carry the extra field, so every event written before restrictions
        // existed still hashes exactly as it did and old logs keep verifying.
        if let Some(allowed) = &self.restricted_to {
            fields.push(json!(allowed));
        }
        let bytes = serde_json::to_vec(&Value::Array(fields)).unwrap_or_default();
        let mut digest = Sha256::new();
        digest.update(b"moo-teams-event-v1\0");
        digest.update(&bytes);
        format!("{:x}", digest.finalize())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredReceipt {
    pub digest: String,
    pub result: Value,
    /// The revision the command committed at; orders receipts for trimming.
    #[serde(default)]
    pub revision: u64,
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
        /// Defaults to the whole team.
        #[serde(default)]
        audience: Audience,
    },
    PutKnowledge {
        team_id: String,
        knowledge_id: String,
        scope: KnowledgeScope,
        title: String,
        body: String,
        #[serde(default)]
        evidence: Vec<Evidence>,
        /// A requirement or ADR starts as proposed and counts only once the Owner accepts it.
        #[serde(default)]
        knowledge_kind: KnowledgeKind,
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
    /// Ask the Owner to authorize one exact protected action. Chat can never do this.
    RequestApproval {
        approval_id: String,
        team_id: String,
        action: ProtectedAction,
        justification: String,
        #[serde(default)]
        alternatives: Vec<String>,
        #[serde(default)]
        risks: Vec<String>,
        ttl_seconds: u64,
    },
    DecideApproval {
        approval_id: String,
        approve: bool,
        rationale: String,
    },
    RevokeApproval {
        approval_id: String,
        rationale: String,
    },
    /// Spend an approval, once, for exactly the action it was granted for.
    ConsumeApproval {
        approval_id: String,
        execution_id: String,
        action: ProtectedAction,
    },
    RequestRecruitment {
        recruitment_id: String,
        team_id: String,
        specialty: String,
        justification: String,
        deliverable: String,
        capabilities: Vec<String>,
        #[serde(default)]
        dependencies: Vec<String>,
        stop_condition: String,
        max_retries: u8,
        max_minutes: u32,
    },
    ActivateRecruitment {
        recruitment_id: String,
        actor_id: String,
        credential_digest: String,
        run_generation: u64,
    },
    SubmitSpecialistHandoff {
        recruitment_id: String,
        summary: String,
        evidence: Vec<Evidence>,
    },
    VerifyRecruitment {
        recruitment_id: String,
        accepted: bool,
        rationale: String,
    },
    CancelRecruitment {
        recruitment_id: String,
        reason: String,
    },
    /// The team id never changes; only its label does.
    RenameTeam {
        team_id: String,
        name: String,
    },
    SuspendTeam {
        team_id: String,
        reason: String,
    },
    ResumeTeam {
        team_id: String,
    },
    CancelTeam {
        team_id: String,
        reason: String,
    },
    /// Name a public interface, schema or API by the paths that define it.
    RegisterSurface {
        surface_id: String,
        project_id: String,
        path_prefixes: Vec<String>,
    },
    /// Record that a team owns one worktree path and branch. The harness creates the worktree.
    ClaimWorktree {
        claim_id: String,
        team_id: String,
        path: String,
        branch: String,
        base_commit: String,
    },
    ReleaseWorktree {
        claim_id: String,
    },
    /// Says whether a released, never-used worktree may be deleted. Teams never deletes it.
    ApproveWorktreeCleanup {
        claim_id: String,
    },
    SubmitIntegration {
        integration_id: String,
        team_id: String,
        claim_id: String,
        target_branch: String,
        commit: String,
        changed_paths: Vec<String>,
        #[serde(default)]
        touches: Vec<String>,
        #[serde(default)]
        depends_on: Vec<String>,
        tests: Vec<Evidence>,
    },
    ReviewIntegration {
        integration_id: String,
        accepted: bool,
        rationale: String,
        #[serde(default)]
        evidence: Vec<Evidence>,
    },
    ResolveConflict {
        conflict_id: String,
        rationale: String,
    },
    AcknowledgeNotice {
        notice_id: String,
    },
    CancelIntegration {
        integration_id: String,
        reason: String,
    },
    /// Owner only; spends an approval naming this integration and commit. There is no force option.
    BeginMerge {
        integration_id: String,
        approval_id: String,
    },
    CompleteMerge {
        integration_id: String,
        merge_commit: String,
    },
    AbortMerge {
        integration_id: String,
        reason: String,
    },
    /// Record which Moo pane runs an agent, and with which provider CLI.
    BindAgentPane {
        actor_id: String,
        pane_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
    },
    UnbindAgentPane {
        actor_id: String,
    },
    DeclareProvider {
        provider_id: String,
        label: String,
        #[serde(default)]
        capabilities: Vec<String>,
        #[serde(default)]
        models: Vec<ModelDecl>,
        metering: Metering,
        #[serde(default)]
        hard_spend_limit: bool,
    },
    SetRolePreference {
        role_key: String,
        provider_id: String,
        #[serde(default)]
        model_id: Option<String>,
    },
    AssignRoleProvider {
        team_id: String,
        role_key: String,
        provider_id: String,
        #[serde(default)]
        model_id: Option<String>,
    },
    ReassignRoleProvider {
        team_id: String,
        role_key: String,
        provider_id: String,
        #[serde(default)]
        model_id: Option<String>,
        approval_id: String,
    },
    ProposePlan {
        plan_id: String,
        goal: String,
    },
    EditPlan {
        plan_id: String,
        edits: Vec<PlanEdit>,
    },
    DiscardPlan {
        plan_id: String,
    },
    MarkPlanLaunched {
        plan_id: String,
        team_id: String,
    },
    PutContextNote {
        team_id: String,
        note_id: String,
        note_kind: NoteKind,
        #[serde(default)]
        role_key: Option<String>,
        summary: String,
        sources: Vec<SourceRef>,
        #[serde(default)]
        valid_for_seconds: Option<u64>,
    },
    SupersedeContextNote {
        note_id: String,
        by_note_id: String,
    },
    AcceptKnowledge {
        knowledge_id: String,
    },
    SupersedeKnowledge {
        knowledge_id: String,
        by_id: String,
    },
    RaiseDispute {
        dispute_id: String,
        team_id: String,
        #[serde(default)]
        task_id: Option<String>,
        topic: String,
        #[serde(default)]
        architectural: bool,
        position: String,
        evidence: Vec<Evidence>,
    },
    RespondDispute {
        dispute_id: String,
        position: String,
        #[serde(default)]
        evidence: Vec<Evidence>,
    },
    ResolveDispute {
        dispute_id: String,
        resolution: String,
    },
    DecideDispute {
        dispute_id: String,
        decision: String,
    },
    ReportUsage {
        team_id: String,
        provider_id: String,
        input_tokens: u64,
        output_tokens: u64,
        #[serde(default)]
        cost_micros: Option<u64>,
        basis: UsageBasis,
    },
    SetTeamLimits {
        team_id: String,
        #[serde(default)]
        max_metered_cost_micros: Option<u64>,
        #[serde(default)]
        max_metered_tokens: Option<u64>,
    },
    SetAgentState {
        actor_id: String,
        state: AgentRunState,
        reason: String,
    },
}

impl Command {
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
            Self::RequestApproval { .. } => "request_approval",
            Self::DecideApproval { .. } => "decide_approval",
            Self::RevokeApproval { .. } => "revoke_approval",
            Self::ConsumeApproval { .. } => "consume_approval",
            Self::RequestRecruitment { .. } => "request_recruitment",
            Self::ActivateRecruitment { .. } => "activate_recruitment",
            Self::SubmitSpecialistHandoff { .. } => "submit_specialist_handoff",
            Self::VerifyRecruitment { .. } => "verify_recruitment",
            Self::CancelRecruitment { .. } => "cancel_recruitment",
            Self::RenameTeam { .. } => "rename_team",
            Self::SuspendTeam { .. } => "suspend_team",
            Self::ResumeTeam { .. } => "resume_team",
            Self::CancelTeam { .. } => "cancel_team",
            Self::RegisterSurface { .. } => "register_surface",
            Self::ClaimWorktree { .. } => "claim_worktree",
            Self::ReleaseWorktree { .. } => "release_worktree",
            Self::ApproveWorktreeCleanup { .. } => "approve_worktree_cleanup",
            Self::SubmitIntegration { .. } => "submit_integration",
            Self::ReviewIntegration { .. } => "review_integration",
            Self::ResolveConflict { .. } => "resolve_conflict",
            Self::AcknowledgeNotice { .. } => "acknowledge_notice",
            Self::CancelIntegration { .. } => "cancel_integration",
            Self::BeginMerge { .. } => "begin_merge",
            Self::CompleteMerge { .. } => "complete_merge",
            Self::AbortMerge { .. } => "abort_merge",
            Self::BindAgentPane { .. } => "bind_agent_pane",
            Self::UnbindAgentPane { .. } => "unbind_agent_pane",
            Self::DeclareProvider { .. } => "declare_provider",
            Self::SetRolePreference { .. } => "set_role_preference",
            Self::AssignRoleProvider { .. } => "assign_role_provider",
            Self::ReassignRoleProvider { .. } => "reassign_role_provider",
            Self::ProposePlan { .. } => "propose_plan",
            Self::EditPlan { .. } => "edit_plan",
            Self::DiscardPlan { .. } => "discard_plan",
            Self::MarkPlanLaunched { .. } => "mark_plan_launched",
            Self::PutContextNote { .. } => "put_context_note",
            Self::SupersedeContextNote { .. } => "supersede_context_note",
            Self::AcceptKnowledge { .. } => "accept_knowledge",
            Self::SupersedeKnowledge { .. } => "supersede_knowledge",
            Self::RaiseDispute { .. } => "raise_dispute",
            Self::RespondDispute { .. } => "respond_dispute",
            Self::ResolveDispute { .. } => "resolve_dispute",
            Self::DecideDispute { .. } => "decide_dispute",
            Self::ReportUsage { .. } => "report_usage",
            Self::SetTeamLimits { .. } => "set_team_limits",
            Self::SetAgentState { .. } => "set_agent_state",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    pub revision: u64,
    pub stream_id: String,
    pub next_sequence: u64,
    /// Events up to this sequence were compacted away; the first retained one follows it.
    #[serde(default)]
    pub events_base: u64,
    /// Hash of the last compacted event, so the retained chain still verifies.
    #[serde(default)]
    pub base_hash: String,
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
    #[serde(default)]
    pub approvals: BTreeMap<String, Approval>,
    #[serde(default)]
    pub recruitments: BTreeMap<String, Recruitment>,
    #[serde(default)]
    pub surfaces: BTreeMap<String, SharedSurface>,
    #[serde(default)]
    pub worktrees: BTreeMap<String, WorktreeClaim>,
    #[serde(default)]
    pub integrations: BTreeMap<String, Integration>,
    #[serde(default)]
    pub conflicts: BTreeMap<String, Conflict>,
    #[serde(default)]
    pub notices: BTreeMap<String, ChangeNotice>,
    #[serde(default)]
    pub pane_bindings: BTreeMap<String, PaneBinding>,
    #[serde(default)]
    pub providers: BTreeMap<String, Provider>,
    #[serde(default)]
    pub preferences: BTreeMap<String, Preference>,
    #[serde(default)]
    pub role_assignments: BTreeMap<String, RoleAssignment>,
    #[serde(default)]
    pub plans: BTreeMap<String, Plan>,
    #[serde(default)]
    pub context_notes: BTreeMap<String, ContextNote>,
    #[serde(default)]
    pub disputes: BTreeMap<String, Dispute>,
    #[serde(default)]
    pub usage: BTreeMap<String, TeamUsage>,
    #[serde(default)]
    pub team_limits: BTreeMap<String, TeamLimits>,
    #[serde(default)]
    pub agent_states: BTreeMap<String, AgentState>,
    /// Server clock for the command being applied (unix seconds); set per command, never stored.
    #[serde(skip)]
    now: u64,
}

struct PendingEvent {
    project_id: Option<String>,
    team_id: Option<String>,
    kind: &'static str,
    payload: Value,
    /// Only these actors (and the Owner) may read the event; `None` means the whole team.
    restricted_to: Option<Vec<String>>,
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
            events_base: 0,
            base_hash: String::new(),
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
            approvals: BTreeMap::new(),
            recruitments: BTreeMap::new(),
            surfaces: BTreeMap::new(),
            worktrees: BTreeMap::new(),
            integrations: BTreeMap::new(),
            conflicts: BTreeMap::new(),
            notices: BTreeMap::new(),
            pane_bindings: BTreeMap::new(),
            providers: BTreeMap::new(),
            preferences: BTreeMap::new(),
            role_assignments: BTreeMap::new(),
            plans: BTreeMap::new(),
            context_notes: BTreeMap::new(),
            disputes: BTreeMap::new(),
            usage: BTreeMap::new(),
            team_limits: BTreeMap::new(),
            agent_states: BTreeMap::new(),
            now: 0,
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

    #[cfg(test)]
    pub fn execute(&mut self, actor: &Actor, command: Command) -> Result<Value> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        self.execute_at(actor, command, now)
    }

    /// Applies `command` as of `now` (unix seconds). Authorization runs here, at execution time,
    /// against the live state, so an approval or credential changed since the request still counts.
    pub fn execute_at(&mut self, actor: &Actor, command: Command, now: u64) -> Result<Value> {
        self.ensure_current_actor(actor)?;
        governance::authorize(actor, &command)?;
        self.ensure_team_may_work(&command)?;
        let authority = command.authority();
        let commit_revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::new("capacity_exceeded", "state revision is exhausted"))?;
        let mut candidate = self.clone();
        candidate.now = now;
        let applied = candidate.apply(actor, command, commit_revision)?;
        if applied.events.is_empty() {
            return Err(Error::new(
                "corrupt_state",
                "a successful command produced no durable event",
            ));
        }
        candidate.revision = commit_revision;
        for pending in applied.events {
            candidate.append_event(actor.id.clone(), commit_revision, Some(authority), pending)?;
        }
        for task in candidate
            .tasks
            .values_mut()
            .filter(|t| t.revision == commit_revision)
        {
            task.updated_at = now;
        }
        candidate.now = 0;
        candidate.compact_events_if_needed();
        candidate.validate()?;
        *self = candidate;
        Ok(applied.result)
    }

    pub fn snapshot(&self, actor: &Actor, team_id: Option<&str>) -> Result<Value> {
        // State is validated when it is loaded and when it is committed, not on every read.
        self.ensure_current_actor(actor)?;
        let selected_team = self.resolve_read_team(actor, team_id)?;
        // A specialist sees its own engagement and the tasks it was scoped to - not the team's
        // private chat, decisions, knowledge, roster or dispatches.
        let specialist = actor.role == Role::Specialist;
        let scoped_tasks: BTreeSet<&str> = self
            .recruitments
            .values()
            .filter(|recruitment| {
                matches!(
                    &recruitment.status,
                    governance::RecruitmentStatus::Active { actor_id, .. }
                        | governance::RecruitmentStatus::HandedOff { actor_id, .. }
                        if actor_id == &actor.id
                )
            })
            .flat_map(|recruitment| recruitment.dependencies.iter().map(String::as_str))
            .collect();
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
            .filter(|_| !specialist)
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
                !specialist
                    && candidate.role != Role::Owner
                    && candidate
                        .team_id
                        .as_deref()
                        .is_some_and(|id| selected_ids.contains(id))
            })
            .map(actor_view)
            .collect();
        let tasks = filtered_values(&self.tasks, |value| {
            selected_ids.contains(value.team_id.as_str())
                && (!specialist || scoped_tasks.contains(value.id.as_str()))
        });
        let acceptances = filtered_values(&self.acceptances, |value| {
            !specialist && selected_ids.contains(value.team_id.as_str())
        });
        let handoffs = filtered_values(&self.handoffs, |value| {
            !specialist && selected_ids.contains(value.team_id.as_str())
        });
        let decisions = filtered_values(&self.decisions, |value| {
            !specialist && selected_ids.contains(value.team_id.as_str())
        });
        let messages = filtered_values(&self.messages, |value| {
            !specialist
                && selected_ids.contains(value.team_id.as_str())
                && message_visible_to(actor, value)
        });
        // Records are keyed by id, which is not time order: show messages and decisions as they happened.
        let mut messages = messages;
        messages.sort_by(|a, b| (a.revision, &a.id).cmp(&(b.revision, &b.id)));
        let mut decisions = decisions;
        decisions.sort_by(|a, b| (a.revision, &a.id).cmp(&(b.revision, &b.id)));
        let disputes = filtered_values(&self.disputes, |value| {
            !specialist && selected_ids.contains(value.team_id.as_str())
        });
        let role_assignments = filtered_values(&self.role_assignments, |value| {
            !specialist && selected_ids.contains(value.team_id.as_str())
        });
        let agent_states: Vec<&AgentState> = self
            .agent_states
            .values()
            .filter(|state| {
                !specialist
                    && self
                        .actors
                        .get(&state.actor_id)
                        .and_then(|agent| agent.team_id.as_deref())
                        .is_some_and(|team| selected_ids.contains(team))
            })
            .collect();
        let usage: Vec<Value> = selected_ids
            .iter()
            .filter(|_| !specialist)
            .map(|team| self.team_usage_view(team))
            .collect();
        // Notes say whether they still hold, so a client never shows a stale one as current.
        let context_notes: Vec<Value> = self
            .context_notes
            .values()
            .filter(|note| !specialist && selected_ids.contains(note.team_id.as_str()))
            .map(|note| {
                let validity = match self.note_validity(note, wall_clock()) {
                    memory::Validity::Current => json!({"state": "current"}),
                    memory::Validity::Stale(reason) => json!({"state": "stale", "reason": reason}),
                };
                json!({"note": note, "validity": validity})
            })
            .collect();
        let bindings = filtered_values(&self.pane_bindings, |value| {
            !specialist
                && self
                    .actors
                    .get(&value.actor_id)
                    .and_then(|bound| bound.team_id.as_deref())
                    .is_some_and(|team| selected_ids.contains(team))
        });
        // Project knowledge is shared by every team of that project; team knowledge stays private.
        let visible_projects: BTreeSet<&str> = self
            .teams
            .values()
            .filter(|team| selected_ids.contains(team.id.as_str()))
            .map(|team| team.project_id.as_str())
            .collect();
        let knowledge = filtered_values(&self.knowledge, |value| {
            !specialist
                && (selected_ids.contains(value.team_id.as_str())
                    || matches!(&value.scope, KnowledgeScope::Project { project_id }
                        if visible_projects.contains(project_id.as_str())))
        });
        let surfaces = filtered_values(&self.surfaces, |value| {
            !specialist && visible_projects.contains(value.project_id.as_str())
        });
        let worktrees = filtered_values(&self.worktrees, |value| {
            !specialist && selected_ids.contains(value.team_id.as_str())
        });
        let integrations = filtered_values(&self.integrations, |value| {
            !specialist && selected_ids.contains(value.team_id.as_str())
        });
        let involves_selected = |teams: &[&String]| {
            !specialist
                && teams
                    .iter()
                    .any(|team| selected_ids.contains(team.as_str()))
        };
        let conflicts = filtered_values(&self.conflicts, |value| {
            involves_selected(&[&value.team_ids[0], &value.team_ids[1]])
        });
        let notices = filtered_values(&self.notices, |value| {
            involves_selected(&[&value.from_team_id, &value.to_team_id])
        });
        let dispatches = filtered_values(&self.dispatches, |value| {
            !specialist && selected_ids.contains(value.team_id.as_str())
        });
        let approvals = filtered_values(&self.approvals, |value| {
            !specialist && selected_ids.contains(value.team_id.as_str())
        });
        let recruitments = filtered_values(&self.recruitments, |value| {
            selected_ids.contains(value.team_id.as_str())
                && (!specialist
                    || matches!(
                        &value.status,
                        governance::RecruitmentStatus::Active { actor_id, .. }
                            | governance::RecruitmentStatus::HandedOff { actor_id, .. }
                            if actor_id == &actor.id
                    ))
        });
        // What is waiting on a person, so a client can surface it without scanning every record.
        let attention = json!({
            "pending_approvals": approvals
                .iter()
                .filter(|approval| matches!(approval.status, governance::ApprovalStatus::Pending))
                .map(|approval| approval.id.as_str())
                .collect::<Vec<_>>(),
            "queued_recruitments": recruitments
                .iter()
                .filter(|recruitment| matches!(recruitment.status, governance::RecruitmentStatus::Queued))
                .map(|recruitment| recruitment.id.as_str())
                .collect::<Vec<_>>(),
            "open_conflicts": conflicts
                .iter()
                .filter(|conflict| conflict.status == portfolio::ConflictStatus::Open)
                .map(|conflict| conflict.id.as_str())
                .collect::<Vec<_>>(),
            "open_notices": notices
                .iter()
                .filter(|notice| notice.status == portfolio::NoticeStatus::Open)
                .map(|notice| notice.id.as_str())
                .collect::<Vec<_>>(),
            "escalated_disputes": disputes
                .iter()
                .filter(|d| matches!(d.status, oversight::DisputeStatus::Escalated { .. }))
                .map(|d| d.id.as_str())
                .collect::<Vec<_>>(),
            "merge_unknown": integrations
                .iter()
                .filter(|item| matches!(item.status, portfolio::IntegrationStatus::MergeUnknown { .. }))
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
        });
        serialize_value(&json!({
            "version": self.version,
            "revision": self.revision,
            "stream_id": self.stream_id,
            "viewer": { "id": actor.id, "role": actor.role, "team_id": actor.team_id },
            "last_event_sequence": self.next_sequence.saturating_sub(1),
            "retained_from": self.events_base + 1,
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
            "approvals": approvals,
            "recruitments": recruitments,
            "surfaces": surfaces,
            "worktrees": worktrees,
            "integrations": integrations,
            "conflicts": conflicts,
            "notices": notices,
            "bindings": bindings,
            "disputes": disputes,
            "role_assignments": role_assignments,
            "agent_states": agent_states,
            "usage": usage,
            "context_notes": context_notes,
            "providers": self.providers.values().collect::<Vec<_>>(),
            "attention": attention,
        }))
    }

    pub fn event_page(
        &self,
        actor: &Actor,
        stream_id: &str,
        after: u64,
        limit: usize,
    ) -> Result<Value> {
        self.ensure_current_actor(actor)?;
        let resync = || {
            json!({
                "stream_id": self.stream_id,
                "retained_from": self.events_base + 1,
                "watermark": self.next_sequence.saturating_sub(1),
            })
        };
        if stream_id != self.stream_id {
            return Err(
                Error::new("cursor_expired", "event cursor belongs to another stream")
                    .with_details(resync()),
            );
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
            )
            .with_details(resync()));
        }
        if after < self.events_base {
            return Err(Error::new(
                "cursor_expired",
                "event cursor is older than the retained log; read a snapshot and resume from its watermark",
            )
            .with_details(resync()));
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
            "watermark": last_sequence,
            "retained_from": self.events_base + 1,
            "events": page,
        }))
    }

    pub fn recover(&mut self) -> Result<bool> {
        let affected: Vec<String> = self
            .dispatches
            .iter()
            .filter(|(_, attempt)| {
                matches!(
                    attempt.state,
                    DispatchState::Attempted | DispatchState::Submitted { .. }
                )
            })
            .map(|(id, _)| id.clone())
            .collect();
        let merging = self
            .integrations
            .values()
            .any(|item| matches!(item.status, portfolio::IntegrationStatus::Merging { .. }));
        if affected.is_empty() && !merging {
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
        // A merge that was running may or may not have landed; the Owner records which.
        let merges = candidate.mark_in_flight_merges_unknown(revision);
        candidate.revision = revision;
        candidate.append_event(
            RECOVERY_ACTOR_ID.to_owned(),
            revision,
            None,
            PendingEvent {
                project_id: None,
                team_id: None,
                restricted_to: None,
                kind: "dispatch_recovery_unknown",
                payload: json!({ "attempt_ids": affected, "merge_unknown_integration_ids": merges }),
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
                audience,
            } => self.append_chat(
                actor, team_id, message_id, body, task_id, audience, revision,
            ),
            Command::PutKnowledge {
                team_id,
                knowledge_id,
                scope,
                title,
                body,
                evidence,
                knowledge_kind: kind,
            } => self.put_knowledge(
                actor,
                team_id,
                knowledge_id,
                scope,
                title,
                body,
                evidence,
                kind,
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
            Command::RequestApproval {
                approval_id,
                team_id,
                action,
                justification,
                alternatives,
                risks,
                ttl_seconds,
            } => self.request_approval(
                actor,
                approval_id,
                team_id,
                action,
                justification,
                alternatives,
                risks,
                ttl_seconds,
                revision,
            ),
            Command::DecideApproval {
                approval_id,
                approve,
                rationale,
            } => self.decide_approval(actor, approval_id, approve, rationale, revision),
            Command::RevokeApproval {
                approval_id,
                rationale,
            } => self.revoke_approval(actor, approval_id, rationale, revision),
            Command::ConsumeApproval {
                approval_id,
                execution_id,
                action,
            } => self.consume_approval(actor, approval_id, execution_id, action, revision),
            Command::RequestRecruitment {
                recruitment_id,
                team_id,
                specialty,
                justification,
                deliverable,
                capabilities,
                dependencies,
                stop_condition,
                max_retries,
                max_minutes,
            } => self.request_recruitment(
                actor,
                RecruitmentRequest {
                    id: recruitment_id,
                    team_id,
                    specialty,
                    justification,
                    deliverable,
                    capabilities,
                    dependencies,
                    stop_condition,
                    max_retries,
                    max_minutes,
                },
                revision,
            ),
            Command::ActivateRecruitment {
                recruitment_id,
                actor_id,
                credential_digest,
                run_generation,
            } => self.activate_recruitment(
                actor,
                recruitment_id,
                actor_id,
                credential_digest,
                run_generation,
                revision,
            ),
            Command::SubmitSpecialistHandoff {
                recruitment_id,
                summary,
                evidence,
            } => self.submit_specialist_handoff(actor, recruitment_id, summary, evidence, revision),
            Command::VerifyRecruitment {
                recruitment_id,
                accepted,
                rationale,
            } => self.verify_recruitment(actor, recruitment_id, accepted, rationale, revision),
            Command::CancelRecruitment {
                recruitment_id,
                reason,
            } => self.cancel_recruitment(actor, recruitment_id, reason, revision),
            Command::RenameTeam { team_id, name } => {
                self.rename_team(actor, team_id, name, revision)
            }
            Command::SuspendTeam { team_id, reason } => {
                self.suspend_team(actor, team_id, reason, revision)
            }
            Command::ResumeTeam { team_id } => self.resume_team(actor, team_id, revision),
            Command::CancelTeam { team_id, reason } => {
                self.cancel_team(actor, team_id, reason, revision)
            }
            Command::RegisterSurface {
                surface_id,
                project_id,
                path_prefixes,
            } => self.register_surface(actor, surface_id, project_id, path_prefixes, revision),
            Command::ClaimWorktree {
                claim_id,
                team_id,
                path,
                branch,
                base_commit,
            } => self.claim_worktree(
                actor,
                claim_id,
                team_id,
                path,
                branch,
                base_commit,
                revision,
            ),
            Command::ReleaseWorktree { claim_id } => {
                self.release_worktree(actor, claim_id, revision)
            }
            Command::ApproveWorktreeCleanup { claim_id } => {
                self.approve_worktree_cleanup(actor, claim_id, revision)
            }
            Command::SubmitIntegration {
                integration_id,
                team_id,
                claim_id,
                target_branch,
                commit,
                changed_paths,
                touches,
                depends_on,
                tests,
            } => self.submit_integration(
                actor,
                Submission {
                    id: integration_id,
                    team_id,
                    claim_id,
                    target_branch,
                    commit,
                    changed_paths,
                    touches,
                    depends_on,
                    tests,
                },
                revision,
            ),
            Command::ReviewIntegration {
                integration_id,
                accepted,
                rationale,
                evidence,
            } => self.review_integration(
                actor,
                integration_id,
                accepted,
                rationale,
                evidence,
                revision,
            ),
            Command::ResolveConflict {
                conflict_id,
                rationale,
            } => self.resolve_conflict(actor, conflict_id, rationale, revision),
            Command::AcknowledgeNotice { notice_id } => {
                self.acknowledge_notice(actor, notice_id, revision)
            }
            Command::CancelIntegration {
                integration_id,
                reason,
            } => self.cancel_integration(actor, integration_id, reason, revision),
            Command::BeginMerge {
                integration_id,
                approval_id,
            } => self.begin_merge(actor, integration_id, approval_id, revision),
            Command::CompleteMerge {
                integration_id,
                merge_commit,
            } => self.complete_merge(actor, integration_id, merge_commit, revision),
            Command::AbortMerge {
                integration_id,
                reason,
            } => self.abort_merge(actor, integration_id, reason, revision),
            Command::BindAgentPane {
                actor_id,
                pane_id,
                provider,
            } => self.bind_agent_pane(actor, actor_id, pane_id, provider, revision),
            Command::UnbindAgentPane { actor_id } => {
                self.unbind_agent_pane(actor, actor_id, revision)
            }
            Command::DeclareProvider {
                provider_id,
                label,
                capabilities,
                models,
                metering,
                hard_spend_limit,
            } => self.declare_provider(
                actor,
                provider_id,
                label,
                capabilities,
                models,
                metering,
                hard_spend_limit,
                revision,
            ),
            Command::SetRolePreference {
                role_key,
                provider_id,
                model_id,
            } => self.set_role_preference(actor, role_key, provider_id, model_id, revision),
            Command::AssignRoleProvider {
                team_id,
                role_key,
                provider_id,
                model_id,
            } => {
                self.assign_role_provider(actor, team_id, role_key, provider_id, model_id, revision)
            }
            Command::ReassignRoleProvider {
                team_id,
                role_key,
                provider_id,
                model_id,
                approval_id,
            } => self.reassign_role_provider(
                actor,
                team_id,
                role_key,
                provider_id,
                model_id,
                approval_id,
                revision,
            ),
            Command::ProposePlan { plan_id, goal } => {
                self.propose_plan(actor, plan_id, goal, revision)
            }
            Command::EditPlan { plan_id, edits } => self.edit_plan(actor, plan_id, edits, revision),
            Command::DiscardPlan { plan_id } => self.discard_plan(actor, plan_id, revision),
            Command::MarkPlanLaunched { plan_id, team_id } => {
                self.mark_plan_launched(actor, plan_id, team_id, revision)
            }
            Command::PutContextNote {
                team_id,
                note_id,
                note_kind: kind,
                role_key,
                summary,
                sources,
                valid_for_seconds,
            } => self.put_context_note(
                actor,
                team_id,
                note_id,
                kind,
                role_key,
                summary,
                sources,
                valid_for_seconds,
                revision,
            ),
            Command::SupersedeContextNote {
                note_id,
                by_note_id,
            } => self.supersede_context_note(actor, note_id, by_note_id, revision),
            Command::AcceptKnowledge { knowledge_id } => {
                self.accept_knowledge(actor, knowledge_id, revision)
            }
            Command::SupersedeKnowledge {
                knowledge_id,
                by_id,
            } => self.supersede_knowledge(actor, knowledge_id, by_id, revision),
            Command::RaiseDispute {
                dispute_id,
                team_id,
                task_id,
                topic,
                architectural,
                position,
                evidence,
            } => self.raise_dispute(
                actor,
                dispute_id,
                team_id,
                task_id,
                topic,
                architectural,
                position,
                evidence,
                revision,
            ),
            Command::RespondDispute {
                dispute_id,
                position,
                evidence,
            } => self.respond_dispute(actor, dispute_id, position, evidence, revision),
            Command::ResolveDispute {
                dispute_id,
                resolution,
            } => self.resolve_dispute(actor, dispute_id, resolution, revision),
            Command::DecideDispute {
                dispute_id,
                decision,
            } => self.decide_dispute(actor, dispute_id, decision, revision),
            Command::ReportUsage {
                team_id,
                provider_id,
                input_tokens,
                output_tokens,
                cost_micros,
                basis,
            } => self.report_usage(
                actor,
                team_id,
                provider_id,
                input_tokens,
                output_tokens,
                cost_micros,
                basis,
                revision,
            ),
            Command::SetTeamLimits {
                team_id,
                max_metered_cost_micros,
                max_metered_tokens,
            } => self.set_team_limits(
                actor,
                team_id,
                max_metered_cost_micros,
                max_metered_tokens,
                revision,
            ),
            Command::SetAgentState {
                actor_id,
                state,
                reason,
            } => self.set_agent_state(actor, actor_id, state, reason, revision),
        }
    }

    /// Drops the oldest events once the log passes `RETAIN_EVENTS_MAX`, keeping `RETAIN_EVENTS_KEEP`.
    /// The hash chain stays verifiable from `base_hash`, and a reader holding an older cursor is told
    /// to resync from a snapshot instead of silently missing events.
    pub fn compact_events_if_needed(&mut self) {
        self.compact_if_over(RETAIN_EVENTS_MAX, RETAIN_EVENTS_KEEP);
    }

    pub fn compact_if_over(&mut self, max: usize, keep: usize) {
        if self.events.len() > max {
            self.compact_events(keep);
        }
    }

    pub fn compact_events(&mut self, keep: usize) {
        if self.events.len() <= keep {
            return;
        }
        let dropped = self.events.len() - keep;
        self.base_hash = self.events[dropped - 1].hash.clone();
        self.events_base += dropped as u64;
        self.events.drain(..dropped);
    }

    /// Idempotency receipts only need to cover a retry window; keep the newest `MAX_RECEIPTS`.
    pub fn trim_receipts(&mut self) {
        if self.receipts.len() <= MAX_RECEIPTS {
            return;
        }
        let mut order: Vec<(u64, String)> = self
            .receipts
            .iter()
            .map(|(key, receipt)| (receipt.revision, key.clone()))
            .collect();
        order.sort();
        for (_, key) in order.into_iter().take(self.receipts.len() - MAX_RECEIPTS) {
            self.receipts.remove(&key);
        }
    }

    fn append_event(
        &mut self,
        actor_id: String,
        revision: u64,
        authority: Option<Authority>,
        event: PendingEvent,
    ) -> Result<()> {
        let sequence = self.next_sequence;
        self.next_sequence = sequence
            .checked_add(1)
            .ok_or_else(|| Error::new("capacity_exceeded", "event sequence is exhausted"))?;
        let mut entry = Event {
            sequence,
            revision,
            actor_id,
            project_id: event.project_id,
            team_id: event.team_id,
            kind: event.kind.to_owned(),
            payload: event.payload,
            authority,
            at: self.now,
            restricted_to: event.restricted_to,
            prev_hash: self
                .events
                .last()
                .map_or_else(|| self.base_hash.clone(), |last| last.hash.clone()),
            hash: String::new(),
        };
        entry.hash = entry.compute_hash();
        self.events.push(entry);
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
        if actor.role == Role::Owner {
            return true;
        }
        // A specialist sees its engagement through the snapshot, never the team's event stream.
        if actor.role == Role::Specialist {
            return false;
        }
        if event
            .restricted_to
            .as_ref()
            .is_some_and(|allowed| !allowed.contains(&actor.id))
        {
            return false;
        }
        actor
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
        if matches!(role, Role::Owner | Role::Specialist) {
            return Err(Error::new(
                "invalid_params",
                "only Lead, Developer and QA are team role enrollments",
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
        self.ensure_project_has_team_capacity(&spec.project_id)?;
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
            status: TeamStatus::Active,
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
        self.ensure_room_for_task(&team_id)?;
        self.ensure_depth_within_limit(&task_id, &dependencies)?;
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
            updated_at: self.now,
        };
        let message = Message {
            id: assignment_message_id.clone(),
            team_id: team_id.clone(),
            sender_id: actor.id.clone(),
            audience: Audience::Team,
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
        self.ensure_depth_within_limit(&task_id, &dependencies)?;
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
        self.ensure_room_to_start(&team_id)?;
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
            audience: Audience::Team,
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
            audience: Audience::Team,
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
            audience: Audience::Team,
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
            audience: Audience::Team,
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
    #[allow(clippy::too_many_arguments)] // A chat message names its channel, audience and task.
    fn append_chat(
        &mut self,
        actor: &Actor,
        team_id: String,
        message_id: String,
        body: String,
        task_id: Option<String>,
        audience: Audience,
        revision: u64,
    ) -> Result<Applied> {
        let team = self.require_team_member_or_owner(actor, &team_id)?.clone();
        validate_id("message_id", &message_id)?;
        validate_text("message body", &body, MAX_MESSAGE_BYTES, false)?;
        if self.messages.contains_key(&message_id) {
            return Err(Error::new("already_exists", "message already exists"));
        }
        match &audience {
            Audience::Team => {}
            Audience::Role { role } => {
                if !matches!(role, Role::Lead | Role::Developer | Role::Qa) {
                    return Err(Error::new(
                        "invalid_params",
                        "a role message goes to the Lead, Developer or QA",
                    ));
                }
            }
            Audience::Actor { actor_id } => {
                let addressee = self
                    .actors
                    .get(actor_id)
                    .ok_or_else(|| Error::new("not_found", "message addressee not found"))?;
                if addressee.role != Role::Owner
                    && addressee.team_id.as_deref() != Some(team_id.as_str())
                {
                    return Err(Error::new(
                        "invalid_reference",
                        "a direct message goes to someone on the team or to the Owner",
                    ));
                }
            }
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
            audience: audience.clone(),
            task_id,
            kind: MessageKind::Text,
            body,
            revision,
        };
        self.messages.insert(message_id.clone(), message.clone());
        // Events about scoped messages are scoped too: whoever cannot read the message cannot even
        // learn from the event stream that it exists.
        let restricted_to = match &audience {
            Audience::Team => None,
            Audience::Role { role } => {
                let mut allowed = vec![actor.id.clone()];
                allowed.extend(
                    self.actors
                        .values()
                        .filter(|member| {
                            member.team_id.as_deref() == Some(team_id.as_str())
                                && member.role == *role
                        })
                        .map(|member| member.id.clone()),
                );
                Some(allowed)
            }
            Audience::Actor { actor_id } => Some(vec![actor.id.clone(), actor_id.clone()]),
        };
        let mut done = applied(
            serialize_value(&message)?,
            Some(team_id.clone()),
            Some(team.project_id),
            "chat_message_appended",
            json!({ "team_id": team_id, "message_id": message_id, "audience": audience }),
        );
        if let Some(event) = done.events.first_mut() {
            event.restricted_to = restricted_to;
        }
        Ok(done)
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
        kind: KnowledgeKind,
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
            kind,
            // Only the Owner can make a requirement or an ADR count; notes need no acceptance.
            status: if kind == KnowledgeKind::Note {
                KnowledgeStatus::Accepted
            } else {
                KnowledgeStatus::Proposed
            },
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
                .is_none_or(|reported| reported == current)
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
        self.ensure_agent_available(target_actor_id)?;
        self.ensure_within_usage_limits(team_id)?;
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
            + self.approvals.len()
            + self.recruitments.len()
            + self.surfaces.len()
            + self.worktrees.len()
            + self.integrations.len()
            + self.conflicts.len()
            + self.notices.len()
            + self.pane_bindings.len()
            + self.providers.len()
            + self.plans.len()
            + self.context_notes.len()
            + self.disputes.len()
            + self.events.len();
        if records > MAX_RECORDS {
            return Err(Error::new(
                "capacity_exceeded",
                "Teams record limit reached",
            ));
        }
        if self.next_sequence != self.events_base + self.events.len() as u64 + 1 {
            return Err(corrupt("event sequence does not match the event log"));
        }
        let mut previous_revision = 0;
        for (index, event) in self.events.iter().enumerate() {
            if event.sequence != self.events_base + index as u64 + 1 {
                return Err(corrupt("event log has a gap"));
            }
            if event.revision < previous_revision || event.revision > self.revision {
                return Err(corrupt("event revision is out of order"));
            }
            previous_revision = event.revision;
            let expected_prev = index
                .checked_sub(1)
                .map_or(self.base_hash.as_str(), |before| {
                    self.events[before].hash.as_str()
                });
            if event.prev_hash != expected_prev || event.hash != event.compute_hash() {
                return Err(corrupt("event log hash chain is broken"));
            }
        }
        self.validate_governance()?;
        self.validate_portfolio()?;
        self.validate_bindings()?;
        self.validate_planning()?;
        self.validate_memory()?;
        self.validate_oversight()?;
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
                        .is_none_or(|task_id| known_task(&v.team_id, task_id))
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

/// Team messages are for the whole team; role and direct messages only for their sender, their
/// addressee and the Owner.
fn message_visible_to(viewer: &Actor, message: &Message) -> bool {
    if viewer.role == Role::Owner || message.sender_id == viewer.id {
        return true;
    }
    match &message.audience {
        Audience::Team => true,
        Audience::Role { role } => viewer.role == *role,
        Audience::Actor { actor_id } => &viewer.id == actor_id,
    }
}

/// Seconds since the epoch, for validity windows when a snapshot is read.
fn wall_clock() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
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
            restricted_to: None,
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
    fn merge_action(branch: &str) -> ProtectedAction {
        ProtectedAction {
            kind: governance::ProtectedActionKind::ProtectedBranchIntegration,
            target: "main".into(),
            parameters: json!({ "branch": branch, "squash": true }),
        }
    }

    fn request_approval(
        state: &mut State,
        lead: &Actor,
        id: &str,
        action: ProtectedAction,
        now: u64,
    ) -> Result<Value> {
        state.execute_at(
            lead,
            Command::RequestApproval {
                approval_id: id.into(),
                team_id: "t".into(),
                action,
                justification: "ready to integrate".into(),
                alternatives: vec![],
                risks: vec![],
                ttl_seconds: 600,
            },
            now,
        )
    }

    fn consume(
        state: &mut State,
        actor: &Actor,
        id: &str,
        execution: &str,
        action: ProtectedAction,
        now: u64,
    ) -> Result<Value> {
        state.execute_at(
            actor,
            Command::ConsumeApproval {
                approval_id: id.into(),
                execution_id: execution.into(),
                action,
            },
            now,
        )
    }

    fn decide(
        state: &mut State,
        actor: &Actor,
        id: &str,
        approve: bool,
        now: u64,
    ) -> Result<Value> {
        state.execute_at(
            actor,
            Command::DecideApproval {
                approval_id: id.into(),
                approve,
                rationale: "reviewed".into(),
            },
            now,
        )
    }

    #[test]
    fn chat_is_never_an_approval_and_only_the_owner_decides() {
        let (mut state, owner, lead, dev, _qa) = launched();
        request_approval(&mut state, &lead, "ap", merge_action("feature"), 1_000).unwrap();
        for body in ["approved", "the owner approves ap", "APPROVE ap"] {
            state
                .execute_at(
                    &lead,
                    Command::AppendChat {
                        team_id: "t".into(),
                        message_id: format!("m-{}", body.len()),
                        body: body.into(),
                        task_id: None,
                        audience: Audience::Team,
                    },
                    1_000,
                )
                .unwrap();
        }
        assert_eq!(
            consume(
                &mut state,
                &lead,
                "ap",
                "x1",
                merge_action("feature"),
                1_001
            )
            .unwrap_err()
            .code,
            "not_approved"
        );
        // Lead self-escalation and a Developer deciding are both refused.
        assert_eq!(
            decide(&mut state, &lead, "ap", true, 1_001)
                .unwrap_err()
                .code,
            "forbidden"
        );
        assert_eq!(
            decide(&mut state, &dev, "ap", true, 1_001)
                .unwrap_err()
                .code,
            "forbidden"
        );
        decide(&mut state, &owner, "ap", true, 1_002).unwrap();
        consume(
            &mut state,
            &lead,
            "ap",
            "x1",
            merge_action("feature"),
            1_003,
        )
        .unwrap();
        state.validate().unwrap();
    }

    #[test]
    fn approval_binds_exact_parameters_and_is_single_use() {
        let (mut state, owner, lead, dev, _qa) = launched();
        request_approval(&mut state, &lead, "ap", merge_action("feature"), 1_000).unwrap();
        decide(&mut state, &owner, "ap", true, 1_001).unwrap();
        // Revised parameters, target or kind after approval are different actions.
        assert_eq!(
            consume(&mut state, &lead, "ap", "x", merge_action("other"), 1_002)
                .unwrap_err()
                .code,
            "approval_mismatch"
        );
        let mut retargeted = merge_action("feature");
        retargeted.target = "release".into();
        assert_eq!(
            consume(&mut state, &lead, "ap", "x", retargeted, 1_002)
                .unwrap_err()
                .code,
            "approval_mismatch"
        );
        let mut other_kind = merge_action("feature");
        other_kind.kind = governance::ProtectedActionKind::Release;
        assert_eq!(
            consume(&mut state, &lead, "ap", "x", other_kind, 1_002)
                .unwrap_err()
                .code,
            "approval_mismatch"
        );
        // A Developer who neither requested it nor leads cannot spend it.
        assert_eq!(
            consume(&mut state, &dev, "ap", "x", merge_action("feature"), 1_002)
                .unwrap_err()
                .code,
            "forbidden"
        );
        // Key order in the parameters does not matter; the second use is a replay.
        let reordered = ProtectedAction {
            parameters: json!({ "squash": true, "branch": "feature" }),
            ..merge_action("feature")
        };
        consume(&mut state, &lead, "ap", "x", reordered, 1_003).unwrap();
        assert_eq!(
            consume(&mut state, &lead, "ap", "y", merge_action("feature"), 1_004)
                .unwrap_err()
                .code,
            "approval_used"
        );
        assert_eq!(
            consume(
                &mut state,
                &owner,
                "ap",
                "z",
                merge_action("feature"),
                1_004
            )
            .unwrap_err()
            .code,
            "approval_used"
        );
    }

    #[test]
    fn approvals_expire_and_revocation_wins() {
        let (mut state, owner, lead, _dev, _qa) = launched();
        request_approval(&mut state, &lead, "old", merge_action("a"), 1_000).unwrap();
        decide(&mut state, &owner, "old", true, 1_000).unwrap();
        assert_eq!(
            consume(&mut state, &lead, "old", "x", merge_action("a"), 1_601)
                .unwrap_err()
                .code,
            "approval_expired"
        );
        request_approval(&mut state, &lead, "edge", merge_action("b"), 2_000).unwrap();
        decide(&mut state, &owner, "edge", true, 2_000).unwrap();
        consume(&mut state, &lead, "edge", "x", merge_action("b"), 2_600).unwrap();
        request_approval(&mut state, &lead, "gone", merge_action("c"), 3_000).unwrap();
        decide(&mut state, &owner, "gone", true, 3_000).unwrap();
        state
            .execute_at(
                &owner,
                Command::RevokeApproval {
                    approval_id: "gone".into(),
                    rationale: "changed my mind".into(),
                },
                3_001,
            )
            .unwrap();
        assert_eq!(
            consume(&mut state, &lead, "gone", "x", merge_action("c"), 3_002)
                .unwrap_err()
                .code,
            "not_approved"
        );
    }

    #[test]
    fn pending_approval_survives_restart_and_does_not_block_unrelated_work() {
        let (mut state, _owner, lead, dev, qa) = launched();
        create_task(&mut state, &lead, "task", &[]).unwrap();
        request_approval(&mut state, &lead, "ap", merge_action("feature"), 1_000).unwrap();
        // The unrelated task runs to a QA decision while the approval waits.
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
        // "Restart": the state is written and read back.
        let restored: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        restored.validate().unwrap();
        assert!(matches!(
            restored.approvals["ap"].status,
            governance::ApprovalStatus::Pending
        ));
        let snapshot = restored.snapshot(&lead, None).unwrap();
        assert_eq!(snapshot["attention"]["pending_approvals"], json!(["ap"]));
    }

    #[test]
    fn specialist_lifecycle_is_bounded_and_the_credential_dies_on_release() {
        let (mut state, owner, lead, dev, _qa) = launched();
        create_task(&mut state, &lead, "task", &[]).unwrap();
        let recruit = |id: &str, capabilities: Vec<&str>| Command::RequestRecruitment {
            recruitment_id: id.into(),
            team_id: "t".into(),
            specialty: "security".into(),
            justification: "needs a security look".into(),
            deliverable: "review notes".into(),
            capabilities: capabilities.into_iter().map(String::from).collect(),
            dependencies: vec!["task".into()],
            stop_condition: "notes delivered".into(),
            max_retries: 1,
            max_minutes: 10,
        };
        // Broader rights need an escalation; non-Leads cannot recruit at all.
        assert_eq!(
            state
                .execute_at(&lead, recruit("r0", vec!["review", "write_code"]), 100)
                .unwrap_err()
                .code,
            "escalation_required"
        );
        assert_eq!(
            state
                .execute_at(&dev, recruit("r0", vec!["review"]), 100)
                .unwrap_err()
                .code,
            "forbidden"
        );
        state
            .execute_at(&lead, recruit("r1", vec!["review", "read_code"]), 100)
            .unwrap();
        state
            .execute_at(&lead, recruit("r2", vec!["review"]), 101)
            .unwrap();
        assert_eq!(
            state.recruitments["r1"].status,
            governance::RecruitmentStatus::Authorized
        );
        assert_eq!(
            state.recruitments["r2"].status,
            governance::RecruitmentStatus::Queued,
            "over capacity queues instead of launching"
        );
        state
            .execute_at(
                &owner,
                Command::ActivateRecruitment {
                    recruitment_id: "r1".into(),
                    actor_id: "sec-1".into(),
                    credential_digest: digest('7'),
                    run_generation: 1,
                },
                102,
            )
            .unwrap();
        let sec = state.authenticate(&digest('7')).unwrap();
        // The specialist cannot recruit, chat, read the team stream, or see private records.
        assert_eq!(
            state
                .execute_at(&sec, recruit("r3", vec!["review"]), 103)
                .unwrap_err()
                .code,
            "forbidden"
        );
        assert_eq!(
            state
                .execute_at(
                    &sec,
                    Command::AppendChat {
                        team_id: "t".into(),
                        message_id: "x".into(),
                        body: "hi".into(),
                        task_id: None,
                        audience: Audience::Team,
                    },
                    103
                )
                .unwrap_err()
                .code,
            "forbidden"
        );
        let view = state.snapshot(&sec, None).unwrap();
        assert_eq!(view["messages"], json!([]));
        assert_eq!(view["knowledge"], json!([]));
        assert_eq!(view["actors"], json!([]));
        assert_eq!(
            view["tasks"].as_array().unwrap().len(),
            1,
            "only the scoped task"
        );
        assert_eq!(view["recruitments"].as_array().unwrap().len(), 1);
        assert_eq!(
            state.event_page(&sec, "stream", 0, 100).unwrap()["events"],
            json!([])
        );
        // Handoff needs evidence; a rejected handoff retries once, then the engagement closes.
        let handoff = |evidence| Command::SubmitSpecialistHandoff {
            recruitment_id: "r1".into(),
            summary: "found two issues".into(),
            evidence,
        };
        assert_eq!(
            state
                .execute_at(&sec, handoff(vec![]), 110)
                .unwrap_err()
                .code,
            "invalid_params"
        );
        state.execute_at(&sec, handoff(evidence()), 110).unwrap();
        let verify = |accepted| Command::VerifyRecruitment {
            recruitment_id: "r1".into(),
            accepted,
            rationale: "checked".into(),
        };
        state.execute_at(&lead, verify(false), 111).unwrap();
        assert!(matches!(
            state.recruitments["r1"].status,
            governance::RecruitmentStatus::Active { attempt: 2, .. }
        ));
        state.execute_at(&sec, handoff(evidence()), 112).unwrap();
        state.execute_at(&lead, verify(true), 113).unwrap();
        assert!(matches!(
            state.recruitments["r1"].status,
            governance::RecruitmentStatus::Released { accepted: true, .. }
        ));
        // Released: the process credential is dead, and the queued request got the freed slot.
        assert_eq!(
            state.authenticate(&digest('7')).unwrap_err().code,
            "unauthorized"
        );
        assert_eq!(
            state.recruitments["r2"].status,
            governance::RecruitmentStatus::Authorized
        );
        state.validate().unwrap();
    }

    #[test]
    fn specialist_retries_and_time_allocation_are_enforced() {
        let (mut state, owner, lead, _dev, _qa) = launched();
        state
            .execute_at(
                &lead,
                Command::RequestRecruitment {
                    recruitment_id: "r".into(),
                    team_id: "t".into(),
                    specialty: "architect".into(),
                    justification: "j".into(),
                    deliverable: "d".into(),
                    capabilities: vec!["review".into()],
                    dependencies: vec![],
                    stop_condition: "s".into(),
                    max_retries: 0,
                    max_minutes: 1,
                },
                0,
            )
            .unwrap();
        state
            .execute_at(
                &owner,
                Command::ActivateRecruitment {
                    recruitment_id: "r".into(),
                    actor_id: "arch".into(),
                    credential_digest: digest('8'),
                    run_generation: 1,
                },
                0,
            )
            .unwrap();
        let arch = state.authenticate(&digest('8')).unwrap();
        let handoff = Command::SubmitSpecialistHandoff {
            recruitment_id: "r".into(),
            summary: "late".into(),
            evidence: evidence(),
        };
        assert_eq!(
            state
                .execute_at(&arch, handoff.clone(), 61)
                .unwrap_err()
                .code,
            "deadline_exceeded"
        );
        state.execute_at(&arch, handoff, 60).unwrap();
        // No retries allowed: a rejection closes the engagement.
        state
            .execute_at(
                &lead,
                Command::VerifyRecruitment {
                    recruitment_id: "r".into(),
                    accepted: false,
                    rationale: "no".into(),
                },
                61,
            )
            .unwrap();
        assert!(matches!(
            state.recruitments["r"].status,
            governance::RecruitmentStatus::Released {
                accepted: false,
                ..
            }
        ));
    }

    #[test]
    fn loops_hit_hard_limits_instead_of_growing() {
        let (mut state, _owner, lead, _dev, _qa) = launched();
        for index in 0..16 {
            request_approval(
                &mut state,
                &lead,
                &format!("ap{index}"),
                merge_action(&format!("b{index}")),
                10,
            )
            .unwrap();
        }
        assert_eq!(
            request_approval(&mut state, &lead, "ap-extra", merge_action("x"), 10)
                .unwrap_err()
                .code,
            "capacity_exceeded"
        );
        for index in 0..16 {
            state
                .execute_at(
                    &lead,
                    Command::RequestRecruitment {
                        recruitment_id: format!("r{index}"),
                        team_id: "t".into(),
                        specialty: "security".into(),
                        justification: "j".into(),
                        deliverable: "d".into(),
                        capabilities: vec!["review".into()],
                        dependencies: vec![],
                        stop_condition: "s".into(),
                        max_retries: 0,
                        max_minutes: 1,
                    },
                    10,
                )
                .unwrap();
        }
        let authorized = state
            .recruitments
            .values()
            .filter(|r| r.status == governance::RecruitmentStatus::Authorized)
            .count();
        assert_eq!(authorized, 1, "one slot per team; the rest queue");
        let extra = Command::RequestRecruitment {
            recruitment_id: "r-extra".into(),
            team_id: "t".into(),
            specialty: "security".into(),
            justification: "j".into(),
            deliverable: "d".into(),
            capabilities: vec!["review".into()],
            dependencies: vec![],
            stop_condition: "s".into(),
            max_retries: 0,
            max_minutes: 1,
        };
        assert_eq!(
            state.execute_at(&lead, extra, 10).unwrap_err().code,
            "capacity_exceeded"
        );
    }

    #[test]
    fn audit_events_are_hash_chained_and_tampering_is_detected() {
        let (mut state, owner, lead, _dev, _qa) = launched();
        request_approval(&mut state, &lead, "ap", merge_action("f"), 500).unwrap();
        decide(&mut state, &owner, "ap", true, 501).unwrap();
        let last = state.events.last().unwrap();
        assert_eq!(last.authority, Some(Authority::OwnerOnly));
        assert_eq!(last.at, 501);
        state.validate().unwrap();
        let mut edited = state.clone();
        edited.events[2].payload = json!({ "forged": true });
        assert_eq!(edited.validate().unwrap_err().code, "corrupt_state");
        let mut dropped = state.clone();
        dropped.events.remove(3);
        assert_eq!(dropped.validate().unwrap_err().code, "corrupt_state");
    }
    fn sha(seed: char) -> String {
        seed.to_string().repeat(40)
    }

    /// A further team `n` (>= 1) in project `p`: ids `t{n}`, `lead{n}`, `dev{n}`, `qa{n}`.
    fn add_team(state: &mut State, owner: &Actor, n: u64) -> (Actor, Actor, Actor) {
        let wide = |slot: u64| format!("{:064x}", n * 16 + slot + 0x1000);
        state
            .execute(
                owner,
                Command::CreateTeamSpec {
                    spec_id: format!("s{n}"),
                    project_id: "p".into(),
                    name: format!("Team {n}"),
                },
            )
            .unwrap();
        for (slot, (prefix, role)) in [
            ("lead", Role::Lead),
            ("dev", Role::Developer),
            ("qa", Role::Qa),
        ]
        .into_iter()
        .enumerate()
        {
            state
                .execute(
                    owner,
                    Command::EnrollActor {
                        spec_id: format!("s{n}"),
                        actor_id: format!("{prefix}{n}"),
                        role,
                        credential_digest: wide(slot as u64),
                        run_generation: 1,
                    },
                )
                .unwrap();
        }
        state
            .execute(
                owner,
                Command::LaunchTeam {
                    spec_id: format!("s{n}"),
                    team_id: format!("t{n}"),
                },
            )
            .unwrap();
        (
            state.authenticate(&wide(0)).unwrap(),
            state.authenticate(&wide(1)).unwrap(),
            state.authenticate(&wide(2)).unwrap(),
        )
    }

    fn claim(
        state: &mut State,
        owner: &Actor,
        id: &str,
        team: &str,
        path: &str,
        branch: &str,
    ) -> Result<Value> {
        state.execute(
            owner,
            Command::ClaimWorktree {
                claim_id: id.into(),
                team_id: team.into(),
                path: path.into(),
                branch: branch.into(),
                base_commit: sha('b'),
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn submit(
        state: &mut State,
        who: &Actor,
        team: &str,
        claim: &str,
        id: &str,
        commit: char,
        paths: &[&str],
        touches: &[&str],
        depends: &[&str],
    ) -> Result<Value> {
        state.execute(
            who,
            Command::SubmitIntegration {
                integration_id: id.into(),
                team_id: team.into(),
                claim_id: claim.into(),
                target_branch: "main".into(),
                commit: sha(commit),
                changed_paths: paths.iter().map(|p| p.to_string()).collect(),
                touches: touches.iter().map(|p| p.to_string()).collect(),
                depends_on: depends.iter().map(|p| p.to_string()).collect(),
                tests: evidence(),
            },
        )
    }

    fn accept(state: &mut State, qa: &Actor, id: &str) {
        state
            .execute(
                qa,
                Command::ReviewIntegration {
                    integration_id: id.into(),
                    accepted: true,
                    rationale: "tests and diff ok".into(),
                    evidence: vec![],
                },
            )
            .unwrap();
    }

    fn status_of(state: &State, id: &str) -> portfolio::IntegrationStatus {
        state.integrations[id].status.clone()
    }

    /// Lead asks, Owner approves, Owner begins the merge of exactly that integration.
    fn begin_merge(
        state: &mut State,
        owner: &Actor,
        lead: &Actor,
        team: &str,
        integration: &str,
    ) -> Result<Value> {
        let approval = format!("ap-{integration}");
        state.execute_at(
            lead,
            Command::RequestApproval {
                approval_id: approval.clone(),
                team_id: team.into(),
                action: portfolio::merge_action_for(&state.integrations[integration]),
                justification: "reviewed and tested".into(),
                alternatives: vec![],
                risks: vec![],
                ttl_seconds: 600,
            },
            10_000,
        )?;
        state.execute_at(
            owner,
            Command::DecideApproval {
                approval_id: approval.clone(),
                approve: true,
                rationale: "ok".into(),
            },
            10_001,
        )?;
        state.execute_at(
            owner,
            Command::BeginMerge {
                integration_id: integration.into(),
                approval_id: approval,
            },
            10_002,
        )
    }

    #[test]
    fn two_teams_edit_independent_areas_and_each_lands_through_the_owner() {
        let (mut state, owner, lead, dev, qa) = launched();
        let (lead2, dev2, qa2) = add_team(&mut state, &owner, 2);
        claim(&mut state, &owner, "w1", "t", "/work/a", "team-a").unwrap();
        claim(&mut state, &owner, "w2", "t2", "/work/b", "team-b").unwrap();
        submit(
            &mut state,
            &dev,
            "t",
            "w1",
            "i1",
            '1',
            &["src/a/x.rs"],
            &[],
            &[],
        )
        .unwrap();
        submit(
            &mut state,
            &dev2,
            "t2",
            "w2",
            "i2",
            '2',
            &["src/b/y.rs"],
            &[],
            &[],
        )
        .unwrap();
        assert!(
            state.conflicts.is_empty(),
            "disjoint areas raise no conflict"
        );
        // Isolation: neither team sees the other's chat, integrations or worktree, but both see project knowledge.
        state
            .execute(
                &lead,
                Command::AppendChat {
                    team_id: "t".into(),
                    message_id: "c1".into(),
                    body: "team one only".into(),
                    task_id: None,
                    audience: Audience::Team,
                },
            )
            .unwrap();
        state
            .execute(
                &owner,
                Command::PutKnowledge {
                    team_id: "t".into(),
                    knowledge_id: "k-proj".into(),
                    scope: KnowledgeScope::Project {
                        project_id: "p".into(),
                    },
                    title: "ADR-1".into(),
                    body: "use queues".into(),
                    evidence: vec![],
                    knowledge_kind: KnowledgeKind::Note,
                },
            )
            .unwrap();
        state
            .execute(
                &lead,
                Command::PutKnowledge {
                    team_id: "t".into(),
                    knowledge_id: "k-team".into(),
                    scope: KnowledgeScope::Team {
                        team_id: "t".into(),
                    },
                    title: "private".into(),
                    body: "wip".into(),
                    evidence: vec![],
                    knowledge_kind: KnowledgeKind::Note,
                },
            )
            .unwrap();
        let view = state.snapshot(&lead2, None).unwrap();
        assert_eq!(view["messages"], json!([]));
        assert_eq!(view["integrations"].as_array().unwrap().len(), 1);
        assert_eq!(view["integrations"][0]["id"], "i2");
        assert_eq!(view["worktrees"].as_array().unwrap().len(), 1);
        let knowledge: Vec<&str> = view["knowledge"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|k| k["id"].as_str())
            .collect();
        assert_eq!(knowledge, ["k-proj"]);
        assert_eq!(
            state.snapshot(&lead2, Some("t")).unwrap_err().code,
            "forbidden"
        );
        // QA accepts both; only the Owner moves them, and each merge needs its own approval.
        accept(&mut state, &qa, "i1");
        accept(&mut state, &qa2, "i2");
        assert_eq!(status_of(&state, "i1"), portfolio::IntegrationStatus::Ready);
        assert_eq!(
            state
                .execute(
                    &lead,
                    Command::BeginMerge {
                        integration_id: "i1".into(),
                        approval_id: "x".into()
                    }
                )
                .unwrap_err()
                .code,
            "forbidden"
        );
        begin_merge(&mut state, &owner, &lead, "t", "i1").unwrap();
        // i1's approval cannot be used for i2, and a team's approval cannot merge another team's work.
        state
            .execute_at(
                &lead2,
                Command::RequestApproval {
                    approval_id: "ap-wrong".into(),
                    team_id: "t2".into(),
                    action: portfolio::merge_action_for(&state.integrations["i1"]),
                    justification: "j".into(),
                    alternatives: vec![],
                    risks: vec![],
                    ttl_seconds: 600,
                },
                10_000,
            )
            .unwrap();
        state
            .execute_at(
                &owner,
                Command::DecideApproval {
                    approval_id: "ap-wrong".into(),
                    approve: true,
                    rationale: "ok".into(),
                },
                10_001,
            )
            .unwrap();
        assert_eq!(
            state
                .execute_at(
                    &owner,
                    Command::BeginMerge {
                        integration_id: "i2".into(),
                        approval_id: "ap-wrong".into()
                    },
                    10_002
                )
                .unwrap_err()
                .code,
            "approval_mismatch"
        );
        state
            .execute(
                &owner,
                Command::CompleteMerge {
                    integration_id: "i1".into(),
                    merge_commit: sha('c'),
                },
            )
            .unwrap();
        begin_merge(&mut state, &owner, &lead2, "t2", "i2").unwrap();
        state
            .execute(
                &owner,
                Command::CompleteMerge {
                    integration_id: "i2".into(),
                    merge_commit: sha('d'),
                },
            )
            .unwrap();
        assert!(matches!(
            status_of(&state, "i2"),
            portfolio::IntegrationStatus::Merged { .. }
        ));
        state.validate().unwrap();
    }

    #[test]
    fn two_teams_touching_one_path_become_an_explicit_review_request() {
        let (mut state, owner, lead, dev, qa) = launched();
        let (lead2, dev2, qa2) = add_team(&mut state, &owner, 2);
        claim(&mut state, &owner, "w1", "t", "/work/a", "team-a").unwrap();
        claim(&mut state, &owner, "w2", "t2", "/work/b", "team-b").unwrap();
        submit(
            &mut state,
            &dev,
            "t",
            "w1",
            "i1",
            '1',
            &["src/lib.rs", "src/a.rs"],
            &[],
            &[],
        )
        .unwrap();
        submit(
            &mut state,
            &dev2,
            "t2",
            "w2",
            "i2",
            '2',
            &["src/lib.rs"],
            &[],
            &[],
        )
        .unwrap();
        let conflict = &state.conflicts["conflict-i1-i2-pathoverlap"];
        assert_eq!(conflict.detail, ["src/lib.rs"]);
        assert_eq!(conflict.team_ids, ["t".to_owned(), "t2".to_owned()]);
        assert_eq!(
            status_of(&state, "i1"),
            portfolio::IntegrationStatus::Blocked
        );
        // Both teams see it, and the other team's Lead gets a structured notice.
        for who in [&lead, &lead2] {
            assert_eq!(
                state.snapshot(who, None).unwrap()["attention"]["open_conflicts"],
                json!(["conflict-i1-i2-pathoverlap"])
            );
        }
        assert_eq!(state.notices["notice-i2-t"].to_team_id, "t");
        // QA acceptance does not clear a conflict; the Owner cannot merge past it.
        accept(&mut state, &qa, "i1");
        accept(&mut state, &qa2, "i2");
        assert_eq!(
            status_of(&state, "i1"),
            portfolio::IntegrationStatus::Blocked
        );
        assert_eq!(
            state
                .execute(
                    &owner,
                    Command::BeginMerge {
                        integration_id: "i1".into(),
                        approval_id: "none".into()
                    }
                )
                .unwrap_err()
                .code,
            "not_ready"
        );
        assert_eq!(
            state
                .execute(
                    &lead,
                    Command::ResolveConflict {
                        conflict_id: "conflict-i1-i2-pathoverlap".into(),
                        rationale: "mine".into()
                    }
                )
                .unwrap_err()
                .code,
            "forbidden"
        );
        assert_eq!(
            state
                .execute(
                    &lead2,
                    Command::AcknowledgeNotice {
                        notice_id: "notice-i2-t".into()
                    }
                )
                .unwrap_err()
                .code,
            "forbidden",
            "only the receiving Lead acknowledges"
        );
        state
            .execute(
                &lead,
                Command::AcknowledgeNotice {
                    notice_id: "notice-i2-t".into(),
                },
            )
            .unwrap();
        state
            .execute(
                &owner,
                Command::ResolveConflict {
                    conflict_id: "conflict-i1-i2-pathoverlap".into(),
                    rationale: "i1 lands first, i2 rebases".into(),
                },
            )
            .unwrap();
        assert_eq!(status_of(&state, "i1"), portfolio::IntegrationStatus::Ready);
        begin_merge(&mut state, &owner, &lead, "t", "i1").unwrap();
        state.validate().unwrap();
    }

    #[test]
    fn disjoint_files_still_conflict_through_a_shared_public_surface() {
        let (mut state, owner, _lead, dev, _qa) = launched();
        let (_lead2, dev2, _qa2) = add_team(&mut state, &owner, 2);
        state
            .execute(
                &owner,
                Command::RegisterSurface {
                    surface_id: "plugin-api".into(),
                    project_id: "p".into(),
                    path_prefixes: vec!["src/api".into()],
                },
            )
            .unwrap();
        claim(&mut state, &owner, "w1", "t", "/work/a", "team-a").unwrap();
        claim(&mut state, &owner, "w2", "t2", "/work/b", "team-b").unwrap();
        // Team 1 edits the API files; team 2 edits only CLI files but relies on the API.
        submit(
            &mut state,
            &dev,
            "t",
            "w1",
            "i1",
            '1',
            &["src/api/schema.rs"],
            &[],
            &[],
        )
        .unwrap();
        assert!(
            state.integrations["i1"].touches.contains("plugin-api"),
            "the registry, not the author, decides what is touched"
        );
        assert_eq!(
            submit(
                &mut state,
                &dev2,
                "t2",
                "w2",
                "ix",
                '3',
                &["src/cli/a.rs"],
                &[],
                &["no-such-surface"]
            )
            .unwrap_err()
            .code,
            "unknown_surface"
        );
        submit(
            &mut state,
            &dev2,
            "t2",
            "w2",
            "i2",
            '2',
            &["src/cli/plugin.rs"],
            &[],
            &["plugin-api"],
        )
        .unwrap();
        assert!(state
            .conflicts
            .contains_key("conflict-i1-i2-dependencyimpact"));
        assert!(!state.conflicts.contains_key("conflict-i1-i2-pathoverlap"));
        assert_eq!(state.notices["notice-i2-t"].surfaces, ["plugin-api"]);
        assert_eq!(
            status_of(&state, "i2"),
            portfolio::IntegrationStatus::Blocked
        );
        // Two teams both changing the surface is a different, also explicit, conflict.
        let (_l3, dev3, _q3) = add_team(&mut state, &owner, 3);
        claim(&mut state, &owner, "w3", "t3", "/work/c", "team-c").unwrap();
        submit(
            &mut state,
            &dev3,
            "t3",
            "w3",
            "i3",
            '4',
            &["src/api/other.rs"],
            &[],
            &[],
        )
        .unwrap();
        assert!(state.conflicts.contains_key("conflict-i1-i3-sharedsurface"));
        state.validate().unwrap();
    }

    #[test]
    fn interrupted_merge_is_reconciled_by_the_owner_and_never_assumed() {
        let (mut state, owner, lead, dev, qa) = launched();
        claim(&mut state, &owner, "w1", "t", "/work/a", "team-a").unwrap();
        submit(
            &mut state,
            &dev,
            "t",
            "w1",
            "i1",
            '1',
            &["src/a.rs"],
            &[],
            &[],
        )
        .unwrap();
        accept(&mut state, &qa, "i1");
        begin_merge(&mut state, &owner, &lead, "t", "i1").unwrap();
        // The server dies mid-merge and restarts from its durable state.
        let mut restarted: State =
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert!(restarted.recover().unwrap());
        assert!(matches!(
            status_of(&restarted, "i1"),
            portfolio::IntegrationStatus::MergeUnknown { .. }
        ));
        assert_eq!(
            restarted.snapshot(&owner, None).unwrap()["attention"]["merge_unknown"],
            json!(["i1"])
        );
        assert!(!restarted.recover().unwrap(), "recovery is idempotent");
        // Detaching or restarting never suspends a team.
        assert_eq!(restarted.teams["t"].status, TeamStatus::Active);
        // The review still stands, but the spent approval cannot be reused; the Owner records what really happened.
        restarted
            .execute(
                &owner,
                Command::AbortMerge {
                    integration_id: "i1".into(),
                    reason: "merge did not land".into(),
                },
            )
            .unwrap();
        assert_eq!(
            status_of(&restarted, "i1"),
            portfolio::IntegrationStatus::Ready
        );
        assert_eq!(
            restarted
                .execute_at(
                    &owner,
                    Command::BeginMerge {
                        integration_id: "i1".into(),
                        approval_id: "ap-i1".into()
                    },
                    10_003
                )
                .unwrap_err()
                .code,
            "approval_used"
        );
        restarted.validate().unwrap();
        // Alternatively the merge did land.
        let mut landed: State =
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        landed.recover().unwrap();
        landed
            .execute(
                &owner,
                Command::CompleteMerge {
                    integration_id: "i1".into(),
                    merge_commit: sha('e'),
                },
            )
            .unwrap();
        assert!(matches!(
            status_of(&landed, "i1"),
            portfolio::IntegrationStatus::Merged { .. }
        ));
    }

    #[test]
    fn renaming_a_team_changes_only_its_label() {
        let (mut state, owner, lead, dev, _qa) = launched();
        let (lead2, _dev2, _qa2) = add_team(&mut state, &owner, 2);
        create_task(&mut state, &lead, "task", &[]).unwrap();
        claim(&mut state, &owner, "w1", "t", "/work/a", "team-a").unwrap();
        state
            .execute(
                &lead,
                Command::RenameTeam {
                    team_id: "t".into(),
                    name: "Platform".into(),
                },
            )
            .unwrap();
        assert_eq!(state.teams["t"].name, "Platform");
        assert_eq!(state.tasks["task"].team_id, "t");
        assert_eq!(state.worktrees["w1"].team_id, "t");
        assert_eq!(
            state
                .execute(
                    &lead2,
                    Command::RenameTeam {
                        team_id: "t".into(),
                        name: "Mine".into()
                    }
                )
                .unwrap_err()
                .code,
            "forbidden"
        );
        assert_eq!(
            state
                .execute(
                    &dev,
                    Command::RenameTeam {
                        team_id: "t".into(),
                        name: "Mine".into()
                    }
                )
                .unwrap_err()
                .code,
            "forbidden"
        );
        // Work addressed by id keeps working under the new name.
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
                &owner,
                Command::RenameTeam {
                    team_id: "t".into(),
                    name: "Platform v2".into(),
                },
            )
            .unwrap();
        assert_eq!(
            state.snapshot(&owner, Some("t")).unwrap()["teams"][0]["name"],
            "Platform v2"
        );
        state.validate().unwrap();
    }

    #[test]
    fn worktrees_never_overlap_and_used_ones_are_preserved() {
        let (mut state, owner, lead, dev, _qa) = launched();
        let (_lead2, _dev2, _qa2) = add_team(&mut state, &owner, 2);
        claim(&mut state, &owner, "w1", "t", "/work/a", "team-a").unwrap();
        for (path, branch) in [
            ("/work/a", "other"),
            ("/work/a/inner", "other"),
            ("/work", "other"),
            ("/work/b", "team-a"),
        ] {
            assert_eq!(
                claim(&mut state, &owner, "wx", "t2", path, branch)
                    .unwrap_err()
                    .code,
                "worktree_conflict",
                "{path} {branch}"
            );
        }
        for path in ["relative/x", "/work/../etc", "/work//x", "/"] {
            assert_eq!(
                claim(&mut state, &owner, "wx", "t2", path, "b")
                    .unwrap_err()
                    .code,
                "invalid_params",
                "{path}"
            );
        }
        claim(&mut state, &owner, "w-spare", "t2", "/work/spare", "spare").unwrap();
        // Reported paths stay inside the worktree.
        for path in [
            "../escape.rs",
            "/abs.rs",
            ".git/config",
            "a/../b.rs",
            "dir//file.rs",
        ] {
            assert_eq!(
                submit(&mut state, &dev, "t", "w1", "ix", '1', &[path], &[], &[])
                    .unwrap_err()
                    .code,
                "invalid_params",
                "{path}"
            );
        }
        // A claim of another team cannot be used to submit.
        assert_eq!(
            submit(
                &mut state,
                &dev,
                "t",
                "w-spare",
                "ix",
                '1',
                &["a.rs"],
                &[],
                &[]
            )
            .unwrap_err()
            .code,
            "invalid_reference"
        );
        // Cleanup: refused while active, approved for an untouched released worktree, never for a used one.
        assert_eq!(
            state
                .execute(
                    &owner,
                    Command::ApproveWorktreeCleanup {
                        claim_id: "w-spare".into()
                    }
                )
                .unwrap_err()
                .code,
            "worktree_in_use"
        );
        state
            .execute(
                &owner,
                Command::ReleaseWorktree {
                    claim_id: "w-spare".into(),
                },
            )
            .unwrap();
        state
            .execute(
                &owner,
                Command::ApproveWorktreeCleanup {
                    claim_id: "w-spare".into(),
                },
            )
            .unwrap();
        submit(&mut state, &dev, "t", "w1", "i1", '1', &["a.rs"], &[], &[]).unwrap();
        assert_eq!(
            state
                .execute(
                    &lead,
                    Command::ReleaseWorktree {
                        claim_id: "w1".into()
                    }
                )
                .unwrap_err()
                .code,
            "integration_open"
        );
        state
            .execute(
                &lead,
                Command::CancelIntegration {
                    integration_id: "i1".into(),
                    reason: "superseded".into(),
                },
            )
            .unwrap();
        state
            .execute(
                &lead,
                Command::ReleaseWorktree {
                    claim_id: "w1".into(),
                },
            )
            .unwrap();
        assert_eq!(
            state
                .execute(
                    &owner,
                    Command::ApproveWorktreeCleanup {
                        claim_id: "w1".into()
                    }
                )
                .unwrap_err()
                .code,
            "worktree_has_work"
        );
        // Released paths and branches can be claimed again.
        claim(&mut state, &owner, "w3", "t2", "/work/a", "team-a").unwrap();
        state.validate().unwrap();
    }

    #[test]
    fn suspend_resume_and_cancel_affect_only_that_team_and_destroy_nothing() {
        let (mut state, owner, lead, dev, qa) = launched();
        let (lead2, dev2, _qa2) = add_team(&mut state, &owner, 2);
        claim(&mut state, &owner, "w1", "t", "/work/a", "team-a").unwrap();
        submit(&mut state, &dev, "t", "w1", "i1", '1', &["a.rs"], &[], &[]).unwrap();
        state
            .execute(
                &lead,
                Command::SuspendTeam {
                    team_id: "t".into(),
                    reason: "pause".into(),
                },
            )
            .unwrap();
        assert_eq!(
            create_task(&mut state, &lead, "task", &[])
                .unwrap_err()
                .code,
            "team_suspended"
        );
        assert_eq!(
            state
                .execute(
                    &qa,
                    Command::ReviewIntegration {
                        integration_id: "i1".into(),
                        accepted: true,
                        rationale: "r".into(),
                        evidence: vec![]
                    }
                )
                .unwrap_err()
                .code,
            "team_suspended"
        );
        // The other team is unaffected, and the suspended one can still read and chat is paused.
        state
            .execute(
                &lead2,
                Command::CreateTask {
                    team_id: "t2".into(),
                    task_id: "task2".into(),
                    title: "x".into(),
                    description: "d".into(),
                    developer_id: "dev2".into(),
                    dependencies: vec![],
                    assignment_message_id: "m2".into(),
                    dispatch: intent("a2", 1),
                },
            )
            .unwrap();
        state.snapshot(&lead, None).unwrap();
        state
            .execute(
                &lead,
                Command::ResumeTeam {
                    team_id: "t".into(),
                },
            )
            .unwrap();
        create_task(&mut state, &lead, "task", &[]).unwrap();
        // Cancel: queued work withdrawn, worktree released but kept, approvals closed, nothing erased.
        state
            .execute_at(
                &lead,
                Command::RequestApproval {
                    approval_id: "ap".into(),
                    team_id: "t".into(),
                    action: merge_action("x"),
                    justification: "j".into(),
                    alternatives: vec![],
                    risks: vec![],
                    ttl_seconds: 600,
                },
                10,
            )
            .unwrap();
        state
            .execute(
                &lead,
                Command::RequestRecruitment {
                    recruitment_id: "r".into(),
                    team_id: "t".into(),
                    specialty: "security".into(),
                    justification: "j".into(),
                    deliverable: "d".into(),
                    capabilities: vec!["review".into()],
                    dependencies: vec![],
                    stop_condition: "s".into(),
                    max_retries: 0,
                    max_minutes: 5,
                },
            )
            .unwrap();
        state
            .execute(
                &owner,
                Command::ActivateRecruitment {
                    recruitment_id: "r".into(),
                    actor_id: "sec".into(),
                    credential_digest: digest('7'),
                    run_generation: 1,
                },
            )
            .unwrap();
        state
            .execute(
                &owner,
                Command::CancelTeam {
                    team_id: "t".into(),
                    reason: "descoped".into(),
                },
            )
            .unwrap();
        assert!(matches!(
            status_of(&state, "i1"),
            portfolio::IntegrationStatus::Cancelled { .. }
        ));
        assert_eq!(
            state.worktrees["w1"].status,
            portfolio::ClaimStatus::Released
        );
        assert!(
            state.worktrees["w1"].used,
            "the worktree stays marked as holding work"
        );
        assert!(matches!(
            state.approvals["ap"].status,
            governance::ApprovalStatus::Denied { .. }
        ));
        assert!(matches!(
            state.recruitments["r"].status,
            governance::RecruitmentStatus::Cancelled { .. }
        ));
        assert_eq!(
            state.authenticate(&digest('7')).unwrap_err().code,
            "unauthorized"
        );
        assert!(
            state.tasks.contains_key("task") && state.messages.contains_key("m-task"),
            "history is kept"
        );
        assert_eq!(
            create_task(&mut state, &lead, "task-late", &[])
                .unwrap_err()
                .code,
            "team_cancelled"
        );
        assert_eq!(
            state
                .execute(
                    &owner,
                    Command::ResumeTeam {
                        team_id: "t".into()
                    }
                )
                .unwrap_err()
                .code,
            "invalid_state"
        );
        // The other team still works.
        state
            .execute(
                &dev2,
                Command::AcceptTask {
                    team_id: "t2".into(),
                    task_id: "task2".into(),
                    acceptance_id: "acc2".into(),
                    note: "n".into(),
                },
            )
            .unwrap();
        state.validate().unwrap();
    }

    #[test]
    fn concurrency_is_bounded_across_the_whole_project() {
        let (mut state, owner, _lead, _dev, _qa) = launched();
        for n in 2..=4 {
            add_team(&mut state, &owner, n);
        }
        // A fifth team is refused (4 active), until one is suspended.
        state
            .execute(
                &owner,
                Command::CreateTeamSpec {
                    spec_id: "s5".into(),
                    project_id: "p".into(),
                    name: "Five".into(),
                },
            )
            .unwrap();
        for (slot, (prefix, role)) in [
            ("lead", Role::Lead),
            ("dev", Role::Developer),
            ("qa", Role::Qa),
        ]
        .into_iter()
        .enumerate()
        {
            state
                .execute(
                    &owner,
                    Command::EnrollActor {
                        spec_id: "s5".into(),
                        actor_id: format!("{prefix}5"),
                        role,
                        credential_digest: format!("{:064x}", 0x9000 + slot),
                        run_generation: 1,
                    },
                )
                .unwrap();
        }
        assert_eq!(
            state
                .execute(
                    &owner,
                    Command::LaunchTeam {
                        spec_id: "s5".into(),
                        team_id: "t5".into()
                    }
                )
                .unwrap_err()
                .code,
            "project_capacity"
        );
        state
            .execute(
                &owner,
                Command::SuspendTeam {
                    team_id: "t2".into(),
                    reason: "make room".into(),
                },
            )
            .unwrap();
        state
            .execute(
                &owner,
                Command::LaunchTeam {
                    spec_id: "s5".into(),
                    team_id: "t5".into(),
                },
            )
            .unwrap();
        assert_eq!(
            state
                .execute(
                    &owner,
                    Command::ResumeTeam {
                        team_id: "t2".into()
                    }
                )
                .unwrap_err()
                .code,
            "project_capacity"
        );
        // The specialist cap is project-wide, not per team.
        let recruit = |team: &str, id: &str| Command::RequestRecruitment {
            recruitment_id: id.into(),
            team_id: team.into(),
            specialty: "security".into(),
            justification: "j".into(),
            deliverable: "d".into(),
            capabilities: vec!["review".into()],
            dependencies: vec![],
            stop_condition: "s".into(),
            max_retries: 0,
            max_minutes: 5,
        };
        let leads: Vec<Actor> = (3..=4)
            .map(|n| {
                state
                    .authenticate(&format!("{:064x}", n * 16 + 0x1000))
                    .unwrap()
            })
            .collect();
        for (lead, (team, id)) in leads.iter().zip([("t3", "r3"), ("t4", "r4")]) {
            state.execute(lead, recruit(team, id)).unwrap();
        }
        let first = state.authenticate(&digest('1')).unwrap();
        state.execute(&first, recruit("t", "r1")).unwrap();
        assert_eq!(
            state.recruitments["r1"].status,
            governance::RecruitmentStatus::Queued
        );
        state.validate().unwrap();
    }

    #[test]
    fn a_phase_two_store_still_loads_and_runs() {
        let (state, owner, lead, _dev, _qa) = launched();
        let mut value = serde_json::to_value(&state).unwrap();
        let object = value.as_object_mut().unwrap();
        for key in [
            "surfaces",
            "worktrees",
            "integrations",
            "conflicts",
            "notices",
        ] {
            object.remove(key);
        }
        object["teams"]["t"]
            .as_object_mut()
            .unwrap()
            .remove("status");
        let mut legacy: State = serde_json::from_value(value).unwrap();
        legacy.validate().unwrap();
        assert_eq!(legacy.teams["t"].status, TeamStatus::Active);
        create_task(&mut legacy, &lead, "task", &[]).unwrap();
        legacy.snapshot(&owner, None).unwrap();
    }
    fn chat(state: &mut State, who: &Actor, id: &str, audience: Audience) -> Result<Value> {
        state.execute(
            who,
            Command::AppendChat {
                team_id: "t".into(),
                message_id: id.into(),
                body: format!("body of {id}"),
                task_id: None,
                audience,
            },
        )
    }

    fn visible_ids(state: &State, viewer: &Actor) -> Vec<String> {
        let snapshot = state.snapshot(viewer, None).unwrap();
        snapshot["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["id"].as_str().map(str::to_owned))
            .collect()
    }

    #[test]
    fn role_and_direct_messages_reach_only_their_audience() {
        let (mut state, owner, lead, dev, qa) = launched();
        let (lead2, _dev2, _qa2) = add_team(&mut state, &owner, 2);
        chat(&mut state, &lead, "all", Audience::Team).unwrap();
        chat(
            &mut state,
            &lead,
            "to-dev-role",
            Audience::Role {
                role: Role::Developer,
            },
        )
        .unwrap();
        chat(
            &mut state,
            &dev,
            "dev-to-qa",
            Audience::Actor {
                actor_id: "qa".into(),
            },
        )
        .unwrap();
        assert_eq!(visible_ids(&state, &lead), ["all", "to-dev-role"]);
        assert_eq!(
            visible_ids(&state, &dev),
            ["all", "to-dev-role", "dev-to-qa"]
        );
        assert_eq!(visible_ids(&state, &qa), ["all", "dev-to-qa"]);
        assert_eq!(
            visible_ids(&state, &owner).len(),
            3,
            "the Owner reads every channel"
        );
        assert!(
            visible_ids(&state, &lead2).is_empty(),
            "another team sees none"
        );
        // The audience is part of the record, so clients can label it.
        let snapshot = state.snapshot(&dev, None).unwrap();
        let direct = snapshot["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == "dev-to-qa")
            .unwrap();
        assert_eq!(
            direct["audience"],
            json!({ "to": "actor", "actor_id": "qa" })
        );
        // Bad audiences are refused by the server, not by whoever typed an @.
        assert_eq!(
            chat(
                &mut state,
                &lead,
                "x1",
                Audience::Role { role: Role::Owner }
            )
            .unwrap_err()
            .code,
            "invalid_params"
        );
        assert_eq!(
            chat(
                &mut state,
                &lead,
                "x2",
                Audience::Actor {
                    actor_id: "lead2".into()
                }
            )
            .unwrap_err()
            .code,
            "invalid_reference",
            "no direct message across teams"
        );
        assert_eq!(
            chat(
                &mut state,
                &lead,
                "x3",
                Audience::Actor {
                    actor_id: "ghost".into()
                }
            )
            .unwrap_err()
            .code,
            "not_found"
        );
        state.validate().unwrap();
    }

    #[test]
    fn an_agent_is_bound_to_a_pane_by_the_owner_or_its_own_lead() {
        let (mut state, owner, lead, dev, _qa) = launched();
        let (lead2, _dev2, _qa2) = add_team(&mut state, &owner, 2);
        let bind = |who: &Actor, state: &mut State, agent: &str, pane: &str| {
            state.execute(
                who,
                Command::BindAgentPane {
                    actor_id: agent.into(),
                    pane_id: pane.into(),
                    provider: Some("claude".into()),
                },
            )
        };
        bind(&lead, &mut state, "dev", "pane-7").unwrap();
        assert_eq!(
            bind(&dev, &mut state, "dev", "pane-8").unwrap_err().code,
            "forbidden"
        );
        assert_eq!(
            bind(&lead2, &mut state, "qa", "pane-9").unwrap_err().code,
            "forbidden",
            "not their team"
        );
        assert_eq!(
            bind(&owner, &mut state, "qa", "pane-7").unwrap_err().code,
            "already_exists",
            "one pane, one agent"
        );
        assert_eq!(
            bind(&owner, &mut state, "owner", "pane-1")
                .unwrap_err()
                .code,
            "invalid_reference"
        );
        bind(&owner, &mut state, "dev", "pane-70").unwrap();
        // Visible to the team and the Owner, not to another team; it survives a restart.
        assert_eq!(
            state.snapshot(&lead, None).unwrap()["bindings"][0]["pane_id"],
            "pane-70"
        );
        assert_eq!(state.snapshot(&lead2, None).unwrap()["bindings"], json!([]));
        let restored: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        restored.validate().unwrap();
        assert_eq!(
            restored.pane_bindings["dev"].provider.as_deref(),
            Some("claude")
        );
        state
            .execute(
                &lead,
                Command::UnbindAgentPane {
                    actor_id: "dev".into(),
                },
            )
            .unwrap();
        assert!(state.pane_bindings.is_empty());
        assert_eq!(
            state
                .execute(
                    &lead,
                    Command::UnbindAgentPane {
                        actor_id: "dev".into()
                    }
                )
                .unwrap_err()
                .code,
            "not_found"
        );
    }

    #[test]
    fn approvals_carry_bounded_alternatives_and_risks_outside_the_digest() {
        let (mut state, owner, lead, _dev, _qa) = launched();
        let request =
            |id: &str, alternatives: Vec<String>, risks: Vec<String>| Command::RequestApproval {
                approval_id: id.into(),
                team_id: "t".into(),
                action: merge_action("f"),
                justification: "j".into(),
                alternatives,
                risks,
                ttl_seconds: 600,
            };
        state
            .execute_at(
                &lead,
                request(
                    "a",
                    vec!["wait for the next release".into()],
                    vec!["touches the public API".into()],
                ),
                10,
            )
            .unwrap();
        state
            .execute_at(&lead, request("b", vec![], vec![]), 10)
            .unwrap();
        assert_eq!(state.approvals["a"].risks, ["touches the public API"]);
        assert_eq!(
            state.approvals["a"].action_digest, state.approvals["b"].action_digest,
            "context is for the Owner and is not what gets approved"
        );
        assert_eq!(
            state
                .execute_at(&lead, request("c", vec!["x".into(); 9], vec![]), 10)
                .unwrap_err()
                .code,
            "invalid_params"
        );
        decide(&mut state, &owner, "a", true, 11).unwrap();
    }
    fn event_ids(page: &Value) -> Vec<u64> {
        page["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["sequence"].as_u64().unwrap())
            .collect()
    }

    #[test]
    fn compaction_keeps_the_chain_verifiable_and_tells_old_cursors_to_resync() {
        let (mut state, owner, lead, _dev, _qa) = launched();
        for i in 0..30 {
            chat(&mut state, &lead, &format!("c{i}"), Audience::Team).unwrap();
        }
        let before = state.next_sequence - 1;
        state.compact_events(10);
        state.validate().unwrap();
        assert_eq!((state.events.len(), state.events_base), (10, before - 10));
        assert_eq!(
            state.next_sequence - 1,
            before,
            "the watermark does not move"
        );
        // Appending after compaction continues the sequence and the chain.
        chat(&mut state, &lead, "after", Audience::Team).unwrap();
        state.validate().unwrap();
        assert_eq!(state.events.last().unwrap().sequence, before + 1);
        // A cursor inside the retained log pages normally and in order.
        let page = state.event_page(&owner, "stream", before - 5, 100).unwrap();
        assert_eq!(
            event_ids(&page),
            (before - 4..=before + 1).collect::<Vec<_>>()
        );
        // A cursor older than the log is told where to resume, never handed a gap.
        let error = state.event_page(&owner, "stream", 3, 100).unwrap_err();
        assert_eq!(error.code, "cursor_expired");
        let details = error.details.unwrap();
        assert_eq!(details["retained_from"], before - 10 + 1);
        assert_eq!(details["watermark"], before + 1);
        // Resyncing from a snapshot's watermark loses nothing after it.
        let snapshot = state.snapshot(&owner, None).unwrap();
        let watermark = snapshot["last_event_sequence"].as_u64().unwrap();
        chat(&mut state, &lead, "later", Audience::Team).unwrap();
        let resumed = state.event_page(&owner, "stream", watermark, 100).unwrap();
        assert_eq!(event_ids(&resumed), vec![watermark + 1]);
        // And a compacted store survives a restart and still validates.
        let restored: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored.events_base, state.events_base);
        // Tampering with what is left is still caught.
        let mut tampered = state.clone();
        tampered.events[0].payload = json!({"forged": true});
        assert_eq!(tampered.validate().unwrap_err().code, "corrupt_state");
    }

    #[test]
    fn compaction_triggers_past_the_limit_and_the_limits_are_sane() {
        const { assert!(RETAIN_EVENTS_KEEP < RETAIN_EVENTS_MAX) };
        const { assert!(RETAIN_EVENTS_MAX + 100 < MAX_RECORDS) };
        let (mut state, _owner, lead, _dev, _qa) = launched();
        for i in 0..40 {
            chat(&mut state, &lead, &format!("m{i}"), Audience::Team).unwrap();
        }
        let total = state.events.len();
        state.compact_if_over(total + 5, 20);
        assert_eq!(
            state.events.len(),
            total,
            "under the limit nothing is dropped"
        );
        state.compact_if_over(total - 1, 20);
        assert_eq!(state.events.len(), 20);
        state.validate().unwrap();
    }

    #[test]
    fn events_about_scoped_messages_are_scoped_too() {
        let (mut state, owner, lead, dev, qa) = launched();
        let (lead2, _dev2, _qa2) = add_team(&mut state, &owner, 2);
        chat(&mut state, &lead, "open", Audience::Team).unwrap();
        chat(
            &mut state,
            &lead,
            "to-dev",
            Audience::Role {
                role: Role::Developer,
            },
        )
        .unwrap();
        chat(
            &mut state,
            &dev,
            "dev-to-qa",
            Audience::Actor {
                actor_id: "qa".into(),
            },
        )
        .unwrap();
        let seen = |viewer: &Actor| -> Vec<String> {
            state.event_page(viewer, "stream", 0, 100).unwrap()["events"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["kind"] == "chat_message_appended")
                .map(|e| e["payload"]["message_id"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(seen(&owner), ["open", "to-dev", "dev-to-qa"]);
        assert_eq!(
            seen(&lead),
            ["open", "to-dev"],
            "lead sent the role message but was not party to dev-to-qa"
        );
        assert_eq!(seen(&dev), ["open", "to-dev", "dev-to-qa"]);
        assert_eq!(
            seen(&qa),
            ["open", "dev-to-qa"],
            "QA is not the developer role"
        );
        assert!(seen(&lead2).is_empty());
    }

    #[test]
    fn receipts_are_trimmed_oldest_first() {
        let (mut state, _owner, _lead, _dev, _qa) = launched();
        for i in 0..(MAX_RECEIPTS + 10) {
            state.receipts.insert(
                format!("k{i}"),
                StoredReceipt {
                    digest: String::new(),
                    result: json!({}),
                    revision: i as u64,
                },
            );
        }
        state.trim_receipts();
        assert_eq!(state.receipts.len(), MAX_RECEIPTS);
        assert!(
            !state.receipts.contains_key("k0")
                && state
                    .receipts
                    .contains_key(&format!("k{}", MAX_RECEIPTS + 9))
        );
    }

    #[test]
    fn an_event_without_restrictions_hashes_exactly_as_it_did_before_they_existed() {
        let event = Event {
            sequence: 3,
            revision: 3,
            actor_id: "owner".into(),
            project_id: Some("p".into()),
            team_id: None,
            kind: "project_created".into(),
            payload: json!({"project_id": "p"}),
            authority: Some(Authority::OwnerOnly),
            at: 77,
            restricted_to: None,
            prev_hash: "abc".into(),
            hash: String::new(),
        };
        // The pre-restriction formula, spelled out: ten fields, no eleventh.
        let legacy = serde_json::to_vec(&json!([
            "abc", 3, 3, "owner", "p", null, "project_created", {"project_id": "p"}, "owner_only", 77
        ]))
        .unwrap();
        let mut digest = Sha256::new();
        digest.update(b"moo-teams-event-v1\0");
        digest.update(&legacy);
        assert_eq!(event.compute_hash(), format!("{:x}", digest.finalize()));
        let restricted = Event {
            restricted_to: Some(vec!["a".into()]),
            ..event.clone()
        };
        assert_ne!(
            restricted.compute_hash(),
            event.compute_hash(),
            "the restriction is covered by the chain"
        );
    }
    // ---- Phase 6: planning, memory, disputes, usage, controls --------------------------------

    fn declare(
        state: &mut State,
        owner: &Actor,
        id: &str,
        caps: &[&str],
        metering: Metering,
        hard: bool,
    ) {
        state
            .execute(
                owner,
                Command::DeclareProvider {
                    provider_id: id.into(),
                    label: id.into(),
                    capabilities: caps.iter().map(|c| c.to_string()).collect(),
                    models: vec![ModelDecl {
                        id: "m1".into(),
                        capabilities: vec![],
                    }],
                    metering,
                    hard_spend_limit: hard,
                },
            )
            .unwrap();
    }

    fn plan_for(state: &State, goal: &str) -> planning::Plan {
        state.plan_goal("p", goal, 1)
    }

    #[test]
    fn the_planner_explains_every_decision_and_proposes_specialists_only_for_named_gaps() {
        let (state, _owner, _lead, _dev, _qa) = launched();
        let oauth = plan_for(
            &state,
            "Add OAuth state validation tests for the Google sign-in flow and document it",
        );
        let keys: Vec<&str> = oauth.tasks.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(keys, ["implement", "tests", "docs"]);
        assert_eq!(oauth.tasks[2].depends_on, ["tests"]);
        let specialists: Vec<&str> = oauth
            .recruitments
            .iter()
            .map(|r| r.specialty.as_str())
            .collect();
        assert_eq!(
            specialists,
            ["security"],
            "OAuth is a security skill the core roles lack"
        );
        assert!(oauth.recruitments[0]
            .because
            .iter()
            .any(|b| b.contains("'oauth'")));
        assert_eq!(oauth.recruitments[0].after_task.as_deref(), Some("tests"));
        assert!(oauth
            .explanations
            .iter()
            .any(|e| e.decision == "specialist: security" && e.because.len() >= 2));
        // A goal naming no outside skill gets no specialist, and says so.
        let typo = plan_for(&state, "Fix the typo in the README");
        assert!(typo.recruitments.is_empty());
        assert!(typo
            .explanations
            .iter()
            .any(|e| e.decision == "no specialists"));
        // An interface migration gets a design step and an architecture review, but not a security one.
        let migration = plan_for(&state, "Migrate the API schema to the new protocol design");
        assert_eq!(migration.tasks[0].key, "design");
        assert_eq!(migration.tasks[1].depends_on, ["design"]);
        assert_eq!(
            migration
                .recruitments
                .iter()
                .map(|r| r.specialty.as_str())
                .collect::<Vec<_>>(),
            ["architect"]
        );
        assert_eq!(planning::plan_depth(&migration.tasks), Some(3));
        // The same goal and declarations always give the same plan.
        assert_eq!(
            plan_for(&state, "Add OAuth tests"),
            plan_for(&state, "Add OAuth tests")
        );
        // Nothing it says claims verification or a cost.
        assert!(oauth.warnings.iter().any(|w| w.contains("not verified")));
        assert!(oauth
            .warnings
            .iter()
            .any(|w| w.contains("Nothing here estimates cost")));
    }

    #[test]
    fn providers_are_chosen_from_declarations_and_preferences_never_from_assumed_cost() {
        let (mut state, owner, _lead, _dev, _qa) = launched();
        // No providers yet: nothing is assumed, and the plan says to assign by hand.
        let none = plan_for(&state, "Add OAuth tests");
        assert!(none.roles.iter().all(|r| r.provider_id.is_none()));
        assert!(none
            .warnings
            .iter()
            .any(|w| w.contains("no providers are declared")));
        declare(
            &mut state,
            &owner,
            "aaa-reviewer",
            &["review"],
            Metering::Metered,
            true,
        );
        declare(
            &mut state,
            &owner,
            "bbb-coder",
            &["code_edit", "shell"],
            Metering::Estimated,
            false,
        );
        let chosen = |plan: &planning::Plan, role: &str| {
            plan.roles
                .iter()
                .find(|r| r.role_key == role)
                .unwrap()
                .provider_id
                .clone()
        };
        let plan = plan_for(&state, "Add OAuth tests");
        assert_eq!(
            chosen(&plan, "developer").as_deref(),
            Some("bbb-coder"),
            "the first declared provider with code_edit"
        );
        assert_eq!(
            chosen(&plan, "lead").as_deref(),
            Some("aaa-reviewer"),
            "a lead needs nothing, so the first declared one"
        );
        assert_eq!(
            plan.recruitments[0].provider_id.as_deref(),
            Some("aaa-reviewer"),
            "the reviewer offers review"
        );
        // A preference wins when the provider is declared to offer what the role needs...
        state
            .execute(
                &owner,
                Command::SetRolePreference {
                    role_key: "developer".into(),
                    provider_id: "bbb-coder".into(),
                    model_id: Some("m1".into()),
                },
            )
            .unwrap();
        let pref = plan_for(&state, "Add OAuth tests");
        assert_eq!(
            pref.roles
                .iter()
                .find(|r| r.role_key == "developer")
                .unwrap()
                .rationale
                .contains("your preference"),
            true
        );
        // ...and is ignored, with the fallback explained, when it is not.
        state
            .execute(
                &owner,
                Command::SetRolePreference {
                    role_key: "qa".into(),
                    provider_id: "aaa-reviewer".into(),
                    model_id: None,
                },
            )
            .unwrap();
        let qa = plan_for(&state, "Add OAuth tests")
            .roles
            .into_iter()
            .find(|r| r.role_key == "qa")
            .unwrap();
        assert_eq!(
            qa.provider_id.as_deref(),
            Some("bbb-coder"),
            "the reviewer is not declared to offer shell"
        );
        // Declarations are validated, and are never marked verified.
        assert_eq!(
            state
                .execute(
                    &owner,
                    Command::SetRolePreference {
                        role_key: "qa".into(),
                        provider_id: "ghost".into(),
                        model_id: None
                    }
                )
                .unwrap_err()
                .code,
            "not_found"
        );
        assert_eq!(
            state
                .execute(
                    &owner,
                    Command::SetRolePreference {
                        role_key: "boss".into(),
                        provider_id: "bbb-coder".into(),
                        model_id: None
                    }
                )
                .unwrap_err()
                .code,
            "invalid_params"
        );
        assert!(state.providers.values().all(|p| !p.verified));
        state.validate().unwrap();
    }

    #[test]
    fn plans_are_edited_before_launch_within_bounds() {
        let (mut state, owner, _lead, _dev, _qa) = launched();
        declare(
            &mut state,
            &owner,
            "coder",
            &["code_edit", "shell", "review"],
            Metering::Metered,
            true,
        );
        state
            .execute(
                &owner,
                Command::ProposePlan {
                    plan_id: "p1".into(),
                    goal: "Add OAuth tests and document them".into(),
                },
            )
            .unwrap();
        let edit = |state: &mut State, edits: Vec<PlanEdit>| {
            state.execute(
                &owner,
                Command::EditPlan {
                    plan_id: "p1".into(),
                    edits,
                },
            )
        };
        edit(
            &mut state,
            vec![
                PlanEdit::RemoveTask { key: "docs".into() },
                PlanEdit::RenameTeam {
                    name: "Auth team".into(),
                },
                PlanEdit::DropRecruitment {
                    key: "security".into(),
                },
            ],
        )
        .unwrap();
        let plan = &state.plans["p1"];
        assert!(plan.tasks.iter().find(|t| t.key == "docs").unwrap().removed);
        assert_eq!(
            (plan.draft.name.as_str(), plan.recruitments[0].dropped),
            ("Auth team", true)
        );
        // Edits that would break the graph are refused and change nothing.
        assert_eq!(
            edit(
                &mut state,
                vec![PlanEdit::SetTaskDependencies {
                    key: "implement".into(),
                    depends_on: vec!["tests".into()]
                }]
            )
            .unwrap_err()
            .code,
            "dependency_cycle"
        );
        assert_eq!(
            edit(
                &mut state,
                vec![PlanEdit::SetTaskDependencies {
                    key: "tests".into(),
                    depends_on: vec!["docs".into()]
                }]
            )
            .unwrap_err()
            .code,
            "dependency_cycle",
            "a removed task cannot be depended on"
        );
        let mut chain = Vec::new();
        for i in 0..8 {
            chain.push(PlanEdit::AddTask {
                key: format!("c{i}"),
                title: format!("step {i}"),
                description: "d".into(),
                depends_on: if i == 0 {
                    vec!["tests".into()]
                } else {
                    vec![format!("c{}", i - 1)]
                },
            });
        }
        let error = edit(&mut state, chain).unwrap_err();
        assert!(
            matches!(error.code, "task_depth_limit" | "capacity_exceeded"),
            "{}",
            error.code
        );
        assert!(
            !state.plans["p1"].tasks.iter().any(|t| t.key == "c0"),
            "a refused batch leaves the plan as it was"
        );
        assert_eq!(
            edit(
                &mut state,
                vec![PlanEdit::SetRoleProvider {
                    role_key: "developer".into(),
                    provider_id: "ghost".into(),
                    model_id: None
                }]
            )
            .unwrap_err()
            .code,
            "not_found"
        );
        edit(
            &mut state,
            vec![PlanEdit::SetRoleProvider {
                role_key: "developer".into(),
                provider_id: "coder".into(),
                model_id: Some("m1".into()),
            }],
        )
        .unwrap();
        // Only the Owner edits; a discarded plan is final.
        let lead = state.authenticate(&digest('1')).unwrap();
        assert_eq!(
            state
                .execute(
                    &lead,
                    Command::EditPlan {
                        plan_id: "p1".into(),
                        edits: vec![PlanEdit::RenameTeam { name: "x".into() }]
                    }
                )
                .unwrap_err()
                .code,
            "forbidden"
        );
        state
            .execute(
                &owner,
                Command::DiscardPlan {
                    plan_id: "p1".into(),
                },
            )
            .unwrap();
        assert_eq!(
            edit(&mut state, vec![PlanEdit::RenameTeam { name: "y".into() }])
                .unwrap_err()
                .code,
            "invalid_state"
        );
        state.validate().unwrap();
    }

    #[test]
    fn a_lead_changes_a_provider_mid_run_only_with_an_owner_approval_for_exactly_that() {
        let (mut state, owner, lead, _dev, _qa) = launched();
        declare(
            &mut state,
            &owner,
            "coder",
            &["code_edit", "shell"],
            Metering::Metered,
            true,
        );
        declare(
            &mut state,
            &owner,
            "other",
            &["code_edit"],
            Metering::Estimated,
            false,
        );
        declare(
            &mut state,
            &owner,
            "reviewer",
            &["review"],
            Metering::Metered,
            true,
        );
        let reassign = |state: &mut State, provider: &str, approval: &str, now: u64| {
            state.execute_at(
                &lead,
                Command::ReassignRoleProvider {
                    team_id: "t".into(),
                    role_key: "developer".into(),
                    provider_id: provider.into(),
                    model_id: Some("m1".into()),
                    approval_id: approval.into(),
                },
                now,
            )
        };
        assert_eq!(
            reassign(&mut state, "coder", "ap-1", 100).unwrap_err().code,
            "not_found",
            "no approval, no change"
        );
        let approve = |state: &mut State, id: &str, provider: &str| {
            state
                .execute_at(
                    &lead,
                    Command::RequestApproval {
                        approval_id: id.into(),
                        team_id: "t".into(),
                        action: planning::reassign_action("t", "developer", provider, Some("m1")),
                        justification: "swap the developer's provider".into(),
                        alternatives: vec![],
                        risks: vec!["different tool behavior".into()],
                        ttl_seconds: 600,
                    },
                    100,
                )
                .unwrap();
            state
                .execute_at(
                    &owner,
                    Command::DecideApproval {
                        approval_id: id.into(),
                        approve: true,
                        rationale: "ok".into(),
                    },
                    101,
                )
                .unwrap();
        };
        approve(&mut state, "ap-1", "coder");
        // The approval names coder; asking for another provider with it is refused and spends nothing.
        assert_eq!(
            reassign(&mut state, "other", "ap-1", 102).unwrap_err().code,
            "approval_mismatch"
        );
        reassign(&mut state, "coder", "ap-1", 102).unwrap();
        assert_eq!(state.role_assignments["t:developer"].provider_id, "coder");
        assert_eq!(
            reassign(&mut state, "coder", "ap-1", 103).unwrap_err().code,
            "approval_used"
        );
        // A provider not declared to offer what the role needs is refused even with an approval.
        approve(&mut state, "ap-2", "reviewer");
        assert_eq!(
            reassign(&mut state, "reviewer", "ap-2", 104)
                .unwrap_err()
                .code,
            "capability_missing"
        );
        // And an expired approval does not count.
        approve(&mut state, "ap-3", "other");
        assert_eq!(
            reassign(&mut state, "other", "ap-3", 100 + 602)
                .unwrap_err()
                .code,
            "approval_expired"
        );
        assert_eq!(state.role_assignments["t:developer"].provider_id, "coder");
        state.validate().unwrap();
    }

    #[test]
    fn memory_serves_accepted_requirements_exact_evidence_and_never_chat() {
        let (mut state, owner, lead, dev, qa) = launched();
        let (lead2, _d2, _q2) = add_team(&mut state, &owner, 2);
        create_task(&mut state, &lead, "task", &[]).unwrap();
        state
            .execute(
                &lead,
                Command::AppendChat {
                    team_id: "t".into(),
                    message_id: "gossip".into(),
                    body: "the secret plan is plan-zebra".into(),
                    task_id: Some("task".into()),
                    audience: Audience::Actor {
                        actor_id: "qa".into(),
                    },
                },
            )
            .unwrap();
        state
            .execute(
                &lead,
                Command::PutKnowledge {
                    team_id: "t".into(),
                    knowledge_id: "adr-1".into(),
                    scope: KnowledgeScope::Team {
                        team_id: "t".into(),
                    },
                    title: "Use state parameters".into(),
                    body: "Every OAuth request carries a random state.".into(),
                    evidence: vec![],
                    knowledge_kind: KnowledgeKind::Adr,
                },
            )
            .unwrap();
        let memory = |state: &State, who: &Actor, budget: usize| {
            state.retrieve_memory(who, Some("t"), Some("task"), budget, 1_000)
        };
        // Proposed is not accepted: agents are not told about it yet.
        assert_eq!(
            memory(&state, &dev, 8192).unwrap()["context"]["requirements"],
            json!([])
        );
        assert_eq!(
            state
                .execute(
                    &lead,
                    Command::AcceptKnowledge {
                        knowledge_id: "adr-1".into()
                    }
                )
                .unwrap_err()
                .code,
            "forbidden"
        );
        state
            .execute(
                &owner,
                Command::AcceptKnowledge {
                    knowledge_id: "adr-1".into(),
                },
            )
            .unwrap();
        // Run the task to a QA review so there is exact evidence to hand over.
        state
            .execute(
                &dev,
                Command::AcceptTask {
                    team_id: "t".into(),
                    task_id: "task".into(),
                    acceptance_id: "acc".into(),
                    note: "go".into(),
                },
            )
            .unwrap();
        let exact = vec![
            Evidence {
                kind: EvidenceKind::Commit,
                reference: "deadbeef00".into(),
                digest: Some("a".repeat(64)),
            },
            Evidence {
                kind: EvidenceKind::Test,
                reference: "go test ./... ok".into(),
                digest: None,
            },
        ];
        state
            .execute(
                &dev,
                Command::SubmitHandoff {
                    team_id: "t".into(),
                    task_id: "task".into(),
                    handoff_id: "h".into(),
                    message_id: "m-h".into(),
                    summary: "done".into(),
                    evidence: exact.clone(),
                    dispatch: intent("a-h", 1),
                },
            )
            .unwrap();
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
                    rationale: "evidence checked".into(),
                    evidence: evidence(),
                    rework_dispatch: None,
                },
            )
            .unwrap();
        let pack = memory(&state, &dev, 8192).unwrap();
        assert_eq!(pack["context"]["requirements"][0]["id"], "adr-1");
        assert_eq!(
            pack["context"]["requirements"][0]["provenance"]["accepted"],
            true
        );
        assert_eq!(
            pack["context"]["handoff"][0]["evidence"],
            serde_json::to_value(&exact).unwrap(),
            "evidence arrives exactly as submitted"
        );
        assert_eq!(pack["context"]["reviews"][0]["body"], "evidence checked");
        assert_eq!(pack["context"]["task"][0]["provenance"]["kind"], "task");
        assert!(
            !pack.to_string().contains("plan-zebra"),
            "no chat, scoped or otherwise, reaches an agent's context"
        );
        assert!(pack["never_included"].as_str().unwrap().contains("chat"));
        assert!(pack["used_bytes"].as_u64().unwrap() <= pack["budget_bytes"].as_u64().unwrap());
        // A small budget drops whole items and counts them; nothing is cut in the middle.
        let tight = memory(&state, &dev, 700).unwrap();
        assert!(tight["used_bytes"].as_u64().unwrap() <= 700);
        assert!(
            tight["omitted_for_budget"]
                .as_object()
                .is_some_and(|o| !o.is_empty()),
            "{tight}"
        );
        for item in tight["context"]["handoff"].as_array().unwrap() {
            assert_eq!(
                item["evidence"],
                serde_json::to_value(&exact).unwrap(),
                "an included handoff is whole"
            );
        }
        // Boundaries: another team's agent, and a budget outside the limits.
        assert_eq!(
            state
                .retrieve_memory(&lead2, Some("t"), Some("task"), 8192, 1)
                .unwrap_err()
                .code,
            "forbidden"
        );
        assert_eq!(memory(&state, &dev, 0).unwrap_err().code, "invalid_params");
        assert_eq!(
            memory(&state, &dev, MAX_BUDGET + 1).unwrap_err().code,
            "invalid_params"
        );
        assert_eq!(
            state
                .retrieve_memory(&owner, None, None, 100, 1)
                .unwrap_err()
                .code,
            "invalid_params"
        );
    }

    #[test]
    fn a_summary_goes_stale_when_what_it_describes_changes_and_is_then_left_out() {
        let (mut state, owner, lead, dev, _qa) = launched();
        create_task(&mut state, &lead, "task", &[]).unwrap();
        let note = |state: &mut State, id: &str, seconds: Option<u64>| {
            state.execute_at(
                &lead,
                Command::PutContextNote {
                    team_id: "t".into(),
                    note_id: id.into(),
                    note_kind: NoteKind::Summary,
                    role_key: None,
                    summary: format!("summary {id}"),
                    sources: vec![SourceRef {
                        kind: SourceKind::Task,
                        id: "task".into(),
                    }],
                    valid_for_seconds: seconds,
                },
                1_000,
            )
        };
        assert_eq!(
            state
                .execute(
                    &lead,
                    Command::PutContextNote {
                        team_id: "t".into(),
                        note_id: "n0".into(),
                        note_kind: NoteKind::Summary,
                        role_key: None,
                        summary: "no sources".into(),
                        sources: vec![],
                        valid_for_seconds: None
                    }
                )
                .unwrap_err()
                .code,
            "invalid_params",
            "a summary without provenance is refused"
        );
        note(&mut state, "n1", None).unwrap();
        note(&mut state, "n-timed", Some(60)).unwrap();
        let served = |state: &State, now: u64| -> (Vec<String>, Vec<String>) {
            let pack = state
                .retrieve_memory(&lead, Some("t"), Some("task"), 8192, now)
                .unwrap();
            let ids = |v: &Value| {
                v.as_array()
                    .unwrap()
                    .iter()
                    .map(|n| n["id"].as_str().unwrap().to_owned())
                    .collect::<Vec<_>>()
            };
            (
                ids(&pack["context"]["notes"]),
                ids(&pack["excluded_as_stale"]),
            )
        };
        let (current, stale) = served(&state, 1_010);
        assert_eq!((current.len(), stale.len()), (2, 0));
        // The validity window ends.
        let (current, stale) = served(&state, 1_061);
        assert_eq!(
            (current, stale),
            (vec!["n1".to_owned()], vec!["n-timed".to_owned()])
        );
        // The task moves on: both notes now describe something that is no longer so.
        state
            .execute(
                &dev,
                Command::AcceptTask {
                    team_id: "t".into(),
                    task_id: "task".into(),
                    acceptance_id: "acc".into(),
                    note: "go".into(),
                },
            )
            .unwrap();
        let (current, stale) = served(&state, 1_010);
        assert!(
            current.is_empty() && stale.len() == 2,
            "{current:?} {stale:?}"
        );
        let pack = state
            .retrieve_memory(&lead, Some("t"), Some("task"), 8192, 1_010)
            .unwrap();
        assert!(pack["excluded_as_stale"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("changed at revision"));
        // A fresh note is current; superseding retires the old one for good.
        note(&mut state, "n2", None).unwrap();
        assert_eq!(served(&state, 1_010).0, ["n2"]);
        state
            .execute(
                &lead,
                Command::SupersedeContextNote {
                    note_id: "n2".into(),
                    by_note_id: "n1".into(),
                },
            )
            .unwrap();
        assert!(served(&state, 1_010).0.is_empty());
        let reason = state.note_validity(&state.context_notes["n2"], 1_010);
        assert!(matches!(reason, memory::Validity::Stale(r) if r.contains("superseded")));
        // Sources must be the team's own records.
        assert_eq!(
            state
                .execute(
                    &lead,
                    Command::PutContextNote {
                        team_id: "t".into(),
                        note_id: "n9".into(),
                        note_kind: NoteKind::Summary,
                        role_key: None,
                        summary: "x".into(),
                        sources: vec![SourceRef {
                            kind: SourceKind::Task,
                            id: "nope".into()
                        }],
                        valid_for_seconds: None
                    }
                )
                .unwrap_err()
                .code,
            "invalid_reference"
        );
        state.validate().unwrap();
        let _ = owner;
    }

    fn evid(reference: &str) -> Evidence {
        Evidence {
            kind: EvidenceKind::Note,
            reference: reference.into(),
            digest: None,
        }
    }

    #[test]
    fn disagreements_are_bounded_reviewed_by_peers_and_escalated_instead_of_looping() {
        let (mut state, owner, lead, dev, qa) = launched();
        create_task(&mut state, &lead, "task", &[]).unwrap();
        let raise =
            |state: &mut State, who: &Actor, id: &str, topic: &str, arch: bool, refs: &[&str]| {
                state.execute(
                    who,
                    Command::RaiseDispute {
                        dispute_id: id.into(),
                        team_id: "t".into(),
                        task_id: Some("task".into()),
                        topic: topic.into(),
                        architectural: arch,
                        position: "my view".into(),
                        evidence: refs.iter().map(|r| evid(r)).collect(),
                    },
                )
            };
        // Two distinct pieces of conflicting evidence, or it is not a dispute.
        assert_eq!(
            raise(&mut state, &dev, "d0", "len", false, &["only-one"])
                .unwrap_err()
                .code,
            "invalid_params"
        );
        assert_eq!(
            raise(&mut state, &dev, "d0", "len", false, &["same", "same"])
                .unwrap_err()
                .code,
            "invalid_params"
        );
        raise(
            &mut state,
            &dev,
            "d1",
            "state length",
            false,
            &["rfc", "test-a"],
        )
        .unwrap();
        assert_eq!(state.disputes["d1"].status, oversight::DisputeStatus::Open);
        // The Lead cannot settle it before a peer has weighed in; the raiser cannot answer themselves.
        assert_eq!(
            state
                .execute(
                    &lead,
                    Command::ResolveDispute {
                        dispute_id: "d1".into(),
                        resolution: "x".into()
                    }
                )
                .unwrap_err()
                .code,
            "invalid_state"
        );
        assert_eq!(
            state
                .execute(
                    &dev,
                    Command::RespondDispute {
                        dispute_id: "d1".into(),
                        position: "again".into(),
                        evidence: vec![]
                    }
                )
                .unwrap_err()
                .code,
            "already_exists"
        );
        state
            .execute(
                &qa,
                Command::RespondDispute {
                    dispute_id: "d1".into(),
                    position: "16 bytes is enough".into(),
                    evidence: vec![evid("test-b")],
                },
            )
            .unwrap();
        // The third position ends peer review: it is escalated, not extended.
        state
            .execute(
                &lead,
                Command::RespondDispute {
                    dispute_id: "d1".into(),
                    position: "I see both".into(),
                    evidence: vec![],
                },
            )
            .unwrap();
        assert!(matches!(
            state.disputes["d1"].status,
            oversight::DisputeStatus::Escalated { .. }
        ));
        assert_eq!(state.disputes["d1"].positions.len(), MAX_POSITIONS);
        assert_eq!(
            state
                .execute(
                    &owner,
                    Command::RespondDispute {
                        dispute_id: "d1".into(),
                        position: "more".into(),
                        evidence: vec![]
                    }
                )
                .unwrap_err()
                .code,
            "forbidden",
            "owners decide, they do not join a debate"
        );
        let snapshot = state.snapshot(&owner, None).unwrap();
        assert_eq!(snapshot["disputes"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            snapshot["attention"]["escalated_disputes"],
            json!(["d1"]),
            "an escalated dispute is waiting for the Owner"
        );
        state
            .execute(
                &owner,
                Command::DecideDispute {
                    dispute_id: "d1".into(),
                    decision: "use 32 bytes".into(),
                },
            )
            .unwrap();
        assert!(
            matches!(&state.disputes["d1"].status, oversight::DisputeStatus::Resolved { by, .. } if by == "owner")
        );
        // A settled question stays settled unless there is new evidence.
        assert_eq!(
            raise(
                &mut state,
                &qa,
                "d2",
                "state length",
                false,
                &["rfc", "test-a"]
            )
            .unwrap_err()
            .code,
            "dispute_settled"
        );
        raise(
            &mut state,
            &qa,
            "d2",
            "state length",
            false,
            &["rfc", "brand-new-evidence"],
        )
        .unwrap();
        // An architectural question goes straight to the Owner, and the Lead cannot decide it.
        raise(
            &mut state,
            &dev,
            "d3",
            "storage layout",
            true,
            &["adr-a", "adr-b"],
        )
        .unwrap();
        assert!(matches!(
            state.disputes["d3"].status,
            oversight::DisputeStatus::Escalated { .. }
        ));
        assert_eq!(
            state
                .execute(
                    &lead,
                    Command::ResolveDispute {
                        dispute_id: "d3".into(),
                        resolution: "x".into()
                    }
                )
                .unwrap_err()
                .code,
            "forbidden"
        );
        // A Lead settles an ordinary dispute once a peer has answered.
        state
            .execute(
                &dev,
                Command::RespondDispute {
                    dispute_id: "d2".into(),
                    position: "agree".into(),
                    evidence: vec![],
                },
            )
            .unwrap();
        state
            .execute(
                &lead,
                Command::ResolveDispute {
                    dispute_id: "d2".into(),
                    resolution: "go with it".into(),
                },
            )
            .unwrap();
        // Unresolved disputes are capped, so a team cannot bury itself in them.
        for i in 0..MAX_OPEN_DISPUTES {
            let r = raise(
                &mut state,
                &dev,
                &format!("x{i}"),
                &format!("topic {i}"),
                false,
                &["a", "b"],
            );
            if i + 1 < MAX_OPEN_DISPUTES {
                r.unwrap();
            } else {
                assert!(r.is_err());
            }
        }
        state.validate().unwrap();
    }

    #[test]
    fn usage_is_split_into_reliable_and_estimated_and_only_reliable_usage_enforces_a_limit() {
        let (mut state, owner, lead, _dev, _qa) = launched();
        declare(
            &mut state,
            &owner,
            "billed",
            &["code_edit", "shell"],
            Metering::Metered,
            true,
        );
        declare(
            &mut state,
            &owner,
            "guess",
            &["code_edit", "shell"],
            Metering::Estimated,
            false,
        );
        declare(
            &mut state,
            &owner,
            "silent",
            &["code_edit", "shell"],
            Metering::None,
            false,
        );
        let report = |state: &mut State, provider: &str, basis: UsageBasis, cost: Option<u64>| {
            state.execute(
                &owner,
                Command::ReportUsage {
                    team_id: "t".into(),
                    provider_id: provider.into(),
                    input_tokens: 1000,
                    output_tokens: 200,
                    cost_micros: cost,
                    basis,
                },
            )
        };
        // Only a provider declared as metered can report metered usage, and it must say what it cost.
        assert_eq!(
            report(&mut state, "guess", UsageBasis::Metered, Some(5))
                .unwrap_err()
                .code,
            "invalid_state"
        );
        assert_eq!(
            report(&mut state, "billed", UsageBasis::Metered, None)
                .unwrap_err()
                .code,
            "invalid_params"
        );
        assert_eq!(
            state
                .execute(
                    &lead,
                    Command::ReportUsage {
                        team_id: "t".into(),
                        provider_id: "billed".into(),
                        input_tokens: 1,
                        output_tokens: 1,
                        cost_micros: Some(1),
                        basis: UsageBasis::Metered
                    }
                )
                .unwrap_err()
                .code,
            "forbidden"
        );
        report(&mut state, "billed", UsageBasis::Metered, Some(2_000)).unwrap();
        report(&mut state, "guess", UsageBasis::Estimated, Some(9_000_000)).unwrap();
        let view = state.team_usage_view("t");
        assert_eq!(view["reliable"]["metered_cost_micros"], 2_000);
        assert_eq!(view["estimated"]["cost_micros"], 9_000_000);
        assert_eq!(
            view["reliable"]["metered_tokens"], 1_200,
            "tokens from estimated sources do not count as reliable"
        );
        for assignment in [("billed", "developer"), ("guess", "lead"), ("silent", "qa")] {
            state
                .execute(
                    &owner,
                    Command::AssignRoleProvider {
                        team_id: "t".into(),
                        role_key: assignment.1.into(),
                        provider_id: assignment.0.into(),
                        model_id: None,
                    },
                )
                .unwrap();
        }
        let warnings = state.usage_warnings("t").join(" | ");
        assert!(
            warnings.contains("guess: costs are estimates"),
            "{warnings}"
        );
        assert!(
            warnings.contains("silent: usage is not measured at all"),
            "{warnings}"
        );
        assert!(
            warnings.contains("silent: this integration cannot cap billing"),
            "{warnings}"
        );
        assert!(warnings.contains("no usage limit is set"), "{warnings}");
        assert!(
            !warnings.contains("billed:"),
            "a metered provider with a hard limit has nothing to warn about"
        );
        // A huge estimate stops nothing; a reached metered limit stops new work.
        state
            .execute(
                &owner,
                Command::SetTeamLimits {
                    team_id: "t".into(),
                    max_metered_cost_micros: Some(2_500),
                    max_metered_tokens: None,
                },
            )
            .unwrap();
        create_task(&mut state, &lead, "ok", &[]).unwrap();
        report(&mut state, "billed", UsageBasis::Metered, Some(600)).unwrap();
        assert_eq!(
            create_task(&mut state, &lead, "blocked", &[])
                .unwrap_err()
                .code,
            "usage_limit_reached"
        );
        state
            .execute(
                &owner,
                Command::SetTeamLimits {
                    team_id: "t".into(),
                    max_metered_cost_micros: Some(10_000),
                    max_metered_tokens: None,
                },
            )
            .unwrap();
        create_task(&mut state, &lead, "unblocked", &[]).unwrap();
        state.validate().unwrap();
    }

    #[test]
    fn idle_and_released_agents_take_no_work_and_only_the_owner_brings_one_back() {
        let (mut state, owner, lead, dev, _qa) = launched();
        let set = |state: &mut State, who: &Actor, agent: &str, to: AgentRunState| {
            state.execute(
                who,
                Command::SetAgentState {
                    actor_id: agent.into(),
                    state: to,
                    reason: "restarting".into(),
                },
            )
        };
        assert_eq!(
            set(&mut state, &dev, "dev", AgentRunState::Idle)
                .unwrap_err()
                .code,
            "forbidden",
            "an agent cannot idle itself, and Developers cannot set states"
        );
        set(&mut state, &lead, "dev", AgentRunState::Idle).unwrap();
        assert_eq!(
            create_task(&mut state, &lead, "task", &[])
                .unwrap_err()
                .code,
            "agent_idle"
        );
        // Idle keeps the credential working; reading still works.
        assert!(state.authenticate(&digest('2')).is_ok());
        set(&mut state, &lead, "dev", AgentRunState::Active).unwrap();
        create_task(&mut state, &lead, "task", &[]).unwrap();
        // Release revokes the credential and the pane binding; a Lead cannot undo it.
        state
            .execute(
                &owner,
                Command::BindAgentPane {
                    actor_id: "dev".into(),
                    pane_id: "w1:p1".into(),
                    provider: None,
                },
            )
            .unwrap();
        set(&mut state, &lead, "dev", AgentRunState::Released).unwrap();
        assert_eq!(
            state.authenticate(&digest('2')).unwrap_err().code,
            "unauthorized"
        );
        assert!(state.pane_bindings.is_empty());
        assert_eq!(
            set(&mut state, &lead, "dev", AgentRunState::Active)
                .unwrap_err()
                .code,
            "forbidden"
        );
        set(&mut state, &owner, "dev", AgentRunState::Active).unwrap();
        assert!(state.authenticate(&digest('2')).is_ok());
        let (lead2, _d, _q) = add_team(&mut state, &owner, 2);
        assert_eq!(
            set(&mut state, &lead2, "dev", AgentRunState::Idle)
                .unwrap_err()
                .code,
            "forbidden",
            "a Lead controls only their own team"
        );
        assert_eq!(
            set(&mut state, &owner, "owner", AgentRunState::Idle)
                .unwrap_err()
                .code,
            "invalid_reference"
        );
        state.validate().unwrap();
    }

    #[test]
    fn hard_limits_stop_concurrency_and_deep_dependency_chains() {
        let (mut state, _owner, lead, dev, _qa) = launched();
        for i in 0..=MAX_ACTIVE_TASKS {
            create_task(&mut state, &lead, &format!("w{i}"), &[]).unwrap();
        }
        for i in 0..MAX_ACTIVE_TASKS {
            state
                .execute(
                    &dev,
                    Command::AcceptTask {
                        team_id: "t".into(),
                        task_id: format!("w{i}"),
                        acceptance_id: format!("acc{i}"),
                        note: "go".into(),
                    },
                )
                .unwrap();
        }
        let fourth = state.execute(
            &dev,
            Command::AcceptTask {
                team_id: "t".into(),
                task_id: format!("w{MAX_ACTIVE_TASKS}"),
                acceptance_id: "acc-last".into(),
                note: "go".into(),
            },
        );
        assert_eq!(fourth.unwrap_err().code, "concurrency_limit");
        // A chain deeper than the limit cannot be built, one task at a time or by editing dependencies.
        let (mut state, _owner, lead, _dev, _qa) = launched();
        create_task(&mut state, &lead, "c0", &[]).unwrap();
        for i in 1..MAX_DEPENDENCY_DEPTH {
            create_task(
                &mut state,
                &lead,
                &format!("c{i}"),
                &[&format!("c{}", i - 1)],
            )
            .unwrap();
        }
        let too_deep = create_task(
            &mut state,
            &lead,
            "too-deep",
            &[&format!("c{}", MAX_DEPENDENCY_DEPTH - 1)],
        );
        assert_eq!(too_deep.unwrap_err().code, "task_depth_limit");
        create_task(&mut state, &lead, "free", &[]).unwrap();
        let rewire = state.execute(
            &lead,
            Command::SetTaskDependencies {
                team_id: "t".into(),
                task_id: "free".into(),
                dependencies: vec![format!("c{}", MAX_DEPENDENCY_DEPTH - 1)],
            },
        );
        assert_eq!(rewire.unwrap_err().code, "task_depth_limit");
        state.validate().unwrap();
    }

    #[test]
    fn the_summary_finds_stuck_work_across_projects_and_suggests_recovery_without_acting() {
        let (mut state, owner, lead, dev, _qa) = launched();
        let (lead2, _d2, _q2) = add_team(&mut state, &owner, 2);
        state
            .execute(
                &owner,
                Command::CreateProject {
                    project_id: "other".into(),
                    name: "Other".into(),
                },
            )
            .unwrap();
        create_task(&mut state, &lead, "old", &[]).unwrap();
        state
            .execute_at(
                &lead2,
                Command::CreateTask {
                    team_id: "t2".into(),
                    task_id: "fresh".into(),
                    title: "x".into(),
                    description: "d".into(),
                    developer_id: "dev2".into(),
                    dependencies: vec![],
                    assignment_message_id: "m-fresh".into(),
                    dispatch: intent("a-fresh", 1),
                },
                5_000,
            )
            .unwrap();
        state
            .execute_at(
                &lead,
                Command::RequestApproval {
                    approval_id: "ap".into(),
                    team_id: "t".into(),
                    action: merge_action("x"),
                    justification: "j".into(),
                    alternatives: vec![],
                    risks: vec![],
                    ttl_seconds: 600,
                },
                5_000,
            )
            .unwrap();
        let now = 5_100;
        let summary = state.summary(&owner, now, 1_800).unwrap();
        assert_eq!(summary["totals"]["teams"], 2);
        assert_eq!(summary["totals"]["pending_approvals"], 1);
        // Nothing has been quiet for half an hour yet, whichever clock the tasks were written with.
        let quiet = state.summary(&owner, now + 3_600, 1_800).unwrap();
        let stuck: Vec<&Value> = quiet["projects"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|p| p["teams"].as_array().unwrap())
            .flat_map(|t| t["stuck"].as_array().unwrap())
            .collect();
        assert!(
            stuck
                .iter()
                .any(|s| s["task_id"] == "fresh" && s["idle_seconds"].as_u64().unwrap() >= 3_600),
            "{quiet}"
        );
        let fresh = stuck.iter().find(|s| s["task_id"] == "fresh").unwrap();
        let actions: Vec<&str> = fresh["suggested_actions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|a| a["command"].as_str())
            .collect();
        assert!(actions.contains(&"set_agent_state") && actions.contains(&"cancel_task"));
        // A failed dispatch adds a retry suggestion; the summary only advises.
        state
            .execute(
                &owner,
                Command::RecordDispatchReceipt {
                    team_id: "t2".into(),
                    attempt_id: "a-fresh".into(),
                    task_id: "fresh".into(),
                    message_id: "m-fresh".into(),
                    run_generation: 1,
                    content_digest: digest('d'),
                    provider_session: None,
                    receipt: DispatchReceiptKind::Attempted,
                    detail: None,
                    evidence: vec![],
                },
            )
            .unwrap();
        state
            .execute(
                &owner,
                Command::RecordDispatchReceipt {
                    team_id: "t2".into(),
                    attempt_id: "a-fresh".into(),
                    task_id: "fresh".into(),
                    message_id: "m-fresh".into(),
                    run_generation: 1,
                    content_digest: digest('d'),
                    provider_session: None,
                    receipt: DispatchReceiptKind::Failed,
                    detail: Some("adapter crashed".into()),
                    evidence: vec![],
                },
            )
            .unwrap();
        let again = state.summary(&owner, now + 3_600, 1_800).unwrap();
        let text = again.to_string();
        assert!(
            text.contains("retry_dispatch") && text.contains("the last dispatch did not complete")
        );
        assert_eq!(
            state.dispatches["a-fresh"].team_id, "t2",
            "nothing was retried on anyone's behalf"
        );
        // A team sees only itself; a suspended team reports no stuck work (it is paused on purpose).
        let mine = state.summary(&lead, now, 1_800).unwrap();
        assert_eq!(mine["totals"]["teams"], 1);
        state
            .execute(
                &owner,
                Command::SuspendTeam {
                    team_id: "t2".into(),
                    reason: "pause".into(),
                },
            )
            .unwrap();
        let paused = state.summary(&owner, now + 9_999, 1).unwrap();
        let t2 = paused["projects"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|p| p["teams"].as_array().unwrap())
            .find(|t| t["id"] == "t2")
            .unwrap()
            .clone();
        assert_eq!(t2["stuck"], json!([]));
        let _ = dev;
    }

    #[test]
    fn tasks_record_when_they_last_changed() {
        let (mut state, _owner, lead, dev, _qa) = launched();
        state
            .execute_at(
                &lead,
                Command::CreateTask {
                    team_id: "t".into(),
                    task_id: "x".into(),
                    title: "x".into(),
                    description: "d".into(),
                    developer_id: "dev".into(),
                    dependencies: vec![],
                    assignment_message_id: "m-x".into(),
                    dispatch: intent("a-x", 1),
                },
                777,
            )
            .unwrap();
        assert_eq!(state.tasks["x"].updated_at, 777);
        state
            .execute_at(
                &dev,
                Command::AcceptTask {
                    team_id: "t".into(),
                    task_id: "x".into(),
                    acceptance_id: "acc".into(),
                    note: "go".into(),
                },
                900,
            )
            .unwrap();
        assert_eq!(state.tasks["x"].updated_at, 900);
        // An unrelated change leaves it alone.
        chat(&mut state, &lead, "c", Audience::Team).unwrap();
        assert_eq!(state.tasks["x"].updated_at, 900);
    }
}

//! Authority classes, owner approvals and bounded specialist recruitment.
//!
//! Chat is never an approval: an approval is a separate record that only the Owner can decide,
//! that binds one exact action by digest, and that can be consumed once before it expires.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{
    applied, require_role, serialize_value, validate_evidence, validate_id, validate_text, Actor,
    Applied, Command, Credential, Evidence, Role, State, MAX_MESSAGE_BYTES, MAX_TITLE_BYTES,
};
use crate::teams::{Error, Result};

const MAX_PENDING_APPROVALS_PER_TEAM: usize = 16;
const MAX_RECRUITMENTS_PER_TEAM: usize = 16;
const MAX_SPECIALISTS_PER_TEAM: usize = 1;
const MAX_SPECIALISTS_PER_PROJECT: usize = 2;
const MAX_SPECIALIST_RETRIES: u8 = 2;
const MAX_SPECIALIST_MINUTES: u32 = 240;
const MAX_APPROVAL_NOTES: usize = 8;
const MAX_ACTION_PARAMETER_BYTES: usize = 4096;
const MIN_APPROVAL_TTL_SECONDS: u64 = 60;
const MAX_APPROVAL_TTL_SECONDS: u64 = 24 * 60 * 60;
/// What a recruited specialist may be asked to do. Anything broader is a rights escalation and
/// needs an Owner approval of its own; the API refuses it here rather than guessing.
const SPECIALIST_CAPABILITIES: &[&str] = &["read_code", "review", "comment", "run_tests"];

/// A: agents act on their own inside their scope. B: a Lead directs bounded work.
/// C: only the Owner may authorize or perform it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Authority {
    AgentAutonomous,
    LeadAuthorized,
    OwnerOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProtectedActionKind {
    ScopeChange,
    ArchitectureChange,
    RightsEscalation,
    BudgetEscalation,
    DestructiveOperation,
    ProtectedBranchIntegration,
    Release,
}

/// The exact action an approval authorizes. `parameters` are part of the bound digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProtectedAction {
    pub kind: ProtectedActionKind,
    pub target: String,
    #[serde(default)]
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved {
        decided_by: String,
        rationale: String,
        expires_at: u64,
    },
    Denied {
        decided_by: String,
        rationale: String,
    },
    Used {
        execution_id: String,
        consumed_by: String,
        at: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub id: String,
    pub team_id: String,
    pub requested_by: String,
    pub action: ProtectedAction,
    pub action_digest: String,
    pub justification: String,
    /// What else was considered, and what could go wrong: context for the Owner, not part of what is approved.
    #[serde(default)]
    pub alternatives: Vec<String>,
    #[serde(default)]
    pub risks: Vec<String>,
    pub ttl_seconds: u64,
    pub requested_at: u64,
    pub status: ApprovalStatus,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RecruitmentStatus {
    /// Over capacity: waits, launches nothing.
    Queued,
    /// Within limits; a slot is reserved until the Owner's harness starts the specialist.
    Authorized,
    Active {
        actor_id: String,
        run_generation: u64,
        deadline: u64,
        attempt: u8,
    },
    HandedOff {
        actor_id: String,
        summary: String,
        evidence: Vec<Evidence>,
        attempt: u8,
    },
    Released {
        accepted: bool,
        rationale: String,
    },
    Cancelled {
        reason: String,
    },
}

impl RecruitmentStatus {
    /// Holds a specialist slot against the team and project limits.
    fn holds_slot(&self) -> bool {
        matches!(
            self,
            Self::Authorized | Self::Active { .. } | Self::HandedOff { .. }
        )
    }
}

/// A temporary specialist. `specialty` is the permanent role identity; the live process is the
/// `actor_id` + `run_generation` recorded once it is activated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recruitment {
    pub id: String,
    pub team_id: String,
    pub requested_by: String,
    pub specialty: String,
    pub justification: String,
    pub deliverable: String,
    pub capabilities: Vec<String>,
    pub dependencies: Vec<String>,
    pub stop_condition: String,
    pub max_retries: u8,
    pub max_minutes: u32,
    pub status: RecruitmentStatus,
    pub revision: u64,
}

impl Command {
    pub fn authority(&self) -> Authority {
        match self {
            Self::AppendChat { .. }
            | Self::AcceptTask { .. }
            | Self::SubmitHandoff { .. }
            | Self::ReviewHandoff { .. }
            | Self::RequestApproval { .. }
            | Self::ConsumeApproval { .. }
            | Self::SubmitSpecialistHandoff { .. }
            | Self::SubmitIntegration { .. }
            | Self::ReviewIntegration { .. }
            | Self::RaiseDispute { .. }
            | Self::RespondDispute { .. } => Authority::AgentAutonomous,
            Self::CreateTask { .. }
            | Self::SetTaskDependencies { .. }
            | Self::FinalizeTask { .. }
            | Self::CancelTask { .. }
            | Self::PutKnowledge { .. }
            | Self::RetryDispatch { .. }
            | Self::RequestRecruitment { .. }
            | Self::VerifyRecruitment { .. }
            | Self::CancelRecruitment { .. }
            | Self::RenameTeam { .. }
            | Self::SuspendTeam { .. }
            | Self::ResumeTeam { .. }
            | Self::ReleaseWorktree { .. }
            | Self::AcknowledgeNotice { .. }
            | Self::BindAgentPane { .. }
            | Self::UnbindAgentPane { .. }
            | Self::ReassignRoleProvider { .. }
            | Self::ResolveDispute { .. }
            | Self::PutContextNote { .. }
            | Self::SupersedeContextNote { .. }
            | Self::SetAgentState { .. }
            | Self::CancelIntegration { .. } => Authority::LeadAuthorized,
            Self::CreateProject { .. }
            | Self::CreateTeamSpec { .. }
            | Self::EnrollActor { .. }
            | Self::LaunchTeam { .. }
            | Self::RecordDispatchReceipt { .. }
            | Self::DecideApproval { .. }
            | Self::RevokeApproval { .. }
            | Self::ActivateRecruitment { .. }
            | Self::CancelTeam { .. }
            | Self::RegisterSurface { .. }
            | Self::ClaimWorktree { .. }
            | Self::ApproveWorktreeCleanup { .. }
            | Self::ResolveConflict { .. }
            | Self::BeginMerge { .. }
            | Self::CompleteMerge { .. }
            | Self::AbortMerge { .. }
            | Self::DeclareProvider { .. }
            | Self::SetRolePreference { .. }
            | Self::AssignRoleProvider { .. }
            | Self::ProposePlan { .. }
            | Self::EditPlan { .. }
            | Self::DiscardPlan { .. }
            | Self::MarkPlanLaunched { .. }
            | Self::AcceptKnowledge { .. }
            | Self::SupersedeKnowledge { .. }
            | Self::DecideDispute { .. }
            | Self::ReportUsage { .. }
            | Self::SetTeamLimits { .. } => Authority::OwnerOnly,
        }
    }

    /// The roles allowed to submit this command at all; handlers still check scope and state.
    pub(in crate::teams) fn permitted_roles(&self) -> &'static [Role] {
        const ALL_MEMBERS: &[Role] = &[Role::Owner, Role::Lead, Role::Developer, Role::Qa];
        match self {
            Self::CreateProject { .. }
            | Self::CreateTeamSpec { .. }
            | Self::EnrollActor { .. }
            | Self::LaunchTeam { .. }
            | Self::RecordDispatchReceipt { .. }
            | Self::DecideApproval { .. }
            | Self::RevokeApproval { .. }
            | Self::ActivateRecruitment { .. }
            | Self::CancelTeam { .. }
            | Self::RegisterSurface { .. }
            | Self::ClaimWorktree { .. }
            | Self::ApproveWorktreeCleanup { .. }
            | Self::ResolveConflict { .. }
            | Self::BeginMerge { .. }
            | Self::CompleteMerge { .. }
            | Self::AbortMerge { .. }
            | Self::DeclareProvider { .. }
            | Self::SetRolePreference { .. }
            | Self::AssignRoleProvider { .. }
            | Self::ProposePlan { .. }
            | Self::EditPlan { .. }
            | Self::DiscardPlan { .. }
            | Self::MarkPlanLaunched { .. }
            | Self::AcceptKnowledge { .. }
            | Self::SupersedeKnowledge { .. }
            | Self::DecideDispute { .. }
            | Self::ReportUsage { .. }
            | Self::SetTeamLimits { .. } => &[Role::Owner],
            Self::CreateTask { .. }
            | Self::SetTaskDependencies { .. }
            | Self::FinalizeTask { .. }
            | Self::CancelTask { .. }
            | Self::RetryDispatch { .. }
            | Self::RequestRecruitment { .. }
            | Self::VerifyRecruitment { .. }
            | Self::AcknowledgeNotice { .. }
            | Self::ReassignRoleProvider { .. }
            | Self::ResolveDispute { .. } => &[Role::Lead],
            Self::RaiseDispute { .. } | Self::RespondDispute { .. } => {
                &[Role::Lead, Role::Developer, Role::Qa]
            }
            Self::PutContextNote { .. }
            | Self::SupersedeContextNote { .. }
            | Self::SetAgentState { .. } => &[Role::Owner, Role::Lead],
            Self::RenameTeam { .. }
            | Self::SuspendTeam { .. }
            | Self::ResumeTeam { .. }
            | Self::ReleaseWorktree { .. }
            | Self::BindAgentPane { .. }
            | Self::UnbindAgentPane { .. }
            | Self::CancelIntegration { .. } => &[Role::Owner, Role::Lead],
            Self::SubmitIntegration { .. } => &[Role::Developer, Role::Lead],
            Self::ReviewIntegration { .. } => &[Role::Qa],
            Self::AcceptTask { .. } | Self::SubmitHandoff { .. } => &[Role::Developer],
            Self::ReviewHandoff { .. } => &[Role::Qa],
            Self::PutKnowledge { .. } | Self::CancelRecruitment { .. } => {
                &[Role::Owner, Role::Lead]
            }
            Self::AppendChat { .. } => ALL_MEMBERS,
            Self::RequestApproval { .. } => &[Role::Lead, Role::Developer, Role::Qa],
            Self::ConsumeApproval { .. } => ALL_MEMBERS,
            Self::SubmitSpecialistHandoff { .. } => &[Role::Specialist],
        }
    }
}

/// Central policy gate, run before any handler. Handlers add scope and state checks of their own;
/// this one guarantees no role reaches a command outside its authority class.
pub(super) fn authorize(actor: &Actor, command: &Command) -> Result<()> {
    if !command.permitted_roles().contains(&actor.role) {
        return Err(Error::new(
            "forbidden",
            format!(
                "{:?} may not submit {} ({:?} authority)",
                actor.role,
                command.kind(),
                command.authority()
            ),
        ));
    }
    Ok(())
}

pub(super) fn action_digest(team_id: &str, action: &ProtectedAction) -> Result<String> {
    let bound = json!({
        "team_id": team_id,
        "kind": action.kind,
        "target": action.target,
        "parameters": canonical(&action.parameters),
    });
    let bytes = serde_json::to_vec(&bound)
        .map_err(|_| Error::new("invalid_params", "action cannot be encoded"))?;
    let mut digest = Sha256::new();
    digest.update(b"moo-teams-action-v1\0");
    digest.update(&bytes);
    Ok(format!("{:x}", digest.finalize()))
}

fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), canonical(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn validate_action(action: &ProtectedAction) -> Result<()> {
    validate_text("action target", &action.target, MAX_TITLE_BYTES, false)?;
    let size = serde_json::to_vec(&action.parameters).map_or(usize::MAX, |bytes| bytes.len());
    if size > MAX_ACTION_PARAMETER_BYTES {
        return Err(Error::new(
            "invalid_params",
            format!("action parameters must be at most {MAX_ACTION_PARAMETER_BYTES} bytes"),
        ));
    }
    Ok(())
}

impl State {
    #[allow(clippy::too_many_arguments)] // The request is the full, exact action the Owner will judge.
    pub(super) fn request_approval(
        &mut self,
        actor: &Actor,
        approval_id: String,
        team_id: String,
        action: ProtectedAction,
        justification: String,
        alternatives: Vec<String>,
        risks: Vec<String>,
        ttl_seconds: u64,
        revision: u64,
    ) -> Result<Applied> {
        let team = self.require_team_member_or_owner(actor, &team_id)?.clone();
        validate_id("approval_id", &approval_id)?;
        for (what, items) in [("alternative", &alternatives), ("risk", &risks)] {
            if items.len() > MAX_APPROVAL_NOTES {
                return Err(Error::new(
                    "invalid_params",
                    format!("name at most {MAX_APPROVAL_NOTES} {what}s"),
                ));
            }
            for item in items {
                validate_text(what, item, MAX_TITLE_BYTES, false)?;
            }
        }
        validate_text("justification", &justification, MAX_MESSAGE_BYTES, false)?;
        validate_action(&action)?;
        if !(MIN_APPROVAL_TTL_SECONDS..=MAX_APPROVAL_TTL_SECONDS).contains(&ttl_seconds) {
            return Err(Error::new(
                "invalid_params",
                format!(
                    "ttl_seconds must be between {MIN_APPROVAL_TTL_SECONDS} and {MAX_APPROVAL_TTL_SECONDS}"
                ),
            ));
        }
        if self.approvals.contains_key(&approval_id) {
            return Err(Error::new("already_exists", "approval already exists"));
        }
        let pending = self
            .approvals
            .values()
            .filter(|approval| {
                approval.team_id == team_id && matches!(approval.status, ApprovalStatus::Pending)
            })
            .count();
        if pending >= MAX_PENDING_APPROVALS_PER_TEAM {
            return Err(Error::new(
                "capacity_exceeded",
                "the team has too many approvals awaiting the Owner",
            ));
        }
        let approval = Approval {
            id: approval_id.clone(),
            team_id: team_id.clone(),
            requested_by: actor.id.clone(),
            action_digest: action_digest(&team_id, &action)?,
            action,
            justification,
            alternatives,
            risks,
            ttl_seconds,
            requested_at: self.now,
            status: ApprovalStatus::Pending,
            revision,
        };
        self.approvals.insert(approval_id.clone(), approval.clone());
        Ok(applied(
            serialize_value(&approval)?,
            Some(team_id.clone()),
            Some(team.project_id),
            "approval_requested",
            json!({
                "team_id": team_id,
                "approval_id": approval_id,
                "kind": approval.action.kind,
                "target": approval.action.target,
                "action_digest": approval.action_digest,
            }),
        ))
    }

    pub(super) fn decide_approval(
        &mut self,
        actor: &Actor,
        approval_id: String,
        approve: bool,
        rationale: String,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        validate_text("rationale", &rationale, MAX_MESSAGE_BYTES, false)?;
        let now = self.now;
        let approval = self
            .approvals
            .get_mut(&approval_id)
            .ok_or_else(|| Error::new("not_found", "approval not found"))?;
        if !matches!(approval.status, ApprovalStatus::Pending) {
            return Err(Error::new("invalid_state", "approval is already decided"));
        }
        approval.status = if approve {
            ApprovalStatus::Approved {
                decided_by: actor.id.clone(),
                rationale,
                expires_at: now.saturating_add(approval.ttl_seconds),
            }
        } else {
            ApprovalStatus::Denied {
                decided_by: actor.id.clone(),
                rationale,
            }
        };
        approval.revision = revision;
        self.approval_applied(
            &approval_id,
            "approval_decided",
            json!({ "approved": approve }),
        )
    }

    pub(super) fn revoke_approval(
        &mut self,
        actor: &Actor,
        approval_id: String,
        rationale: String,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        validate_text("rationale", &rationale, MAX_MESSAGE_BYTES, false)?;
        let approval = self
            .approvals
            .get_mut(&approval_id)
            .ok_or_else(|| Error::new("not_found", "approval not found"))?;
        if !matches!(
            approval.status,
            ApprovalStatus::Pending | ApprovalStatus::Approved { .. }
        ) {
            return Err(Error::new(
                "invalid_state",
                "only a pending or unused approved approval can be revoked",
            ));
        }
        approval.status = ApprovalStatus::Denied {
            decided_by: actor.id.clone(),
            rationale,
        };
        approval.revision = revision;
        self.approval_applied(&approval_id, "approval_revoked", json!({}))
    }

    /// Spends an approval for exactly the action it was granted for. This records the authorization
    /// and its one-time use; performing the action is the caller's job (see the Phase 2 report).
    pub(super) fn consume_approval(
        &mut self,
        actor: &Actor,
        approval_id: String,
        execution_id: String,
        action: ProtectedAction,
        revision: u64,
    ) -> Result<Applied> {
        self.spend_approval(actor, &approval_id, &execution_id, &action, revision)?;
        self.approval_applied(
            &approval_id,
            "approval_consumed",
            json!({ "execution_id": execution_id }),
        )
    }

    /// The single place an approval is checked and spent: who, state, expiry and exact action.
    pub(super) fn spend_approval(
        &mut self,
        actor: &Actor,
        approval_id: &str,
        execution_id: &str,
        action: &ProtectedAction,
        revision: u64,
    ) -> Result<()> {
        validate_id("execution_id", execution_id)?;
        validate_action(action)?;
        let now = self.now;
        let approval = self
            .approvals
            .get(approval_id)
            .ok_or_else(|| Error::new("not_found", "approval not found"))?;
        let allowed = actor.role == Role::Owner
            || (actor.team_id.as_deref() == Some(approval.team_id.as_str())
                && (actor.id == approval.requested_by || actor.role == Role::Lead));
        if !allowed {
            return Err(Error::new(
                "forbidden",
                "only the requester, the team Lead or the Owner may use this approval",
            ));
        }
        match &approval.status {
            ApprovalStatus::Approved { expires_at, .. } if now <= *expires_at => {}
            ApprovalStatus::Approved { .. } => {
                return Err(Error::new("approval_expired", "approval has expired"));
            }
            ApprovalStatus::Used { .. } => {
                return Err(Error::new("approval_used", "approval was already used"));
            }
            _ => return Err(Error::new("not_approved", "approval is not granted")),
        }
        if action_digest(&approval.team_id, action)? != approval.action_digest {
            return Err(Error::new(
                "approval_mismatch",
                "the action differs from what the Owner approved",
            ));
        }
        let approval = self
            .approvals
            .get_mut(approval_id)
            .ok_or_else(|| Error::new("corrupt_state", "approval disappeared"))?;
        approval.status = ApprovalStatus::Used {
            execution_id: execution_id.to_owned(),
            consumed_by: actor.id.clone(),
            at: now,
        };
        approval.revision = revision;
        Ok(())
    }

    fn approval_applied(
        &self,
        approval_id: &str,
        kind: &'static str,
        extra: Value,
    ) -> Result<Applied> {
        let approval = self
            .approvals
            .get(approval_id)
            .ok_or_else(|| Error::new("corrupt_state", "approval disappeared"))?;
        let project_id = self.require_team(&approval.team_id)?.project_id.clone();
        let mut payload = json!({
            "team_id": approval.team_id,
            "approval_id": approval.id,
            "action_digest": approval.action_digest,
        });
        if let (Some(payload), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
            payload.extend(extra.clone());
        }
        Ok(applied(
            serialize_value(approval)?,
            Some(approval.team_id.clone()),
            Some(project_id),
            kind,
            payload,
        ))
    }
}

/// A request for one bounded specialist engagement.
pub(super) struct RecruitmentRequest {
    pub id: String,
    pub team_id: String,
    pub specialty: String,
    pub justification: String,
    pub deliverable: String,
    pub capabilities: Vec<String>,
    pub dependencies: Vec<String>,
    pub stop_condition: String,
    pub max_retries: u8,
    pub max_minutes: u32,
}

impl State {
    fn slots_in_use(&self, team_id: Option<&str>) -> usize {
        self.recruitments
            .values()
            .filter(|recruitment| {
                recruitment.status.holds_slot()
                    && team_id.is_none_or(|team_id| recruitment.team_id == team_id)
            })
            .count()
    }

    fn has_free_slot(&self, team_id: &str) -> bool {
        self.slots_in_use(Some(team_id)) < MAX_SPECIALISTS_PER_TEAM
            && self.slots_in_use(None) < MAX_SPECIALISTS_PER_PROJECT
    }

    pub(super) fn request_recruitment(
        &mut self,
        actor: &Actor,
        request: RecruitmentRequest,
        revision: u64,
    ) -> Result<Applied> {
        let team = self
            .require_team_actor(actor, &request.team_id, Role::Lead)?
            .clone();
        validate_id("recruitment_id", &request.id)?;
        validate_id("specialty", &request.specialty)?;
        validate_text(
            "justification",
            &request.justification,
            MAX_MESSAGE_BYTES,
            false,
        )?;
        validate_text(
            "deliverable",
            &request.deliverable,
            MAX_MESSAGE_BYTES,
            false,
        )?;
        validate_text(
            "stop condition",
            &request.stop_condition,
            MAX_MESSAGE_BYTES,
            false,
        )?;
        if request.capabilities.is_empty()
            || request.capabilities.len() > SPECIALIST_CAPABILITIES.len()
        {
            return Err(Error::new(
                "invalid_params",
                "name between one and four capabilities",
            ));
        }
        for capability in &request.capabilities {
            if !SPECIALIST_CAPABILITIES.contains(&capability.as_str()) {
                return Err(Error::new(
                    "escalation_required",
                    format!("capability '{capability}' needs an Owner-approved rights escalation"),
                ));
            }
        }
        if request.max_retries > MAX_SPECIALIST_RETRIES
            || request.max_minutes == 0
            || request.max_minutes > MAX_SPECIALIST_MINUTES
        {
            return Err(Error::new(
                "invalid_params",
                format!(
                    "max_retries must be at most {MAX_SPECIALIST_RETRIES} and max_minutes between 1 and {MAX_SPECIALIST_MINUTES}"
                ),
            ));
        }
        for task_id in &request.dependencies {
            match self.tasks.get(task_id) {
                Some(task) if task.team_id == request.team_id && !task.status.is_terminal() => {}
                _ => {
                    return Err(Error::new(
                        "invalid_reference",
                        "dependency must be an open task of this team",
                    ));
                }
            }
        }
        if self.recruitments.contains_key(&request.id) {
            return Err(Error::new("already_exists", "recruitment already exists"));
        }
        self.ensure_within_usage_limits(&request.team_id)?;
        let total = self
            .recruitments
            .values()
            .filter(|recruitment| recruitment.team_id == request.team_id)
            .count();
        if total >= MAX_RECRUITMENTS_PER_TEAM {
            return Err(Error::new(
                "capacity_exceeded",
                "the team reached its recruitment limit",
            ));
        }
        let status = if self.has_free_slot(&request.team_id) {
            RecruitmentStatus::Authorized
        } else {
            RecruitmentStatus::Queued
        };
        let recruitment = Recruitment {
            id: request.id.clone(),
            team_id: request.team_id.clone(),
            requested_by: actor.id.clone(),
            specialty: request.specialty,
            justification: request.justification,
            deliverable: request.deliverable,
            capabilities: request.capabilities,
            dependencies: request.dependencies,
            stop_condition: request.stop_condition,
            max_retries: request.max_retries,
            max_minutes: request.max_minutes,
            status,
            revision,
        };
        self.recruitments
            .insert(recruitment.id.clone(), recruitment.clone());
        Ok(applied(
            serialize_value(&recruitment)?,
            Some(request.team_id),
            Some(team.project_id),
            "recruitment_requested",
            json!({
                "recruitment_id": recruitment.id,
                "specialty": recruitment.specialty,
                "status": recruitment.status,
            }),
        ))
    }

    pub(super) fn activate_recruitment(
        &mut self,
        actor: &Actor,
        recruitment_id: String,
        actor_id: String,
        credential_digest: String,
        run_generation: u64,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        validate_id("actor_id", &actor_id)?;
        super::validate_digest("credential_digest", &credential_digest)?;
        if run_generation == 0 {
            return Err(Error::new(
                "invalid_params",
                "run_generation must be greater than zero",
            ));
        }
        if self.actors.contains_key(&actor_id) || self.credentials.contains_key(&credential_digest)
        {
            return Err(Error::new(
                "already_exists",
                "actor or credential is already enrolled",
            ));
        }
        let now = self.now;
        let recruitment = self
            .recruitments
            .get_mut(&recruitment_id)
            .ok_or_else(|| Error::new("not_found", "recruitment not found"))?;
        if !matches!(recruitment.status, RecruitmentStatus::Authorized) {
            return Err(Error::new(
                "invalid_state",
                "only an authorized recruitment can be activated",
            ));
        }
        recruitment.status = RecruitmentStatus::Active {
            actor_id: actor_id.clone(),
            run_generation,
            deadline: now.saturating_add(u64::from(recruitment.max_minutes) * 60),
            attempt: 1,
        };
        recruitment.revision = revision;
        let team_id = recruitment.team_id.clone();
        self.credentials.insert(
            credential_digest.clone(),
            Credential {
                actor_id: actor_id.clone(),
                run_generation: Some(run_generation),
                active: true,
            },
        );
        self.actors.insert(
            actor_id.clone(),
            Actor {
                id: actor_id.clone(),
                role: Role::Specialist,
                team_id: Some(team_id.clone()),
                run_generation: Some(run_generation),
                auth_digest: credential_digest,
            },
        );
        self.recruitment_applied(
            &recruitment_id,
            "recruitment_activated",
            json!({ "actor_id": actor_id }),
        )
    }

    pub(super) fn submit_specialist_handoff(
        &mut self,
        actor: &Actor,
        recruitment_id: String,
        summary: String,
        evidence: Vec<Evidence>,
        revision: u64,
    ) -> Result<Applied> {
        validate_text("handoff summary", &summary, MAX_MESSAGE_BYTES, false)?;
        validate_evidence(&evidence, true)?;
        let now = self.now;
        let recruitment = self
            .recruitments
            .get_mut(&recruitment_id)
            .ok_or_else(|| Error::new("not_found", "recruitment not found"))?;
        let RecruitmentStatus::Active {
            actor_id,
            deadline,
            attempt,
            ..
        } = recruitment.status.clone()
        else {
            return Err(Error::new("invalid_state", "recruitment is not active"));
        };
        if actor_id != actor.id {
            return Err(Error::new(
                "forbidden",
                "this engagement belongs to another specialist",
            ));
        }
        if now > deadline {
            return Err(Error::new(
                "deadline_exceeded",
                "the engagement passed its time allocation; the Lead must cancel or verify",
            ));
        }
        recruitment.status = RecruitmentStatus::HandedOff {
            actor_id,
            summary,
            evidence,
            attempt,
        };
        recruitment.revision = revision;
        self.recruitment_applied(&recruitment_id, "recruitment_handed_off", json!({}))
    }

    pub(super) fn verify_recruitment(
        &mut self,
        actor: &Actor,
        recruitment_id: String,
        accepted: bool,
        rationale: String,
        revision: u64,
    ) -> Result<Applied> {
        validate_text("rationale", &rationale, MAX_MESSAGE_BYTES, false)?;
        let now = self.now;
        let recruitment = self
            .recruitments
            .get(&recruitment_id)
            .ok_or_else(|| Error::new("not_found", "recruitment not found"))?;
        self.require_team_actor(actor, &recruitment.team_id, Role::Lead)?;
        let RecruitmentStatus::HandedOff {
            actor_id, attempt, ..
        } = recruitment.status.clone()
        else {
            return Err(Error::new(
                "invalid_state",
                "no handoff is awaiting verification",
            ));
        };
        let retry = !accepted && attempt <= recruitment.max_retries;
        let minutes = u64::from(recruitment.max_minutes);
        let recruitment = self
            .recruitments
            .get_mut(&recruitment_id)
            .ok_or_else(|| Error::new("corrupt_state", "recruitment disappeared"))?;
        recruitment.revision = revision;
        if retry {
            recruitment.status = RecruitmentStatus::Active {
                run_generation: self
                    .actors
                    .get(&actor_id)
                    .and_then(|specialist| specialist.run_generation)
                    .unwrap_or_default(),
                actor_id,
                deadline: now.saturating_add(minutes * 60),
                attempt: attempt + 1,
            };
        } else {
            recruitment.status = RecruitmentStatus::Released {
                accepted,
                rationale,
            };
            self.release_specialist(&actor_id, revision);
        }
        self.promote_queued(revision);
        self.recruitment_applied(
            &recruitment_id,
            "recruitment_verified",
            json!({ "accepted": accepted, "retry": retry }),
        )
    }

    pub(super) fn cancel_recruitment(
        &mut self,
        actor: &Actor,
        recruitment_id: String,
        reason: String,
        revision: u64,
    ) -> Result<Applied> {
        validate_text("reason", &reason, MAX_MESSAGE_BYTES, false)?;
        let recruitment = self
            .recruitments
            .get(&recruitment_id)
            .ok_or_else(|| Error::new("not_found", "recruitment not found"))?;
        self.require_team_member_or_owner(actor, &recruitment.team_id)?;
        let specialist = match &recruitment.status {
            RecruitmentStatus::Queued | RecruitmentStatus::Authorized => None,
            RecruitmentStatus::Active { actor_id, .. }
            | RecruitmentStatus::HandedOff { actor_id, .. } => Some(actor_id.clone()),
            _ => {
                return Err(Error::new(
                    "invalid_state",
                    "recruitment is already finished",
                ))
            }
        };
        let recruitment = self
            .recruitments
            .get_mut(&recruitment_id)
            .ok_or_else(|| Error::new("corrupt_state", "recruitment disappeared"))?;
        recruitment.status = RecruitmentStatus::Cancelled { reason };
        recruitment.revision = revision;
        if let Some(actor_id) = specialist {
            self.release_specialist(&actor_id, revision);
        }
        self.promote_queued(revision);
        self.recruitment_applied(&recruitment_id, "recruitment_cancelled", json!({}))
    }

    /// The process is gone: its credential stops authenticating. The actor record stays for audit.
    fn release_specialist(&mut self, actor_id: &str, _revision: u64) {
        if let Some(digest) = self
            .actors
            .get(actor_id)
            .map(|actor| actor.auth_digest.clone())
        {
            if let Some(credential) = self.credentials.get_mut(&digest) {
                credential.active = false;
            }
        }
    }

    /// Frees capacity for the oldest queued requests, in request order.
    fn promote_queued(&mut self, revision: u64) {
        let mut queued: Vec<(u64, String, String)> = self
            .recruitments
            .values()
            .filter(|recruitment| matches!(recruitment.status, RecruitmentStatus::Queued))
            .map(|recruitment| {
                (
                    recruitment.revision,
                    recruitment.id.clone(),
                    recruitment.team_id.clone(),
                )
            })
            .collect();
        queued.sort();
        for (_, id, team_id) in queued {
            if !self.has_free_slot(&team_id) {
                continue;
            }
            if let Some(recruitment) = self.recruitments.get_mut(&id) {
                recruitment.status = RecruitmentStatus::Authorized;
                recruitment.revision = revision;
            }
        }
    }

    fn recruitment_applied(
        &self,
        recruitment_id: &str,
        kind: &'static str,
        extra: Value,
    ) -> Result<Applied> {
        let recruitment = self
            .recruitments
            .get(recruitment_id)
            .ok_or_else(|| Error::new("corrupt_state", "recruitment disappeared"))?;
        let project_id = self.require_team(&recruitment.team_id)?.project_id.clone();
        let mut payload = json!({
            "team_id": recruitment.team_id,
            "recruitment_id": recruitment.id,
            "status": recruitment.status,
        });
        if let (Some(payload), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
            payload.extend(extra.clone());
        }
        Ok(applied(
            serialize_value(recruitment)?,
            Some(recruitment.team_id.clone()),
            Some(project_id),
            kind,
            payload,
        ))
    }

    pub(super) fn validate_governance(&self) -> Result<()> {
        let corrupt = |message: &str| Error::new("corrupt_state", message.to_owned());
        for approval in self.approvals.values() {
            if !self.teams.contains_key(&approval.team_id) {
                return Err(corrupt("approval names an unknown team"));
            }
            if action_digest(&approval.team_id, &approval.action)? != approval.action_digest {
                return Err(corrupt("approval digest does not match its action"));
            }
        }
        for recruitment in self.recruitments.values() {
            if !self.teams.contains_key(&recruitment.team_id) {
                return Err(corrupt("recruitment names an unknown team"));
            }
            if let RecruitmentStatus::Active { actor_id, .. }
            | RecruitmentStatus::HandedOff { actor_id, .. } = &recruitment.status
            {
                match self.actors.get(actor_id) {
                    Some(specialist) if specialist.role == Role::Specialist => {}
                    _ => return Err(corrupt("recruitment names a missing specialist")),
                }
            }
            if recruitment
                .dependencies
                .iter()
                .any(|task_id| !self.tasks.contains_key(task_id))
            {
                return Err(corrupt("recruitment names an unknown task"));
            }
        }
        for team_id in self.teams.keys() {
            if self.slots_in_use(Some(team_id)) > MAX_SPECIALISTS_PER_TEAM {
                return Err(corrupt("team exceeds its specialist limit"));
            }
        }
        if self.slots_in_use(None) > MAX_SPECIALISTS_PER_PROJECT {
            return Err(corrupt("project exceeds its specialist limit"));
        }
        Ok(())
    }
}

impl State {
    /// A cancelled team's specialists are stopped and their credentials die with them.
    pub(super) fn close_team_recruitments(&mut self, team_id: &str, revision: u64) {
        let open: Vec<(String, Option<String>)> = self
            .recruitments
            .values()
            .filter(|item| item.team_id == team_id)
            .filter_map(|item| match &item.status {
                RecruitmentStatus::Queued | RecruitmentStatus::Authorized => {
                    Some((item.id.clone(), None))
                }
                RecruitmentStatus::Active { actor_id, .. }
                | RecruitmentStatus::HandedOff { actor_id, .. } => {
                    Some((item.id.clone(), Some(actor_id.clone())))
                }
                _ => None,
            })
            .collect();
        for (id, specialist) in open {
            if let Some(recruitment) = self.recruitments.get_mut(&id) {
                recruitment.status = RecruitmentStatus::Cancelled {
                    reason: "team cancelled".into(),
                };
                recruitment.revision = revision;
            }
            if let Some(actor_id) = specialist {
                self.release_specialist(&actor_id, revision);
            }
        }
        self.promote_queued(revision);
    }
}

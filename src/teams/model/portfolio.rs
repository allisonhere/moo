//! Several teams in one project: lifecycle, worktree ownership, shared surfaces, and the
//! project-level integration queue with explicit conflicts.
//!
//! Nothing here touches git or the filesystem. The server records claims, evidence and decisions;
//! a harness performs worktree creation, merges and deletion, and must consult these records first.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::governance::{ProtectedAction, ProtectedActionKind};
use super::{
    applied, require_role, serialize_value, validate_evidence, validate_id, validate_text, Actor,
    Applied, Command, Evidence, Role, State, MAX_MESSAGE_BYTES, MAX_NAME_BYTES,
};
use crate::teams::{Error, Result};

const MAX_ACTIVE_TEAMS_PER_PROJECT: usize = 4;
const MAX_ACTIVE_CLAIMS_PER_PROJECT: usize = 8;
const MAX_PATH_BYTES: usize = 512;
const MAX_CHANGED_PATHS: usize = 512;
const MAX_SURFACES_PER_INTEGRATION: usize = 32;
const MAX_PREFIXES_PER_SURFACE: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TeamStatus {
    #[default]
    Active,
    Suspended {
        reason: String,
    },
    Cancelled {
        reason: String,
    },
}

/// A named public interface, schema or API whose paths are known to the project, so a change to
/// it is noticed even when the files two teams touch are disjoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedSurface {
    pub id: String,
    pub project_id: String,
    pub path_prefixes: Vec<String>,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    Active,
    Released,
}

/// A team's exclusive right to one worktree path and branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeClaim {
    pub id: String,
    pub project_id: String,
    pub team_id: String,
    pub path: String,
    pub branch: String,
    pub base_commit: String,
    pub status: ClaimStatus,
    /// Work was submitted from this worktree; it is never approved for cleanup.
    pub used: bool,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationReview {
    pub reviewer_id: String,
    pub accepted: bool,
    pub rationale: String,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum IntegrationStatus {
    AwaitingReview,
    /// Accepted by QA with no open conflict; only the Owner can move it on.
    Ready,
    /// An open conflict names it.
    Blocked,
    /// The Owner spent an approval and the merge is being performed.
    Merging {
        approval_id: String,
    },
    /// The server stopped while merging; the Owner must record what actually happened.
    MergeUnknown {
        approval_id: String,
    },
    Merged {
        merge_commit: String,
    },
    Rejected {
        reason: String,
    },
    Cancelled {
        reason: String,
    },
}

impl IntegrationStatus {
    fn is_open(&self) -> bool {
        matches!(
            self,
            Self::AwaitingReview
                | Self::Ready
                | Self::Blocked
                | Self::Merging { .. }
                | Self::MergeUnknown { .. }
        )
    }

    /// Statuses whose readiness is derived from the review and the conflicts.
    fn is_queued(&self) -> bool {
        matches!(self, Self::AwaitingReview | Self::Ready | Self::Blocked)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Integration {
    pub id: String,
    pub project_id: String,
    pub team_id: String,
    pub claim_id: String,
    pub source_branch: String,
    pub target_branch: String,
    pub commit: String,
    pub base_commit: String,
    pub changed_paths: Vec<String>,
    /// Surfaces this change alters: declared by the team plus those its paths fall under.
    pub touches: BTreeSet<String>,
    /// Surfaces this change relies on.
    pub depends_on: BTreeSet<String>,
    pub tests: Vec<Evidence>,
    pub review: Option<IntegrationReview>,
    pub status: IntegrationStatus,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    PathOverlap,
    SharedSurface,
    DependencyImpact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ConflictStatus {
    Open,
    Resolved { by: String, rationale: String },
}

/// An explicit review request: two open integrations that cannot both land unexamined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub id: String,
    pub project_id: String,
    pub kind: ConflictKind,
    pub integration_ids: [String; 2],
    pub team_ids: [String; 2],
    pub detail: Vec<String>,
    pub status: ConflictStatus,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NoticeStatus {
    Open,
    Acknowledged { by: String },
}

/// Structured heads-up to another team that a change affects something it touches or relies on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeNotice {
    pub id: String,
    pub project_id: String,
    pub from_team_id: String,
    pub to_team_id: String,
    pub integration_id: String,
    pub surfaces: Vec<String>,
    pub paths: Vec<String>,
    pub status: NoticeStatus,
    pub revision: u64,
}

/// The fields of an integration submission.
pub(super) struct Submission {
    pub id: String,
    pub team_id: String,
    pub claim_id: String,
    pub target_branch: String,
    pub commit: String,
    pub changed_paths: Vec<String>,
    pub touches: Vec<String>,
    pub depends_on: Vec<String>,
    pub tests: Vec<Evidence>,
}

impl Command {
    /// Commands that stay available to a suspended or cancelled team, because they wind work down.
    fn runs_while_inactive(&self) -> bool {
        matches!(
            self,
            Self::SuspendTeam { .. }
                | Self::ResumeTeam { .. }
                | Self::CancelTeam { .. }
                | Self::RenameTeam { .. }
                | Self::ReleaseWorktree { .. }
                | Self::CancelIntegration { .. }
                | Self::AbortMerge { .. }
                | Self::CompleteMerge { .. }
                | Self::CancelRecruitment { .. }
                | Self::RevokeApproval { .. }
                | Self::ResolveConflict { .. }
                | Self::AcknowledgeNotice { .. }
        )
    }
}

impl State {
    /// The team a command acts for, found from the command or the record it names.
    fn command_team_id(&self, command: &Command) -> Option<String> {
        let by_integration = |id: &str| self.integrations.get(id).map(|item| item.team_id.clone());
        match command {
            Command::CreateTask { team_id, .. }
            | Command::SetTaskDependencies { team_id, .. }
            | Command::AcceptTask { team_id, .. }
            | Command::SubmitHandoff { team_id, .. }
            | Command::ReviewHandoff { team_id, .. }
            | Command::FinalizeTask { team_id, .. }
            | Command::CancelTask { team_id, .. }
            | Command::AppendChat { team_id, .. }
            | Command::PutKnowledge { team_id, .. }
            | Command::RecordDispatchReceipt { team_id, .. }
            | Command::RetryDispatch { team_id, .. }
            | Command::RequestApproval { team_id, .. }
            | Command::RequestRecruitment { team_id, .. }
            | Command::SuspendTeam { team_id, .. }
            | Command::ResumeTeam { team_id }
            | Command::CancelTeam { team_id, .. }
            | Command::RenameTeam { team_id, .. }
            | Command::ClaimWorktree { team_id, .. }
            | Command::SubmitIntegration { team_id, .. } => Some(team_id.clone()),
            Command::BindAgentPane { actor_id, .. } | Command::UnbindAgentPane { actor_id } => self
                .actors
                .get(actor_id)
                .and_then(|agent| agent.team_id.clone()),
            Command::DecideApproval { approval_id, .. }
            | Command::RevokeApproval { approval_id, .. }
            | Command::ConsumeApproval { approval_id, .. } => self
                .approvals
                .get(approval_id)
                .map(|item| item.team_id.clone()),
            Command::ActivateRecruitment { recruitment_id, .. }
            | Command::SubmitSpecialistHandoff { recruitment_id, .. }
            | Command::VerifyRecruitment { recruitment_id, .. }
            | Command::CancelRecruitment { recruitment_id, .. } => self
                .recruitments
                .get(recruitment_id)
                .map(|item| item.team_id.clone()),
            Command::ReleaseWorktree { claim_id }
            | Command::ApproveWorktreeCleanup { claim_id } => self
                .worktrees
                .get(claim_id)
                .map(|item| item.team_id.clone()),
            Command::ReviewIntegration { integration_id, .. }
            | Command::CancelIntegration { integration_id, .. }
            | Command::BeginMerge { integration_id, .. }
            | Command::CompleteMerge { integration_id, .. }
            | Command::AbortMerge { integration_id, .. } => by_integration(integration_id),
            Command::AcknowledgeNotice { notice_id } => self
                .notices
                .get(notice_id)
                .map(|item| item.to_team_id.clone()),
            Command::CreateProject { .. }
            | Command::CreateTeamSpec { .. }
            | Command::EnrollActor { .. }
            | Command::LaunchTeam { .. }
            | Command::RegisterSurface { .. }
            | Command::ResolveConflict { .. } => None,
        }
    }

    /// Only an Active team may start or advance work; winding down stays possible.
    pub(super) fn ensure_team_may_work(&self, command: &Command) -> Result<()> {
        if command.runs_while_inactive() {
            return Ok(());
        }
        let Some(team_id) = self.command_team_id(command) else {
            return Ok(());
        };
        match self.teams.get(&team_id).map(|team| &team.status) {
            Some(TeamStatus::Suspended { .. }) => Err(Error::new(
                "team_suspended",
                "the team is suspended; resume it to continue",
            )),
            Some(TeamStatus::Cancelled { .. }) => {
                Err(Error::new("team_cancelled", "the team was cancelled"))
            }
            _ => Ok(()),
        }
    }

    fn active_teams_in(&self, project_id: &str) -> usize {
        self.teams
            .values()
            .filter(|team| team.project_id == project_id && team.status == TeamStatus::Active)
            .count()
    }

    pub(super) fn ensure_project_has_team_capacity(&self, project_id: &str) -> Result<()> {
        if self.active_teams_in(project_id) >= MAX_ACTIVE_TEAMS_PER_PROJECT {
            return Err(Error::new(
                "project_capacity",
                format!("the project already runs {MAX_ACTIVE_TEAMS_PER_PROJECT} active teams"),
            ));
        }
        Ok(())
    }

    pub(super) fn rename_team(
        &mut self,
        actor: &Actor,
        team_id: String,
        name: String,
        revision: u64,
    ) -> Result<Applied> {
        self.require_team_member_or_owner(actor, &team_id)?;
        validate_text("team name", &name, MAX_NAME_BYTES, false)?;
        let team = self
            .teams
            .get_mut(&team_id)
            .ok_or_else(|| Error::new("not_found", "team not found"))?;
        let previous = std::mem::replace(&mut team.name, name.clone());
        team.revision = revision;
        let project_id = team.project_id.clone();
        let result = serialize_value(&*team)?;
        Ok(applied(
            result,
            Some(team_id.clone()),
            Some(project_id),
            "team_renamed",
            json!({ "team_id": team_id, "from": previous, "to": name }),
        ))
    }

    pub(super) fn suspend_team(
        &mut self,
        actor: &Actor,
        team_id: String,
        reason: String,
        revision: u64,
    ) -> Result<Applied> {
        self.require_team_member_or_owner(actor, &team_id)?;
        validate_text("reason", &reason, MAX_MESSAGE_BYTES, false)?;
        self.set_team_status(
            &team_id,
            TeamStatus::Suspended { reason },
            revision,
            "team_suspended",
        )
    }

    pub(super) fn resume_team(
        &mut self,
        actor: &Actor,
        team_id: String,
        revision: u64,
    ) -> Result<Applied> {
        let team = self.require_team_member_or_owner(actor, &team_id)?;
        if !matches!(team.status, TeamStatus::Suspended { .. }) {
            return Err(Error::new(
                "invalid_state",
                "only a suspended team can resume",
            ));
        }
        let project_id = team.project_id.clone();
        self.ensure_project_has_team_capacity(&project_id)?;
        self.set_team_status(&team_id, TeamStatus::Active, revision, "team_resumed")
    }

    /// Stops the team for good: queued work is withdrawn, specialists and approvals are closed,
    /// worktrees are released but never deleted, and recorded history is kept.
    pub(super) fn cancel_team(
        &mut self,
        actor: &Actor,
        team_id: String,
        reason: String,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        validate_text("reason", &reason, MAX_MESSAGE_BYTES, false)?;
        if matches!(
            self.require_team(&team_id)?.status,
            TeamStatus::Cancelled { .. }
        ) {
            return Err(Error::new("invalid_state", "team is already cancelled"));
        }
        for integration in self
            .integrations
            .values_mut()
            .filter(|item| item.team_id == team_id && item.status.is_queued())
        {
            integration.status = IntegrationStatus::Cancelled {
                reason: "team cancelled".into(),
            };
            integration.revision = revision;
        }
        for claim in self
            .worktrees
            .values_mut()
            .filter(|item| item.team_id == team_id && item.status == ClaimStatus::Active)
        {
            claim.status = ClaimStatus::Released;
            claim.revision = revision;
        }
        for approval in self.approvals.values_mut().filter(|item| {
            item.team_id == team_id
                && matches!(
                    item.status,
                    super::governance::ApprovalStatus::Pending
                        | super::governance::ApprovalStatus::Approved { .. }
                )
        }) {
            approval.status = super::governance::ApprovalStatus::Denied {
                decided_by: actor.id.clone(),
                rationale: "team cancelled".into(),
            };
            approval.revision = revision;
        }
        for attempt in self
            .dispatches
            .values_mut()
            .filter(|item| item.team_id == team_id && !item.state.is_terminal())
        {
            attempt.state = super::DispatchState::Cancelled;
            attempt.revision = revision;
        }
        self.close_team_recruitments(&team_id, revision);
        self.refresh_all_integrations(revision);
        self.set_team_status(
            &team_id,
            TeamStatus::Cancelled { reason },
            revision,
            "team_cancelled",
        )
    }

    fn set_team_status(
        &mut self,
        team_id: &str,
        status: TeamStatus,
        revision: u64,
        kind: &'static str,
    ) -> Result<Applied> {
        let team = self
            .teams
            .get_mut(team_id)
            .ok_or_else(|| Error::new("not_found", "team not found"))?;
        if matches!(team.status, TeamStatus::Cancelled { .. }) {
            return Err(Error::new("team_cancelled", "the team was cancelled"));
        }
        team.status = status;
        team.revision = revision;
        let project_id = team.project_id.clone();
        let result = serialize_value(&*team)?;
        Ok(applied(
            result,
            Some(team_id.to_owned()),
            Some(project_id),
            kind,
            json!({ "team_id": team_id, "team_status": self.teams[team_id].status }),
        ))
    }

    pub(super) fn register_surface(
        &mut self,
        actor: &Actor,
        surface_id: String,
        project_id: String,
        path_prefixes: Vec<String>,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        validate_id("surface_id", &surface_id)?;
        self.require_project(&project_id)?;
        if self.surfaces.contains_key(&surface_id) {
            return Err(Error::new("already_exists", "surface already exists"));
        }
        if path_prefixes.is_empty() || path_prefixes.len() > MAX_PREFIXES_PER_SURFACE {
            return Err(Error::new(
                "invalid_params",
                format!("a surface names between 1 and {MAX_PREFIXES_PER_SURFACE} path prefixes"),
            ));
        }
        let path_prefixes = path_prefixes
            .iter()
            .map(|prefix| normalize_repo_path(prefix))
            .collect::<Result<Vec<_>>>()?;
        let surface = SharedSurface {
            id: surface_id.clone(),
            project_id: project_id.clone(),
            path_prefixes,
            revision,
        };
        self.surfaces.insert(surface_id.clone(), surface.clone());
        Ok(applied(
            serialize_value(&surface)?,
            None,
            Some(project_id),
            "surface_registered",
            json!({ "surface_id": surface_id }),
        ))
    }

    pub(super) fn claim_worktree(
        &mut self,
        actor: &Actor,
        claim_id: String,
        team_id: String,
        path: String,
        branch: String,
        base_commit: String,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        validate_id("claim_id", &claim_id)?;
        let team = self.require_team(&team_id)?.clone();
        let path = normalize_worktree_path(&path)?;
        validate_branch(&branch)?;
        validate_commit("base_commit", &base_commit)?;
        if self.worktrees.contains_key(&claim_id) {
            return Err(Error::new(
                "already_exists",
                "worktree claim already exists",
            ));
        }
        let active: Vec<&WorktreeClaim> = self
            .worktrees
            .values()
            .filter(|claim| {
                claim.project_id == team.project_id && claim.status == ClaimStatus::Active
            })
            .collect();
        if active.len() >= MAX_ACTIVE_CLAIMS_PER_PROJECT {
            return Err(Error::new(
                "project_capacity",
                format!(
                    "the project already holds {MAX_ACTIVE_CLAIMS_PER_PROJECT} active worktrees"
                ),
            ));
        }
        if let Some(other) = active.iter().find(|claim| paths_nest(&claim.path, &path)) {
            return Err(Error::new(
                "worktree_conflict",
                format!(
                    "path overlaps worktree '{}' held by team '{}'",
                    other.id, other.team_id
                ),
            ));
        }
        if let Some(other) = active.iter().find(|claim| claim.branch == branch) {
            return Err(Error::new(
                "worktree_conflict",
                format!("branch is already checked out by worktree '{}'", other.id),
            ));
        }
        let claim = WorktreeClaim {
            id: claim_id.clone(),
            project_id: team.project_id.clone(),
            team_id: team_id.clone(),
            path,
            branch,
            base_commit,
            status: ClaimStatus::Active,
            used: false,
            revision,
        };
        self.worktrees.insert(claim_id.clone(), claim.clone());
        Ok(applied(
            serialize_value(&claim)?,
            Some(team_id),
            Some(team.project_id),
            "worktree_claimed",
            json!({ "claim_id": claim_id, "path": claim.path, "branch": claim.branch }),
        ))
    }

    pub(super) fn release_worktree(
        &mut self,
        actor: &Actor,
        claim_id: String,
        revision: u64,
    ) -> Result<Applied> {
        let claim = self
            .worktrees
            .get(&claim_id)
            .ok_or_else(|| Error::new("not_found", "worktree claim not found"))?;
        self.require_team_member_or_owner(actor, &claim.team_id)?;
        if actor.role != Role::Owner && actor.role != Role::Lead {
            return Err(Error::new(
                "forbidden",
                "only the Owner or the Lead may release a worktree",
            ));
        }
        if claim.status != ClaimStatus::Active {
            return Err(Error::new("invalid_state", "worktree is already released"));
        }
        if self
            .integrations
            .values()
            .any(|item| item.claim_id == claim_id && item.status.is_open())
        {
            return Err(Error::new(
                "integration_open",
                "an open integration still uses this worktree",
            ));
        }
        let claim = self
            .worktrees
            .get_mut(&claim_id)
            .ok_or_else(|| Error::new("corrupt_state", "claim disappeared"))?;
        claim.status = ClaimStatus::Released;
        claim.revision = revision;
        let (team_id, project_id) = (claim.team_id.clone(), claim.project_id.clone());
        let result = serialize_value(&*claim)?;
        Ok(applied(
            result,
            Some(team_id),
            Some(project_id),
            "worktree_released",
            json!({ "claim_id": claim_id }),
        ))
    }

    /// Says whether a released worktree is safe for a harness to delete. It never deletes anything,
    /// and a worktree that ever carried submitted work is never approved.
    pub(super) fn approve_worktree_cleanup(
        &mut self,
        actor: &Actor,
        claim_id: String,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        let claim = self
            .worktrees
            .get(&claim_id)
            .ok_or_else(|| Error::new("not_found", "worktree claim not found"))?;
        if claim.status != ClaimStatus::Released {
            return Err(Error::new("worktree_in_use", "release the worktree first"));
        }
        if claim.used
            || self
                .integrations
                .values()
                .any(|item| item.claim_id == claim_id)
        {
            return Err(Error::new(
                "worktree_has_work",
                "this worktree carried submitted work and is preserved",
            ));
        }
        let (team_id, project_id) = (claim.team_id.clone(), claim.project_id.clone());
        let claim = self
            .worktrees
            .get_mut(&claim_id)
            .ok_or_else(|| Error::new("corrupt_state", "claim disappeared"))?;
        claim.revision = revision;
        let result = serialize_value(&*claim)?;
        Ok(applied(
            result,
            Some(team_id),
            Some(project_id),
            "worktree_cleanup_approved",
            json!({ "claim_id": claim_id, "path": self.worktrees[&claim_id].path }),
        ))
    }

    pub(super) fn submit_integration(
        &mut self,
        actor: &Actor,
        submission: Submission,
        revision: u64,
    ) -> Result<Applied> {
        let team = self
            .require_team_member_or_owner(actor, &submission.team_id)?
            .clone();
        validate_id("integration_id", &submission.id)?;
        validate_branch(&submission.target_branch)?;
        validate_commit("commit", &submission.commit)?;
        validate_evidence(&submission.tests, true)?;
        if self.integrations.contains_key(&submission.id) {
            return Err(Error::new("already_exists", "integration already exists"));
        }
        let claim = self
            .worktrees
            .get(&submission.claim_id)
            .ok_or_else(|| Error::new("not_found", "worktree claim not found"))?;
        if claim.team_id != submission.team_id || claim.status != ClaimStatus::Active {
            return Err(Error::new(
                "invalid_reference",
                "the worktree is not an active claim of this team",
            ));
        }
        if submission.changed_paths.is_empty() || submission.changed_paths.len() > MAX_CHANGED_PATHS
        {
            return Err(Error::new(
                "invalid_params",
                format!("name between 1 and {MAX_CHANGED_PATHS} changed paths"),
            ));
        }
        // Paths are relative to the claimed worktree, so nothing outside it can be reported.
        let changed_paths: Vec<String> = submission
            .changed_paths
            .iter()
            .map(|path| normalize_repo_path(path))
            .collect::<Result<BTreeSet<_>>>()?
            .into_iter()
            .collect();
        let mut touches = self.resolve_surfaces(&team.project_id, &submission.touches)?;
        let depends_on = self.resolve_surfaces(&team.project_id, &submission.depends_on)?;
        for surface in self
            .surfaces
            .values()
            .filter(|item| item.project_id == team.project_id)
        {
            if changed_paths.iter().any(|path| {
                surface
                    .path_prefixes
                    .iter()
                    .any(|prefix| path_within(path, prefix))
            }) {
                touches.insert(surface.id.clone());
            }
        }
        let integration = Integration {
            id: submission.id.clone(),
            project_id: team.project_id.clone(),
            team_id: submission.team_id.clone(),
            claim_id: submission.claim_id.clone(),
            source_branch: claim.branch.clone(),
            target_branch: submission.target_branch,
            commit: submission.commit,
            base_commit: claim.base_commit.clone(),
            changed_paths,
            touches,
            depends_on,
            tests: submission.tests,
            review: None,
            status: IntegrationStatus::AwaitingReview,
            revision,
        };
        let found = self.detect_conflicts(&integration);
        let mut conflict_ids = Vec::new();
        for (kind, other, detail) in found {
            let id = conflict_id(&integration.id, &other.id, kind);
            let (first, second) = if integration.id < other.id {
                (integration.clone(), other.clone())
            } else {
                (other.clone(), integration.clone())
            };
            self.conflicts.insert(
                id.clone(),
                Conflict {
                    id: id.clone(),
                    project_id: integration.project_id.clone(),
                    kind,
                    integration_ids: [first.id.clone(), second.id.clone()],
                    team_ids: [first.team_id.clone(), second.team_id.clone()],
                    detail: detail.clone(),
                    status: ConflictStatus::Open,
                    revision,
                },
            );
            let notice_id = format!("notice-{}-{}", integration.id, other.team_id);
            let notice = self
                .notices
                .entry(notice_id.clone())
                .or_insert_with(|| ChangeNotice {
                    id: notice_id,
                    project_id: integration.project_id.clone(),
                    from_team_id: integration.team_id.clone(),
                    to_team_id: other.team_id.clone(),
                    integration_id: integration.id.clone(),
                    surfaces: Vec::new(),
                    paths: Vec::new(),
                    status: NoticeStatus::Open,
                    revision,
                });
            match kind {
                ConflictKind::PathOverlap => notice.paths.extend(detail),
                _ => notice.surfaces.extend(detail),
            }
            notice.surfaces.sort();
            notice.surfaces.dedup();
            notice.paths.sort();
            notice.paths.dedup();
            conflict_ids.push(id);
        }
        self.integrations
            .insert(integration.id.clone(), integration.clone());
        if let Some(claim) = self.worktrees.get_mut(&integration.claim_id) {
            claim.used = true;
            claim.revision = revision;
        }
        self.refresh_all_integrations(revision);
        let status = self.integrations[&integration.id].status.clone();
        Ok(applied(
            serialize_value(&self.integrations[&integration.id])?,
            Some(integration.team_id.clone()),
            Some(integration.project_id),
            "integration_submitted",
            json!({
                "integration_id": integration.id,
                "commit": integration.commit,
                "status": status,
                "conflict_ids": conflict_ids,
            }),
        ))
    }

    fn resolve_surfaces(&self, project_id: &str, names: &[String]) -> Result<BTreeSet<String>> {
        if names.len() > MAX_SURFACES_PER_INTEGRATION {
            return Err(Error::new(
                "invalid_params",
                format!("name at most {MAX_SURFACES_PER_INTEGRATION} surfaces"),
            ));
        }
        let mut resolved = BTreeSet::new();
        for name in names {
            match self.surfaces.get(name) {
                Some(surface) if surface.project_id == project_id => {
                    resolved.insert(name.clone());
                }
                _ => {
                    return Err(Error::new(
                        "unknown_surface",
                        format!("surface '{name}' is not registered for this project"),
                    ));
                }
            }
        }
        Ok(resolved)
    }

    /// Open integrations of other teams in the same project that the new one cannot land beside
    /// unexamined: shared paths, a surface both change, or a change to something the other relies on.
    fn detect_conflicts(&self, new: &Integration) -> Vec<(ConflictKind, Integration, Vec<String>)> {
        let mut found = Vec::new();
        for other in self.integrations.values().filter(|other| {
            other.project_id == new.project_id
                && other.team_id != new.team_id
                && other.status.is_open()
        }) {
            let paths: Vec<String> = new
                .changed_paths
                .iter()
                .filter(|path| {
                    other
                        .changed_paths
                        .iter()
                        .any(|theirs| paths_nest(path, theirs))
                })
                .cloned()
                .collect();
            if !paths.is_empty() {
                found.push((ConflictKind::PathOverlap, other.clone(), paths));
            }
            let shared: Vec<String> = new.touches.intersection(&other.touches).cloned().collect();
            if !shared.is_empty() {
                found.push((ConflictKind::SharedSurface, other.clone(), shared));
            }
            let impact: Vec<String> = new
                .touches
                .intersection(&other.depends_on)
                .chain(new.depends_on.intersection(&other.touches))
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            if !impact.is_empty() {
                found.push((ConflictKind::DependencyImpact, other.clone(), impact));
            }
        }
        found
    }

    pub(super) fn review_integration(
        &mut self,
        actor: &Actor,
        integration_id: String,
        accepted: bool,
        rationale: String,
        evidence: Vec<Evidence>,
        revision: u64,
    ) -> Result<Applied> {
        validate_text("rationale", &rationale, MAX_MESSAGE_BYTES, false)?;
        validate_evidence(&evidence, false)?;
        let integration = self
            .integrations
            .get(&integration_id)
            .ok_or_else(|| Error::new("not_found", "integration not found"))?;
        self.require_team_actor(actor, &integration.team_id, Role::Qa)?;
        if !integration.status.is_queued() {
            return Err(Error::new(
                "invalid_state",
                "integration is not awaiting review",
            ));
        }
        let integration = self
            .integrations
            .get_mut(&integration_id)
            .ok_or_else(|| Error::new("corrupt_state", "integration disappeared"))?;
        integration.review = Some(IntegrationReview {
            reviewer_id: actor.id.clone(),
            accepted,
            rationale: rationale.clone(),
            evidence,
        });
        if !accepted {
            integration.status = IntegrationStatus::Rejected { reason: rationale };
        }
        integration.revision = revision;
        self.refresh_all_integrations(revision);
        self.integration_applied(
            &integration_id,
            "integration_reviewed",
            json!({ "accepted": accepted }),
        )
    }

    pub(super) fn resolve_conflict(
        &mut self,
        actor: &Actor,
        conflict_id: String,
        rationale: String,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        validate_text("rationale", &rationale, MAX_MESSAGE_BYTES, false)?;
        let conflict = self
            .conflicts
            .get_mut(&conflict_id)
            .ok_or_else(|| Error::new("not_found", "conflict not found"))?;
        if conflict.status != ConflictStatus::Open {
            return Err(Error::new("invalid_state", "conflict is already resolved"));
        }
        conflict.status = ConflictStatus::Resolved {
            by: actor.id.clone(),
            rationale,
        };
        conflict.revision = revision;
        let (project_id, kind, ids) = (
            conflict.project_id.clone(),
            conflict.kind,
            conflict.integration_ids.clone(),
        );
        let result = serialize_value(&*conflict)?;
        self.refresh_all_integrations(revision);
        Ok(applied(
            result,
            None,
            Some(project_id),
            "conflict_resolved",
            json!({ "conflict_id": conflict_id, "kind": kind, "integration_ids": ids }),
        ))
    }

    pub(super) fn acknowledge_notice(
        &mut self,
        actor: &Actor,
        notice_id: String,
        revision: u64,
    ) -> Result<Applied> {
        let notice = self
            .notices
            .get(&notice_id)
            .ok_or_else(|| Error::new("not_found", "notice not found"))?;
        self.require_team_actor(actor, &notice.to_team_id, Role::Lead)?;
        let notice = self
            .notices
            .get_mut(&notice_id)
            .ok_or_else(|| Error::new("corrupt_state", "notice disappeared"))?;
        notice.status = NoticeStatus::Acknowledged {
            by: actor.id.clone(),
        };
        notice.revision = revision;
        let (team_id, project_id) = (notice.to_team_id.clone(), notice.project_id.clone());
        let result = serialize_value(&*notice)?;
        Ok(applied(
            result,
            Some(team_id),
            Some(project_id),
            "notice_acknowledged",
            json!({ "notice_id": notice_id }),
        ))
    }

    pub(super) fn cancel_integration(
        &mut self,
        actor: &Actor,
        integration_id: String,
        reason: String,
        revision: u64,
    ) -> Result<Applied> {
        validate_text("reason", &reason, MAX_MESSAGE_BYTES, false)?;
        let integration = self
            .integrations
            .get(&integration_id)
            .ok_or_else(|| Error::new("not_found", "integration not found"))?;
        self.require_team_member_or_owner(actor, &integration.team_id)?;
        if !integration.status.is_queued() {
            return Err(Error::new(
                "invalid_state",
                "only a queued integration can be cancelled; resolve an in-flight merge first",
            ));
        }
        let integration = self
            .integrations
            .get_mut(&integration_id)
            .ok_or_else(|| Error::new("corrupt_state", "integration disappeared"))?;
        integration.status = IntegrationStatus::Cancelled { reason };
        integration.revision = revision;
        self.refresh_all_integrations(revision);
        self.integration_applied(&integration_id, "integration_cancelled", json!({}))
    }

    /// Owner only, with an approval that names this exact integration and commit. No force option
    /// exists: the harness merges what was reviewed, or records that it did not.
    pub(super) fn begin_merge(
        &mut self,
        actor: &Actor,
        integration_id: String,
        approval_id: String,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        let integration = self
            .integrations
            .get(&integration_id)
            .ok_or_else(|| Error::new("not_found", "integration not found"))?
            .clone();
        if integration.status != IntegrationStatus::Ready {
            return Err(Error::new(
                "not_ready",
                "the integration needs an accepted review and no open conflict",
            ));
        }
        if self
            .worktrees
            .get(&integration.claim_id)
            .is_none_or(|claim| claim.status != ClaimStatus::Active)
        {
            return Err(Error::new(
                "invalid_state",
                "the worktree claim is no longer active",
            ));
        }
        let approval_team = self
            .approvals
            .get(&approval_id)
            .map(|approval| approval.team_id.clone())
            .ok_or_else(|| Error::new("not_found", "approval not found"))?;
        if approval_team != integration.team_id {
            return Err(Error::new(
                "approval_mismatch",
                "the approval belongs to another team",
            ));
        }
        self.spend_approval(
            actor,
            &approval_id,
            &format!("merge-{integration_id}"),
            &merge_action(&integration),
            revision,
        )?;
        let stored = self
            .integrations
            .get_mut(&integration_id)
            .ok_or_else(|| Error::new("corrupt_state", "integration disappeared"))?;
        stored.status = IntegrationStatus::Merging { approval_id };
        stored.revision = revision;
        self.integration_applied(&integration_id, "merge_started", json!({}))
    }

    pub(super) fn complete_merge(
        &mut self,
        actor: &Actor,
        integration_id: String,
        merge_commit: String,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        validate_commit("merge_commit", &merge_commit)?;
        let integration = self
            .integrations
            .get_mut(&integration_id)
            .ok_or_else(|| Error::new("not_found", "integration not found"))?;
        if !matches!(
            integration.status,
            IntegrationStatus::Merging { .. } | IntegrationStatus::MergeUnknown { .. }
        ) {
            return Err(Error::new("invalid_state", "no merge is in flight"));
        }
        integration.status = IntegrationStatus::Merged {
            merge_commit: merge_commit.clone(),
        };
        integration.revision = revision;
        self.refresh_all_integrations(revision);
        self.integration_applied(
            &integration_id,
            "merge_completed",
            json!({ "merge_commit": merge_commit }),
        )
    }

    pub(super) fn abort_merge(
        &mut self,
        actor: &Actor,
        integration_id: String,
        reason: String,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        validate_text("reason", &reason, MAX_MESSAGE_BYTES, false)?;
        let integration = self
            .integrations
            .get_mut(&integration_id)
            .ok_or_else(|| Error::new("not_found", "integration not found"))?;
        if !matches!(
            integration.status,
            IntegrationStatus::Merging { .. } | IntegrationStatus::MergeUnknown { .. }
        ) {
            return Err(Error::new("invalid_state", "no merge is in flight"));
        }
        // The approval is spent: a retry needs a fresh one. The review is kept, so it is Ready again.
        integration.status = IntegrationStatus::AwaitingReview;
        integration.revision = revision;
        self.refresh_all_integrations(revision);
        self.integration_applied(
            &integration_id,
            "merge_aborted",
            json!({ "reason": reason }),
        )
    }

    /// After a crash, a merge may or may not have happened. Say so instead of guessing.
    pub(super) fn mark_in_flight_merges_unknown(&mut self, revision: u64) -> Vec<String> {
        let mut affected = Vec::new();
        for integration in self.integrations.values_mut() {
            if let IntegrationStatus::Merging { approval_id } = &integration.status {
                integration.status = IntegrationStatus::MergeUnknown {
                    approval_id: approval_id.clone(),
                };
                integration.revision = revision;
                affected.push(integration.id.clone());
            }
        }
        affected
    }

    /// Readiness follows the review and the conflicts, so it is recomputed, never hand-edited.
    fn refresh_all_integrations(&mut self, revision: u64) {
        let blocked: BTreeSet<String> = self
            .conflicts
            .values()
            .filter(|conflict| conflict.status == ConflictStatus::Open)
            .flat_map(|conflict| conflict.integration_ids.iter().cloned())
            .collect();
        for integration in self.integrations.values_mut() {
            if !integration.status.is_queued() {
                continue;
            }
            let next = if blocked.contains(&integration.id) {
                IntegrationStatus::Blocked
            } else if integration
                .review
                .as_ref()
                .is_some_and(|review| review.accepted)
            {
                IntegrationStatus::Ready
            } else {
                IntegrationStatus::AwaitingReview
            };
            if integration.status != next {
                integration.status = next;
                integration.revision = revision;
            }
        }
    }

    fn integration_applied(&self, id: &str, kind: &'static str, extra: Value) -> Result<Applied> {
        let integration = self
            .integrations
            .get(id)
            .ok_or_else(|| Error::new("corrupt_state", "integration disappeared"))?;
        let mut payload = json!({
            "integration_id": integration.id,
            "team_id": integration.team_id,
            "status": integration.status,
        });
        if let (Some(payload), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
            payload.extend(extra.clone());
        }
        Ok(applied(
            serialize_value(integration)?,
            Some(integration.team_id.clone()),
            Some(integration.project_id.clone()),
            kind,
            payload,
        ))
    }

    pub(super) fn validate_portfolio(&self) -> Result<()> {
        let corrupt = |message: &str| Error::new("corrupt_state", message.to_owned());
        for surface in self.surfaces.values() {
            if !self.projects.contains_key(&surface.project_id) {
                return Err(corrupt("surface names an unknown project"));
            }
        }
        let mut active: Vec<&WorktreeClaim> = Vec::new();
        for claim in self.worktrees.values() {
            match self.teams.get(&claim.team_id) {
                Some(team) if team.project_id == claim.project_id => {}
                _ => return Err(corrupt("worktree claim names an unknown team")),
            }
            if claim.status == ClaimStatus::Active {
                if active.iter().any(|other| {
                    other.project_id == claim.project_id
                        && (paths_nest(&other.path, &claim.path) || other.branch == claim.branch)
                }) {
                    return Err(corrupt("active worktree claims overlap"));
                }
                active.push(claim);
            }
        }
        for integration in self.integrations.values() {
            if !self.worktrees.contains_key(&integration.claim_id)
                || self
                    .teams
                    .get(&integration.team_id)
                    .is_none_or(|team| team.project_id != integration.project_id)
            {
                return Err(corrupt("integration names an unknown team or worktree"));
            }
            if integration
                .touches
                .iter()
                .chain(&integration.depends_on)
                .any(|surface| !self.surfaces.contains_key(surface))
            {
                return Err(corrupt("integration names an unknown surface"));
            }
        }
        for conflict in self.conflicts.values() {
            if conflict
                .integration_ids
                .iter()
                .any(|id| !self.integrations.contains_key(id))
            {
                return Err(corrupt("conflict names an unknown integration"));
            }
        }
        for notice in self.notices.values() {
            if !self.integrations.contains_key(&notice.integration_id)
                || !self.teams.contains_key(&notice.to_team_id)
            {
                return Err(corrupt("notice names an unknown integration or team"));
            }
        }
        Ok(())
    }

    /// One line per team the actor may see, for pickers and dashboards.
    pub fn list_teams(&self, actor: &Actor) -> Result<Value> {
        let snapshot = self.snapshot(actor, None)?;
        let teams: Vec<Value> = self
            .teams
            .values()
            .filter(|team| {
                snapshot["teams"]
                    .as_array()
                    .is_some_and(|visible| visible.iter().any(|item| item["id"] == team.id.as_str()))
            })
            .map(|team| {
                json!({
                    "id": team.id,
                    "name": team.name,
                    "project_id": team.project_id,
                    "status": team.status,
                    "open_tasks": self.tasks.values().filter(|task| task.team_id == team.id && !task.status.is_terminal()).count(),
                    "open_integrations": self.integrations.values().filter(|item| item.team_id == team.id && item.status.is_open()).count(),
                })
            })
            .collect();
        Ok(json!({ "teams": teams, "revision": self.revision, "stream_id": self.stream_id }))
    }
}

fn merge_action(integration: &Integration) -> ProtectedAction {
    ProtectedAction {
        kind: ProtectedActionKind::ProtectedBranchIntegration,
        target: integration.target_branch.clone(),
        parameters: json!({
            "integration_id": integration.id,
            "commit": integration.commit,
            "source_branch": integration.source_branch,
        }),
    }
}

/// The action an Owner approval must name before an integration may start merging.
#[cfg(test)]
pub fn merge_action_for(integration: &Integration) -> ProtectedAction {
    merge_action(integration)
}

fn conflict_id(a: &str, b: &str, kind: ConflictKind) -> String {
    let (first, second) = if a < b { (a, b) } else { (b, a) };
    format!("conflict-{first}-{second}-{kind:?}").to_lowercase()
}

/// `path` is `prefix` itself or inside it.
fn path_within(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// One path contains the other (or they are equal).
fn paths_nest(a: &str, b: &str) -> bool {
    path_within(a, b) || path_within(b, a)
}

fn validate_segments(value: &str, what: &str) -> Result<()> {
    let bad = value.len() > MAX_PATH_BYTES
        || value.contains('\\')
        || value.chars().any(char::is_control)
        || value
            .split('/')
            .skip(usize::from(value.starts_with('/')))
            .any(|segment| segment.is_empty() || segment == "." || segment == "..");
    if bad {
        return Err(Error::new(
            "invalid_params",
            format!("{what} must be a normalized path of at most {MAX_PATH_BYTES} bytes"),
        ));
    }
    Ok(())
}

/// A path inside a worktree: relative, no `..`, and not inside `.git`.
pub(super) fn normalize_repo_path(path: &str) -> Result<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed.starts_with('/') {
        return Err(Error::new(
            "invalid_params",
            "repository paths must be relative",
        ));
    }
    validate_segments(trimmed, "repository path")?;
    if trimmed == ".git" || trimmed.starts_with(".git/") {
        return Err(Error::new(
            "invalid_params",
            "repository paths cannot point into .git",
        ));
    }
    Ok(trimmed.to_owned())
}

fn normalize_worktree_path(path: &str) -> Result<String> {
    let trimmed = path.trim_end_matches('/');
    if !trimmed.starts_with('/') || trimmed.len() < 2 {
        return Err(Error::new(
            "invalid_params",
            "worktree paths must be absolute",
        ));
    }
    validate_segments(trimmed, "worktree path")?;
    Ok(trimmed.to_owned())
}

fn validate_branch(branch: &str) -> Result<()> {
    let valid = !branch.is_empty()
        && branch.len() <= 200
        && !branch.starts_with(['-', '/'])
        && !branch.ends_with(['/', '.'])
        && !branch.contains("..")
        && !branch.contains("//")
        && branch
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'));
    if !valid {
        return Err(Error::new(
            "invalid_params",
            "branch is not a valid branch name",
        ));
    }
    Ok(())
}

fn validate_commit(field: &str, commit: &str) -> Result<()> {
    if commit.len() != 40
        || !commit
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(Error::new(
            "invalid_params",
            format!("{field} must be a 40-character lowercase git commit id"),
        ));
    }
    Ok(())
}

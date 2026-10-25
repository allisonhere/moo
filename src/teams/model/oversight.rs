//! Keeping a team honest and bounded: disagreements that cannot loop forever, usage that says how
//! much of it is measured, hard limits on work in flight, controls for agents, and one summary across
//! every project that shows what needs a person.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::planning::Metering;
use super::{
    applied, require_role, serialize_value, validate_evidence, validate_id, validate_text, Actor,
    Applied, DispatchState, Evidence, Role, State, TaskStatus, MAX_MESSAGE_BYTES, MAX_TITLE_BYTES,
};
use crate::teams::{Error, Result};

/// Work in flight, per team. Hard caps, not advice.
pub const MAX_ACTIVE_TASKS: usize = 3;
pub const MAX_OPEN_TASKS: usize = 40;
pub const MAX_QUEUED_DISPATCHES: usize = 20;
pub const MAX_OPEN_DISPUTES: usize = 6;
/// A dispute has its raiser and at most two peers; then it is decided or escalated, never extended.
pub const MAX_POSITIONS: usize = 3;
pub const DEFAULT_STUCK_SECONDS: u64 = 30 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub actor_id: String,
    pub text: String,
    pub evidence: Vec<Evidence>,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DisputeStatus {
    /// Waiting for peers (at most two) to give their view.
    Open,
    /// Needs the Owner: an architectural question, or peer review ran its course.
    Escalated {
        reason: String,
    },
    Resolved {
        by: String,
        resolution: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dispute {
    pub id: String,
    pub team_id: String,
    pub task_id: Option<String>,
    pub topic: String,
    pub architectural: bool,
    /// The conflicting evidence that started it (at least two distinct references).
    pub evidence: Vec<Evidence>,
    pub positions: Vec<Position>,
    pub status: DisputeStatus,
    pub revision: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub metered_cost_micros: u64,
    pub estimated_cost_micros: u64,
    pub metered_reports: u64,
    pub estimated_reports: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamUsage {
    pub by_provider: BTreeMap<String, ProviderUsage>,
    pub last_revision: u64,
}

impl TeamUsage {
    fn total(&self, f: impl Fn(&ProviderUsage) -> u64) -> u64 {
        self.by_provider.values().map(f).sum()
    }
    pub fn metered_cost(&self) -> u64 {
        self.total(|p| p.metered_cost_micros)
    }
    pub fn estimated_cost(&self) -> u64 {
        self.total(|p| p.estimated_cost_micros)
    }
    pub fn metered_tokens(&self) -> u64 {
        // Tokens count against a limit only when they came from reliable metering.
        self.by_provider
            .values()
            .filter(|p| p.metered_reports > 0)
            .map(|p| p.input_tokens + p.output_tokens)
            .sum()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamLimits {
    pub max_metered_cost_micros: Option<u64>,
    pub max_metered_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageBasis {
    Metered,
    Estimated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunState {
    /// Working normally.
    Active,
    /// Paused: no new work is sent to it; its credential still works.
    Idle,
    /// Gone: its credential is revoked until the Owner reactivates it.
    Released,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentState {
    pub actor_id: String,
    pub state: AgentRunState,
    pub reason: String,
    pub revision: u64,
}

impl State {
    // ---- disputes ---------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)] // A dispute starts with its topic, evidence and first position.
    pub(super) fn raise_dispute(
        &mut self,
        actor: &Actor,
        dispute_id: String,
        team_id: String,
        task_id: Option<String>,
        topic: String,
        architectural: bool,
        position: String,
        evidence: Vec<Evidence>,
        revision: u64,
    ) -> Result<Applied> {
        let team = self.require_team_actor_any(actor, &team_id)?;
        validate_id("dispute_id", &dispute_id)?;
        validate_text("topic", &topic, MAX_TITLE_BYTES, false)?;
        validate_text("position", &position, MAX_MESSAGE_BYTES, false)?;
        validate_evidence(&evidence, true)?;
        let distinct: std::collections::BTreeSet<&str> =
            evidence.iter().map(|e| e.reference.as_str()).collect();
        if distinct.len() < 2 {
            return Err(Error::new(
                "invalid_params",
                "a dispute needs two distinct pieces of conflicting evidence",
            ));
        }
        if self.disputes.contains_key(&dispute_id) {
            return Err(Error::new("already_exists", "dispute already exists"));
        }
        if let Some(task) = task_id.as_deref() {
            if self.tasks.get(task).is_none_or(|t| t.team_id != team_id) {
                return Err(Error::new(
                    "invalid_reference",
                    "the task is not one of this team's",
                ));
            }
        }
        let open = self
            .disputes
            .values()
            .filter(|d| d.team_id == team_id && !matches!(d.status, DisputeStatus::Resolved { .. }))
            .count();
        if open >= MAX_OPEN_DISPUTES {
            return Err(Error::new(
                "capacity_exceeded",
                "the team has too many unresolved disputes; settle some first",
            ));
        }
        // A settled question stays settled unless there is something new to say about it.
        let settled_evidence: std::collections::BTreeSet<&str> = self
            .disputes
            .values()
            .filter(|d| {
                d.team_id == team_id
                    && d.task_id == task_id
                    && d.topic == topic
                    && matches!(d.status, DisputeStatus::Resolved { .. })
            })
            .flat_map(|d| d.evidence.iter().map(|e| e.reference.as_str()))
            .collect();
        let settled_exists = self.disputes.values().any(|d| {
            d.team_id == team_id
                && d.task_id == task_id
                && d.topic == topic
                && matches!(d.status, DisputeStatus::Resolved { .. })
        });
        if settled_exists
            && distinct
                .iter()
                .all(|reference| settled_evidence.contains(reference))
        {
            return Err(Error::new(
                "dispute_settled",
                "that question was already decided and this adds no new evidence",
            ));
        }
        let status = if architectural {
            DisputeStatus::Escalated {
                reason: "an architectural question goes to the Owner".into(),
            }
        } else {
            DisputeStatus::Open
        };
        let dispute = Dispute {
            id: dispute_id.clone(),
            team_id: team_id.clone(),
            task_id,
            topic,
            architectural,
            evidence,
            positions: vec![Position {
                actor_id: actor.id.clone(),
                text: position,
                evidence: Vec::new(),
                revision,
            }],
            status,
            revision,
        };
        self.disputes.insert(dispute_id.clone(), dispute.clone());
        Ok(applied(
            serialize_value(&dispute)?,
            Some(team_id),
            Some(team.project_id),
            "dispute_raised",
            json!({ "dispute_id": dispute_id, "architectural": architectural, "status": dispute.status }),
        ))
    }

    fn require_team_actor_any(&self, actor: &Actor, team_id: &str) -> Result<super::Team> {
        let team = self.require_team_member_or_owner(actor, team_id)?.clone();
        Ok(team)
    }

    pub(super) fn respond_dispute(
        &mut self,
        actor: &Actor,
        dispute_id: String,
        position: String,
        evidence: Vec<Evidence>,
        revision: u64,
    ) -> Result<Applied> {
        validate_text("position", &position, MAX_MESSAGE_BYTES, false)?;
        validate_evidence(&evidence, false)?;
        let dispute = self
            .disputes
            .get(&dispute_id)
            .ok_or_else(|| Error::new("not_found", "dispute not found"))?;
        self.require_team_member_or_owner(actor, &dispute.team_id)?;
        if dispute.status != DisputeStatus::Open {
            return Err(Error::new(
                "invalid_state",
                "this dispute is no longer taking peer responses",
            ));
        }
        if dispute.positions.iter().any(|p| p.actor_id == actor.id) {
            return Err(Error::new(
                "already_exists",
                "each person gives one position per dispute",
            ));
        }
        let dispute = self
            .disputes
            .get_mut(&dispute_id)
            .ok_or_else(|| Error::new("corrupt_state", "dispute disappeared"))?;
        dispute.positions.push(Position {
            actor_id: actor.id.clone(),
            text: position,
            evidence,
            revision,
        });
        dispute.revision = revision;
        // The raiser and two peers is the whole review. Past that it is decided, not debated.
        if dispute.positions.len() >= MAX_POSITIONS {
            dispute.status = DisputeStatus::Escalated {
                reason: "peer review is finished; the Lead or Owner decides".into(),
            };
        }
        let (team_id, result, status) = (
            dispute.team_id.clone(),
            serialize_value(&*dispute)?,
            dispute.status.clone(),
        );
        let project_id = self.require_team(&team_id)?.project_id.clone();
        Ok(applied(
            result,
            Some(team_id),
            Some(project_id),
            "dispute_responded",
            json!({ "dispute_id": dispute_id, "status": status }),
        ))
    }

    /// The Lead may settle a non-architectural dispute once a peer has weighed in.
    pub(super) fn resolve_dispute(
        &mut self,
        actor: &Actor,
        dispute_id: String,
        resolution: String,
        revision: u64,
    ) -> Result<Applied> {
        validate_text("resolution", &resolution, MAX_MESSAGE_BYTES, false)?;
        let dispute = self
            .disputes
            .get(&dispute_id)
            .ok_or_else(|| Error::new("not_found", "dispute not found"))?;
        self.require_team_actor(actor, &dispute.team_id, Role::Lead)?;
        if matches!(dispute.status, DisputeStatus::Resolved { .. }) {
            return Err(Error::new(
                "invalid_state",
                "the dispute is already resolved",
            ));
        }
        if dispute.architectural {
            return Err(Error::new(
                "forbidden",
                "an architectural dispute is decided by the Owner",
            ));
        }
        if dispute.positions.len() < 2 {
            return Err(Error::new(
                "invalid_state",
                "ask a peer for their view before deciding",
            ));
        }
        self.finish_dispute(
            &dispute_id,
            &actor.id,
            resolution,
            revision,
            "dispute_resolved",
        )
    }

    pub(super) fn decide_dispute(
        &mut self,
        actor: &Actor,
        dispute_id: String,
        decision: String,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        validate_text("decision", &decision, MAX_MESSAGE_BYTES, false)?;
        let dispute = self
            .disputes
            .get(&dispute_id)
            .ok_or_else(|| Error::new("not_found", "dispute not found"))?;
        if matches!(dispute.status, DisputeStatus::Resolved { .. }) {
            return Err(Error::new(
                "invalid_state",
                "the dispute is already resolved",
            ));
        }
        self.finish_dispute(
            &dispute_id,
            &actor.id,
            decision,
            revision,
            "dispute_decided",
        )
    }

    fn finish_dispute(
        &mut self,
        id: &str,
        by: &str,
        resolution: String,
        revision: u64,
        kind: &'static str,
    ) -> Result<Applied> {
        let dispute = self
            .disputes
            .get_mut(id)
            .ok_or_else(|| Error::new("corrupt_state", "dispute disappeared"))?;
        dispute.status = DisputeStatus::Resolved {
            by: by.to_owned(),
            resolution,
        };
        dispute.revision = revision;
        let (team_id, result) = (dispute.team_id.clone(), serialize_value(&*dispute)?);
        let project_id = self.require_team(&team_id)?.project_id.clone();
        Ok(applied(
            result,
            Some(team_id),
            Some(project_id),
            kind,
            json!({ "dispute_id": id }),
        ))
    }

    // ---- usage and limits -------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)] // A usage report is what the provider said about one stretch of work.
    pub(super) fn report_usage(
        &mut self,
        actor: &Actor,
        team_id: String,
        provider_id: String,
        input_tokens: u64,
        output_tokens: u64,
        cost_micros: Option<u64>,
        basis: UsageBasis,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        let team = self.require_team(&team_id)?.clone();
        let provider = self
            .providers
            .get(&provider_id)
            .ok_or_else(|| Error::new("not_found", "that provider is not declared"))?;
        if basis == UsageBasis::Metered && provider.metering != Metering::Metered {
            return Err(Error::new(
                "invalid_state",
                format!("'{provider_id}' is not declared as metered, so its usage can only be reported as an estimate"),
            ));
        }
        if basis == UsageBasis::Metered && cost_micros.is_none() {
            return Err(Error::new(
                "invalid_params",
                "metered usage needs the cost the provider reported",
            ));
        }
        const LIMIT: u64 = 1_000_000_000_000;
        if input_tokens > LIMIT || output_tokens > LIMIT || cost_micros.is_some_and(|c| c > LIMIT) {
            return Err(Error::new(
                "invalid_params",
                "a usage report is out of range",
            ));
        }
        let usage = self.usage.entry(team_id.clone()).or_default();
        let by = usage.by_provider.entry(provider_id.clone()).or_default();
        by.input_tokens = by.input_tokens.saturating_add(input_tokens);
        by.output_tokens = by.output_tokens.saturating_add(output_tokens);
        match basis {
            UsageBasis::Metered => {
                by.metered_cost_micros = by
                    .metered_cost_micros
                    .saturating_add(cost_micros.unwrap_or(0));
                by.metered_reports += 1;
            }
            UsageBasis::Estimated => {
                by.estimated_cost_micros = by
                    .estimated_cost_micros
                    .saturating_add(cost_micros.unwrap_or(0));
                by.estimated_reports += 1;
            }
        }
        usage.last_revision = revision;
        let result = self.team_usage_view(&team_id);
        Ok(applied(
            result,
            Some(team_id),
            Some(team.project_id),
            "usage_reported",
            json!({ "provider_id": provider_id, "basis": basis, "input_tokens": input_tokens, "output_tokens": output_tokens }),
        ))
    }

    pub(super) fn set_team_limits(
        &mut self,
        actor: &Actor,
        team_id: String,
        max_metered_cost_micros: Option<u64>,
        max_metered_tokens: Option<u64>,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        let team = self.require_team(&team_id)?.clone();
        let _ = revision;
        self.team_limits.insert(
            team_id.clone(),
            TeamLimits {
                max_metered_cost_micros,
                max_metered_tokens,
            },
        );
        Ok(applied(
            json!({ "team_id": team_id, "max_metered_cost_micros": max_metered_cost_micros, "max_metered_tokens": max_metered_tokens }),
            Some(team_id.clone()),
            Some(team.project_id),
            "team_limits_set",
            json!({ "team_id": team_id, "max_metered_cost_micros": max_metered_cost_micros, "max_metered_tokens": max_metered_tokens }),
        ))
    }

    /// Stops new work once reliably metered usage reaches a limit. Estimates never enforce anything,
    /// and spend already under way is not stopped: that depends on the provider (see the warnings).
    pub(super) fn ensure_within_usage_limits(&self, team_id: &str) -> Result<()> {
        let (Some(limits), Some(usage)) = (self.team_limits.get(team_id), self.usage.get(team_id))
        else {
            return Ok(());
        };
        if limits
            .max_metered_cost_micros
            .is_some_and(|max| usage.metered_cost() >= max)
            || limits
                .max_metered_tokens
                .is_some_and(|max| usage.metered_tokens() >= max)
        {
            return Err(Error::new(
                "usage_limit_reached",
                "the team reached its metered usage limit; the Owner can raise it",
            ));
        }
        Ok(())
    }

    /// What the numbers do and do not mean for this team, in plain words.
    pub fn usage_warnings(&self, team_id: &str) -> Vec<String> {
        let mut providers: std::collections::BTreeSet<&str> = self
            .role_assignments
            .values()
            .filter(|a| a.team_id == team_id)
            .map(|a| a.provider_id.as_str())
            .collect();
        if let Some(usage) = self.usage.get(team_id) {
            providers.extend(usage.by_provider.keys().map(String::as_str));
        }
        let mut warnings = Vec::new();
        for id in providers {
            let Some(provider) = self.providers.get(id) else {
                continue;
            };
            match provider.metering {
                Metering::Metered => {}
                Metering::Estimated => {
                    warnings.push(format!("{id}: costs are estimates, not billing figures"))
                }
                Metering::None => warnings.push(format!(
                    "{id}: usage is not measured at all, so spend cannot be seen here"
                )),
            }
            if !provider.hard_spend_limit {
                warnings.push(format!("{id}: this integration cannot cap billing; limits only stop new work from starting"));
            }
        }
        if !warnings.is_empty() && !self.team_limits.contains_key(team_id) {
            warnings.push("no usage limit is set for this team".into());
        }
        warnings
    }

    pub fn team_usage_view(&self, team_id: &str) -> Value {
        let usage = self.usage.get(team_id).cloned().unwrap_or_default();
        json!({
            "team_id": team_id,
            "reliable": {
                "metered_cost_micros": usage.metered_cost(),
                "metered_tokens": usage.metered_tokens(),
                "note": "from providers declared as metered; only these can enforce a limit",
            },
            "estimated": {
                "cost_micros": usage.estimated_cost(),
                "note": "approximations made by the harness; never enforced, never to be read as billing",
            },
            "by_provider": usage.by_provider,
            "limits": self.team_limits.get(team_id),
            "warnings": self.usage_warnings(team_id),
        })
    }

    // ---- agent controls ---------------------------------------------------------------------

    pub(super) fn set_agent_state(
        &mut self,
        actor: &Actor,
        actor_id: String,
        state: AgentRunState,
        reason: String,
        revision: u64,
    ) -> Result<Applied> {
        validate_text("reason", &reason, MAX_MESSAGE_BYTES, false)?;
        let agent = self
            .actors
            .get(&actor_id)
            .ok_or_else(|| Error::new("not_found", "agent not found"))?
            .clone();
        let team_id = match (&agent.team_id, agent.role) {
            (Some(team), Role::Lead | Role::Developer | Role::Qa) => team.clone(),
            _ => {
                return Err(Error::new(
                    "invalid_reference",
                    "only a team's Lead, Developer or QA has a run state",
                ))
            }
        };
        if actor.role != Role::Owner && actor.team_id.as_deref() != Some(team_id.as_str()) {
            return Err(Error::new(
                "forbidden",
                "a Lead controls only their own team's agents",
            ));
        }
        let current = self
            .agent_states
            .get(&actor_id)
            .map_or(AgentRunState::Active, |s| s.state);
        if current == AgentRunState::Released && actor.role != Role::Owner {
            return Err(Error::new(
                "forbidden",
                "only the Owner brings a released agent back",
            ));
        }
        if current == state {
            return Err(Error::new(
                "invalid_state",
                "the agent is already in that state",
            ));
        }
        if actor.id == actor_id && state != AgentRunState::Active {
            return Err(Error::new(
                "forbidden",
                "an agent cannot idle or release itself",
            ));
        }
        if let Some(credential) = self.credentials.get_mut(&agent.auth_digest_clone()) {
            credential.active = state != AgentRunState::Released;
        }
        if state == AgentRunState::Released {
            self.pane_bindings.remove(&actor_id);
        }
        self.agent_states.insert(
            actor_id.clone(),
            AgentState {
                actor_id: actor_id.clone(),
                state,
                reason,
                revision,
            },
        );
        let project_id = self.require_team(&team_id)?.project_id.clone();
        Ok(applied(
            json!({ "actor_id": actor_id, "state": state }),
            Some(team_id),
            Some(project_id),
            "agent_state_set",
            json!({ "actor_id": actor_id, "state": state }),
        ))
    }

    pub fn agent_run_state(&self, actor_id: &str) -> AgentRunState {
        self.agent_states
            .get(actor_id)
            .map_or(AgentRunState::Active, |s| s.state)
    }

    /// New work is not sent to an agent that is idle or released.
    pub(super) fn ensure_agent_available(&self, actor_id: &str) -> Result<()> {
        match self.agent_run_state(actor_id) {
            AgentRunState::Active => Ok(()),
            AgentRunState::Idle => Err(Error::new(
                "agent_idle",
                format!("{actor_id} is idle; resume it to give it work"),
            )),
            AgentRunState::Released => {
                Err(Error::new("agent_idle", format!("{actor_id} was released")))
            }
        }
    }

    // ---- hard limits on work in flight ------------------------------------------------------

    pub(super) fn ensure_room_for_task(&self, team_id: &str) -> Result<()> {
        let open = self
            .tasks
            .values()
            .filter(|t| t.team_id == team_id && !t.status.is_terminal())
            .count();
        if open >= MAX_OPEN_TASKS {
            return Err(Error::new(
                "capacity_exceeded",
                format!("a team has at most {MAX_OPEN_TASKS} open tasks"),
            ));
        }
        let queued = self
            .dispatches
            .values()
            .filter(|d| {
                d.team_id == team_id
                    && matches!(
                        d.state,
                        DispatchState::Queued
                            | DispatchState::Attempted
                            | DispatchState::Submitted { .. }
                    )
            })
            .count();
        if queued >= MAX_QUEUED_DISPATCHES {
            return Err(Error::new(
                "capacity_exceeded",
                format!("a team has at most {MAX_QUEUED_DISPATCHES} queued dispatches"),
            ));
        }
        self.ensure_within_usage_limits(team_id)
    }

    pub(super) fn ensure_room_to_start(&self, team_id: &str) -> Result<()> {
        let active = self
            .tasks
            .values()
            .filter(|t| {
                t.team_id == team_id
                    && matches!(
                        t.status,
                        TaskStatus::InProgress { .. } | TaskStatus::ReworkRequested { .. }
                    )
            })
            .count();
        if active >= MAX_ACTIVE_TASKS {
            return Err(Error::new(
                "concurrency_limit",
                format!(
                    "a team works on at most {MAX_ACTIVE_TASKS} tasks at once; finish one first"
                ),
            ));
        }
        Ok(())
    }

    /// Longest chain of tasks that wait for each other, counting the task itself.
    pub(super) fn dependency_depth(
        &self,
        task_id: &str,
        extra: &std::collections::BTreeSet<String>,
    ) -> usize {
        fn go(
            state: &State,
            id: &str,
            extra: &std::collections::BTreeSet<String>,
            root: &str,
            seen: &mut Vec<String>,
        ) -> usize {
            if seen.iter().any(|s| s == id) {
                return 0;
            }
            seen.push(id.to_owned());
            let deps: Vec<String> = if id == root {
                extra.iter().cloned().collect()
            } else {
                state
                    .tasks
                    .get(id)
                    .map(|t| t.dependencies.iter().cloned().collect())
                    .unwrap_or_default()
            };
            let depth = 1 + deps
                .iter()
                .map(|d| go(state, d, extra, root, seen))
                .max()
                .unwrap_or(0);
            seen.pop();
            depth
        }
        go(self, task_id, extra, task_id, &mut Vec::new())
    }

    pub(super) fn ensure_depth_within_limit(
        &self,
        task_id: &str,
        dependencies: &std::collections::BTreeSet<String>,
    ) -> Result<()> {
        let depth = self.dependency_depth(task_id, dependencies);
        if depth > super::planning::MAX_DEPENDENCY_DEPTH {
            return Err(Error::new(
                "task_depth_limit",
                format!(
                    "dependency chains are at most {} tasks deep",
                    super::planning::MAX_DEPENDENCY_DEPTH
                ),
            ));
        }
        Ok(())
    }

    // ---- the cross-project summary ----------------------------------------------------------

    /// One view across every project the caller can see: what is waiting for a person, what looks
    /// stuck, and what the numbers mean. Stuck is "no change for `stuck_after` seconds", nothing more.
    pub fn summary(&self, actor: &Actor, now: u64, stuck_after: u64) -> Result<Value> {
        let snapshot = self.snapshot(actor, None)?;
        let visible: std::collections::BTreeSet<String> = snapshot["teams"]
            .as_array()
            .map(|teams| {
                teams
                    .iter()
                    .filter_map(|t| t["id"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let mut totals = (0u64, 0u64, 0u64, 0u64);
        let mut projects = Vec::new();
        for project in self.projects.values() {
            let mut teams = Vec::new();
            for team in self
                .teams
                .values()
                .filter(|t| t.project_id == project.id && visible.contains(&t.id))
            {
                let open_tasks = self
                    .tasks
                    .values()
                    .filter(|t| t.team_id == team.id && !t.status.is_terminal())
                    .count();
                let in_progress = self
                    .tasks
                    .values()
                    .filter(|t| {
                        t.team_id == team.id
                            && matches!(
                                t.status,
                                TaskStatus::InProgress { .. } | TaskStatus::ReworkRequested { .. }
                            )
                    })
                    .count();
                let pending_approvals = self
                    .approvals
                    .values()
                    .filter(|a| {
                        a.team_id == team.id
                            && matches!(a.status, super::governance::ApprovalStatus::Pending)
                    })
                    .count();
                let escalated: Vec<&str> = self
                    .disputes
                    .values()
                    .filter(|d| {
                        d.team_id == team.id && matches!(d.status, DisputeStatus::Escalated { .. })
                    })
                    .map(|d| d.id.as_str())
                    .collect();
                let mut stuck = Vec::new();
                if team.status == super::TeamStatus::Active {
                    for task in self
                        .tasks
                        .values()
                        .filter(|t| t.team_id == team.id && !t.status.is_terminal())
                    {
                        let idle = now.saturating_sub(task.updated_at);
                        if task.updated_at > 0 && idle > stuck_after {
                            stuck.push(json!({
                                "task_id": task.id, "title": task.title, "owner": task.developer_id,
                                "status": tag_json(&task.status), "idle_seconds": idle,
                                "suggested_actions": self.recovery_suggestions(task),
                            }));
                        }
                    }
                }
                let agents: Vec<Value> = [&team.lead_id, &team.developer_id, &team.qa_id]
                    .into_iter()
                    .map(|id| json!({"id": id, "state": self.agent_run_state(id)}))
                    .collect();
                totals.0 += pending_approvals as u64;
                totals.1 += stuck.len() as u64;
                totals.2 += escalated.len() as u64;
                totals.3 += 1;
                teams.push(json!({
                    "id": team.id, "name": team.name, "status": team.status,
                    "open_tasks": open_tasks, "in_progress": in_progress,
                    "pending_approvals": pending_approvals, "escalated_disputes": escalated,
                    "open_conflicts": self.conflicts.values().filter(|c| c.team_ids.contains(&team.id) && c.status == super::portfolio::ConflictStatus::Open).count(),
                    "stuck": stuck,
                    "agents": agents,
                    "usage": self.team_usage_view(&team.id),
                }));
            }
            if !teams.is_empty() {
                projects.push(json!({"id": project.id, "name": project.name, "teams": teams}));
            }
        }
        Ok(json!({
            "type": "teams_summary",
            "generated_at": now,
            "stuck_after_seconds": stuck_after,
            "revision": self.revision,
            "stream_id": self.stream_id,
            "totals": {"teams": totals.3, "pending_approvals": totals.0, "stuck_tasks": totals.1, "escalated_disputes": totals.2},
            "projects": projects,
        }))
    }

    /// What a person could do about a task that has stopped moving. Advice, never an action taken.
    fn recovery_suggestions(&self, task: &super::Task) -> Vec<Value> {
        let mut out = Vec::new();
        let last_failed = self
            .dispatches
            .values()
            .filter(|d| d.task_id == task.id)
            .max_by_key(|d| d.revision)
            .filter(|d| {
                matches!(
                    d.state,
                    DispatchState::Failed { .. }
                        | DispatchState::Rejected { .. }
                        | DispatchState::Unknown { .. }
                        | DispatchState::Cancelled
                )
            });
        if let Some(attempt) = last_failed {
            out.push(
                json!({"command": "retry_dispatch", "previous_attempt_id": attempt.id,
                "why": "the last dispatch did not complete"}),
            );
        }
        let (agent, why) = match task.status {
            TaskStatus::AwaitingQa { .. } => (
                self.teams.get(&task.team_id).map(|t| t.qa_id.clone()),
                "QA has not reviewed the handoff",
            ),
            _ => (
                Some(task.developer_id.clone()),
                "the Developer has not moved the task",
            ),
        };
        if let Some(agent) = agent {
            if self.agent_run_state(&agent) == AgentRunState::Active {
                out.push(
                    json!({"command": "set_agent_state", "actor_id": agent, "state": "idle",
                    "why": format!("{why}; idle it, restart its process, then resume it")}),
                );
            }
        }
        out.push(json!({"command": "cancel_task", "task_id": task.id, "why": "if the work is no longer wanted"}));
        out
    }

    pub(super) fn validate_oversight(&self) -> Result<()> {
        let corrupt = |m: &str| Error::new("corrupt_state", m.to_owned());
        for dispute in self.disputes.values() {
            if !self.teams.contains_key(&dispute.team_id)
                || dispute.positions.len() > MAX_POSITIONS
                || dispute.positions.is_empty()
            {
                return Err(corrupt("a dispute is malformed or exceeds its bounds"));
            }
        }
        for team_id in self.usage.keys().chain(self.team_limits.keys()) {
            if !self.teams.contains_key(team_id) {
                return Err(corrupt("usage or limits name an unknown team"));
            }
        }
        for state in self.agent_states.values() {
            if !self.actors.contains_key(&state.actor_id) {
                return Err(corrupt("an agent state names an unknown agent"));
            }
        }
        Ok(())
    }
}

fn tag_json(status: &TaskStatus) -> Value {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.get("status").cloned())
        .unwrap_or(Value::Null)
}

impl Actor {
    pub(super) fn auth_digest_clone(&self) -> String {
        self.auth_digest.clone()
    }
}

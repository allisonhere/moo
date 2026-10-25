//! Which terminal pane runs which agent, and with which provider CLI.
//!
//! The server cannot see this by itself: it never launches agents. Whoever starts an agent in a
//! Moo pane records the link here, and clients use it to focus the pane. It is a record, not a
//! capability: binding grants the agent nothing.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    applied, serialize_value, validate_id, validate_text, Actor, Applied, Role, State,
    MAX_NAME_BYTES,
};
use crate::teams::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneBinding {
    pub actor_id: String,
    pub pane_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub revision: u64,
}

impl State {
    pub(super) fn bind_agent_pane(
        &mut self,
        actor: &Actor,
        actor_id: String,
        pane_id: String,
        provider: Option<String>,
        revision: u64,
    ) -> Result<Applied> {
        validate_id("pane_id", &pane_id)?;
        if let Some(provider) = provider.as_deref() {
            validate_text("provider", provider, MAX_NAME_BYTES, false)?;
        }
        let team_id = self.bindable_team(actor, &actor_id)?;
        if self
            .pane_bindings
            .values()
            .any(|binding| binding.pane_id == pane_id && binding.actor_id != actor_id)
        {
            return Err(Error::new(
                "already_exists",
                "that pane is already bound to another agent",
            ));
        }
        let binding = PaneBinding {
            actor_id: actor_id.clone(),
            pane_id: pane_id.clone(),
            provider,
            revision,
        };
        self.pane_bindings.insert(actor_id.clone(), binding.clone());
        let project_id = self.require_team(&team_id)?.project_id.clone();
        Ok(applied(
            serialize_value(&binding)?,
            Some(team_id),
            Some(project_id),
            "agent_pane_bound",
            json!({ "actor_id": actor_id, "pane_id": pane_id }),
        ))
    }

    pub(super) fn unbind_agent_pane(
        &mut self,
        actor: &Actor,
        actor_id: String,
        _revision: u64,
    ) -> Result<Applied> {
        let team_id = self.bindable_team(actor, &actor_id)?;
        let binding = self
            .pane_bindings
            .remove(&actor_id)
            .ok_or_else(|| Error::new("not_found", "that agent has no bound pane"))?;
        let project_id = self.require_team(&team_id)?.project_id.clone();
        Ok(applied(
            serialize_value(&binding)?,
            Some(team_id),
            Some(project_id),
            "agent_pane_unbound",
            json!({ "actor_id": actor_id }),
        ))
    }

    /// The team of a non-owner agent that `actor` (the Owner, or that team's Lead) may bind.
    fn bindable_team(&self, actor: &Actor, actor_id: &str) -> Result<String> {
        let agent = self
            .actors
            .get(actor_id)
            .ok_or_else(|| Error::new("not_found", "agent not found"))?;
        let team_id = match (&agent.team_id, agent.role) {
            (Some(team_id), Role::Lead | Role::Developer | Role::Qa) => team_id.clone(),
            _ => {
                return Err(Error::new(
                    "invalid_reference",
                    "only a team Lead, Developer or QA can be bound to a pane",
                ));
            }
        };
        if actor.role != Role::Owner && actor.team_id.as_deref() != Some(team_id.as_str()) {
            return Err(Error::new(
                "forbidden",
                "a Lead binds panes only for their own team",
            ));
        }
        Ok(team_id)
    }

    pub(super) fn validate_bindings(&self) -> Result<()> {
        let mut panes = std::collections::BTreeSet::new();
        for binding in self.pane_bindings.values() {
            if !self.actors.contains_key(&binding.actor_id) || !panes.insert(&binding.pane_id) {
                return Err(Error::new(
                    "corrupt_state",
                    "a pane binding names an unknown agent or repeats a pane",
                ));
            }
        }
        Ok(())
    }
}

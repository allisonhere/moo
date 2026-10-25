//! Provider declarations, an explainable goal planner, and reviewable plans.
//!
//! Everything here is conservative and checkable. The planner is a fixed table of signals matched
//! against the words of a goal: no model, no randomness, no network. Every decision it makes lists
//! what it matched, and nothing it proposes happens until the Owner has reviewed and launched it.
//! Providers and their capabilities are *declared by the Owner*; nothing here verifies them, and
//! cost is never assumed.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    applied, require_role, serialize_value, validate_id, validate_text, Actor, Applied, Role,
    State, MAX_MESSAGE_BYTES, MAX_NAME_BYTES, MAX_TITLE_BYTES,
};
use crate::teams::{drafts, Error, Result};

pub const MAX_PLAN_TASKS: usize = 12;
pub const MAX_PLAN_RECRUITMENTS: usize = 4;
pub const MAX_PLANS: usize = 50;
pub const MAX_PROVIDERS: usize = 16;
pub const MAX_DEPENDENCY_DEPTH: usize = 6;
const MAX_GOAL_BYTES: usize = 2000;
const MAX_CAPABILITIES: usize = 24;

/// What a provider says about how it reports usage. Declared, not verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Metering {
    /// Usage and cost come from the provider's own accounting.
    Metered,
    /// Anything reported is an approximation made by the harness.
    Estimated,
    /// Nothing is reported.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelDecl {
    pub id: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub label: String,
    pub capabilities: BTreeSet<String>,
    pub models: Vec<ModelDecl>,
    pub metering: Metering,
    /// The integration can stop spending at a limit, not just stop starting new work.
    pub hard_spend_limit: bool,
    /// Always false: capabilities are the Owner's declaration, never checked by Teams.
    pub verified: bool,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preference {
    pub role_key: String,
    pub provider_id: String,
    pub model_id: Option<String>,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleAssignment {
    pub team_id: String,
    pub role_key: String,
    pub provider_id: String,
    pub model_id: Option<String>,
    pub since_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RolePlan {
    pub role_key: String,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub rationale: String,
    /// Capabilities the role needs that no declared provider offers.
    pub unmet: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanTask {
    pub key: String,
    pub title: String,
    pub description: String,
    pub depends_on: Vec<String>,
    pub removed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanRecruitment {
    pub key: String,
    pub specialty: String,
    /// The words of the goal that show the skill gap.
    pub because: Vec<String>,
    pub deliverable: String,
    pub capabilities: Vec<String>,
    /// The Lead requests the specialist once this task is finalized; nothing starts on its own.
    pub after_task: Option<String>,
    pub dropped: bool,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Explanation {
    pub decision: String,
    pub because: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanDraft {
    pub name: String,
    pub project_id: String,
    pub lead: String,
    pub developer: String,
    pub qa: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PlanStatus {
    Draft,
    Launched { team_id: String },
    Discarded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub goal: String,
    pub status: PlanStatus,
    pub draft: PlanDraft,
    pub roles: Vec<RolePlan>,
    pub tasks: Vec<PlanTask>,
    pub recruitments: Vec<PlanRecruitment>,
    pub explanations: Vec<Explanation>,
    pub warnings: Vec<String>,
    pub revision: u64,
}

/// A change the Owner makes to a plan before launching it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlanEdit {
    RenameTeam {
        name: String,
    },
    SetProject {
        project_id: String,
    },
    RemoveTask {
        key: String,
    },
    RestoreTask {
        key: String,
    },
    SetTaskTitle {
        key: String,
        title: String,
    },
    SetTaskDependencies {
        key: String,
        depends_on: Vec<String>,
    },
    AddTask {
        key: String,
        title: String,
        description: String,
        #[serde(default)]
        depends_on: Vec<String>,
    },
    DropRecruitment {
        key: String,
    },
    RestoreRecruitment {
        key: String,
    },
    SetRoleProvider {
        role_key: String,
        provider_id: String,
        #[serde(default)]
        model_id: Option<String>,
    },
}

// ---- the planner --------------------------------------------------------------------------------

struct Signal {
    id: &'static str,
    words: &'static [&'static str],
}

const SIGNALS: &[Signal] = &[
    Signal {
        id: "security",
        words: &[
            "oauth",
            "auth",
            "authentication",
            "authorization",
            "token",
            "credential",
            "password",
            "secret",
            "security",
            "encrypt",
            "crypto",
            "permission",
            "login",
            "signin",
        ],
    },
    Signal {
        id: "api",
        words: &[
            "api",
            "schema",
            "interface",
            "protocol",
            "endpoint",
            "contract",
            "wire",
        ],
    },
    Signal {
        id: "migration",
        words: &[
            "migrate",
            "migration",
            "backward",
            "compatibility",
            "breaking",
            "deprecate",
        ],
    },
    Signal {
        id: "architecture",
        words: &[
            "architecture",
            "architect",
            "design",
            "refactor",
            "rewrite",
            "restructure",
        ],
    },
    Signal {
        id: "performance",
        words: &[
            "performance",
            "latency",
            "benchmark",
            "throughput",
            "slow",
            "scale",
        ],
    },
    Signal {
        id: "docs",
        words: &[
            "doc",
            "docs",
            "document",
            "documentation",
            "readme",
            "guide",
            "changelog",
        ],
    },
    Signal {
        id: "tests",
        words: &[
            "test",
            "tests",
            "testing",
            "verify",
            "coverage",
            "regression",
        ],
    },
];

/// What the three core roles can do between them. A specialist is proposed only for a skill that is
/// not in this list.
pub const CORE_SKILLS: &[&str] = &["code", "testing", "planning", "documentation"];

/// The capabilities a role needs from its provider, by role key.
pub fn required_capabilities(role_key: &str) -> Vec<&'static str> {
    match role_key {
        "developer" => vec!["code_edit"],
        "qa" => vec!["shell"],
        key if key.starts_with("specialist:") => vec!["review"],
        _ => Vec::new(),
    }
}

fn tokens(goal: &str) -> Vec<String> {
    goal.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The words of the goal that matched one signal.
fn matched(goal_tokens: &[String], signal: &Signal) -> Vec<String> {
    let mut found: Vec<String> = goal_tokens
        .iter()
        .filter(|token| {
            signal
                .words
                .iter()
                .any(|word| token.as_str() == *word || (word.len() >= 6 && token.starts_with(word)))
        })
        .cloned()
        .collect();
    found.sort();
    found.dedup();
    found
}

fn short(goal: &str) -> String {
    let goal = goal.trim();
    if goal.chars().count() <= 80 {
        return goal.to_owned();
    }
    let cut: String = goal.chars().take(77).collect();
    format!("{cut}...")
}

impl State {
    /// The declared provider that should run `role_key`, and why. Preference first, then the first
    /// declared provider with what the role needs. Cost is never part of the choice.
    pub fn select_provider(&self, role_key: &str) -> RolePlan {
        let needs = required_capabilities(role_key);
        let offers = |provider: &Provider, model: Option<&str>| {
            let model_caps = model.and_then(|id| provider.models.iter().find(|m| m.id == id));
            needs.iter().all(|need| {
                provider.capabilities.contains(*need)
                    || model_caps.is_some_and(|m| m.capabilities.iter().any(|c| c == need))
            })
        };
        let generic = role_key.split(':').next().unwrap_or(role_key);
        let preference = self
            .preferences
            .get(role_key)
            .or_else(|| self.preferences.get(generic))
            .and_then(|pref| {
                let provider = self.providers.get(&pref.provider_id)?;
                offers(provider, pref.model_id.as_deref()).then_some((provider, pref))
            });
        if let Some((provider, pref)) = preference {
            return RolePlan {
                role_key: role_key.to_owned(),
                provider_id: Some(provider.id.clone()),
                model_id: pref.model_id.clone(),
                rationale: format!(
                    "your preference for '{}' is declared to offer what this role needs",
                    pref.role_key
                ),
                unmet: Vec::new(),
            };
        }
        if let Some(provider) = self.providers.values().find(|p| offers(p, None)) {
            return RolePlan {
                role_key: role_key.to_owned(),
                provider_id: Some(provider.id.clone()),
                model_id: provider.models.first().map(|m| m.id.clone()),
                rationale: "the first declared provider that offers the capabilities this role needs (not ranked by cost)".into(),
                unmet: Vec::new(),
            };
        }
        let unmet: Vec<String> = needs.iter().map(|n| (*n).to_owned()).collect();
        RolePlan {
            role_key: role_key.to_owned(),
            provider_id: None,
            model_id: None,
            rationale: if self.providers.is_empty() {
                "no providers are declared yet; assign one by hand".into()
            } else {
                format!(
                    "no declared provider offers {}; assign one by hand",
                    unmet.join(", ")
                )
            },
            unmet,
        }
    }

    /// Turns a goal into a reviewable plan. Pure: the same goal and declarations give the same plan.
    pub fn plan_goal(&self, plan_id: &str, goal: &str, revision: u64) -> Plan {
        let goal = goal.trim();
        let goal_tokens = tokens(goal);
        let hits: BTreeMap<&str, Vec<String>> = SIGNALS
            .iter()
            .map(|signal| (signal.id, matched(&goal_tokens, signal)))
            .filter(|(_, found)| !found.is_empty())
            .collect();
        let has = |id: &str| hits.contains_key(id);
        let quote = |id: &str| -> String {
            hits.get(id)
                .map(|words| {
                    format!(
                        "the goal mentions {}",
                        words
                            .iter()
                            .map(|w| format!("'{w}'"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })
                .unwrap_or_default()
        };
        let mut explanations = Vec::new();
        let mut tasks: Vec<PlanTask> = Vec::new();
        let design = has("architecture") || has("api") || has("migration");
        if design {
            let mut because = Vec::new();
            for id in ["architecture", "api", "migration"] {
                if has(id) {
                    because.push(quote(id));
                }
            }
            explanations.push(Explanation {
                decision: "task: design note before implementing".into(),
                because,
            });
            tasks.push(PlanTask {
                key: "design".into(),
                title: format!("Write a short design note for: {}", short(goal)),
                description: "Record the approach, the interfaces it touches and what could break, so review has something to check against.".into(),
                depends_on: Vec::new(),
                removed: false,
            });
        }
        tasks.push(PlanTask {
            key: "implement".into(),
            title: format!("Implement: {}", short(goal)),
            description: format!(
                "Deliver: {goal}. Hand off with the commit and the evidence that it works."
            ),
            depends_on: if design {
                vec!["design".into()]
            } else {
                Vec::new()
            },
            removed: false,
        });
        explanations.push(Explanation {
            decision: "task: implement".into(),
            because: vec!["every goal needs an implementation task".into()],
        });
        tasks.push(PlanTask {
            key: "tests".into(),
            title: format!("Test and verify: {}", short(goal)),
            description: "Run the relevant tests and add cases that would have caught the problem; attach the results.".into(),
            depends_on: vec!["implement".into()],
            removed: false,
        });
        explanations.push(Explanation {
            decision: "task: test and verify".into(),
            because: vec![if has("tests") {
                quote("tests")
            } else {
                "QA reviews a handoff, so there must be evidence to review".into()
            }],
        });
        if has("docs") {
            tasks.push(PlanTask {
                key: "docs".into(),
                title: format!("Update documentation for: {}", short(goal)),
                description: "Document what changed and how to use it, and hand the pages off with the evidence.".into(),
                depends_on: vec!["tests".into()],
                removed: false,
            });
            explanations.push(Explanation {
                decision: "task: documentation".into(),
                because: vec![quote("docs")],
            });
        }

        // A specialist only where the goal names a skill the core roles do not have.
        let mut recruitments = Vec::new();
        let mut warnings = vec![
            "Provider capabilities are what you declared; Teams has not verified them.".to_owned(),
            "Nothing here estimates cost: Teams only shows usage that a provider integration reports.".to_owned(),
        ];
        let gap = |skill: &str, because: Vec<String>| -> Explanation {
            Explanation {
                decision: format!("specialist: {skill}"),
                because: [
                    because,
                    vec![format!(
                        "the core roles cover {} but not {skill}",
                        CORE_SKILLS.join(", ")
                    )],
                ]
                .concat(),
            }
        };
        let mut specialists: Vec<(&str, Vec<String>, String, Option<&str>)> = Vec::new();
        if has("security") {
            specialists.push((
                "security",
                vec![quote("security")],
                format!("Security review notes for: {}", short(goal)),
                Some("tests"),
            ));
        }
        let design_signals = ["architecture", "api", "migration"]
            .iter()
            .filter(|id| has(id))
            .count();
        if design_signals >= 2 {
            specialists.push((
                "architect",
                ["architecture", "api", "migration"]
                    .iter()
                    .filter(|id| has(id))
                    .map(|id| quote(id))
                    .collect(),
                format!(
                    "Architecture review of the design note for: {}",
                    short(goal)
                ),
                Some("design"),
            ));
        }
        if has("performance") {
            specialists.push((
                "performance",
                vec![quote("performance")],
                format!("Performance review and measurements for: {}", short(goal)),
                Some("tests"),
            ));
        }
        for (skill, because, deliverable, after) in
            specialists.into_iter().take(MAX_PLAN_RECRUITMENTS)
        {
            explanations.push(gap(skill, because.clone()));
            let role = self.select_provider(&format!("specialist:{skill}"));
            if role.provider_id.is_none() {
                warnings.push(format!("specialist '{skill}': {}", role.rationale));
            }
            recruitments.push(PlanRecruitment {
                key: skill.to_owned(),
                specialty: skill.to_owned(),
                because,
                deliverable,
                capabilities: vec!["review".into(), "read_code".into()],
                after_task: after.map(str::to_owned),
                dropped: false,
                provider_id: role.provider_id,
                model_id: role.model_id,
            });
        }
        if recruitments.is_empty() {
            explanations.push(Explanation {
                decision: "no specialists".into(),
                because: vec!["the goal names no skill outside the core roles".into()],
            });
        }

        let roles: Vec<RolePlan> = ["lead", "developer", "qa"]
            .iter()
            .map(|role| self.select_provider(role))
            .collect();
        for role in &roles {
            if role.provider_id.is_none() {
                warnings.push(format!("{}: {}", role.role_key, role.rationale));
            }
        }
        let named = drafts::named(self, &short(goal).chars().take(60).collect::<String>());
        Plan {
            id: plan_id.to_owned(),
            goal: goal.to_owned(),
            status: PlanStatus::Draft,
            draft: PlanDraft {
                name: named.name,
                project_id: named.project_id,
                lead: named.lead,
                developer: named.developer,
                qa: named.qa,
            },
            roles,
            tasks,
            recruitments,
            explanations,
            warnings,
            revision,
        }
    }
}

/// Longest dependency chain (in tasks) among the kept tasks; `None` if there is a cycle or an
/// unknown dependency.
pub fn plan_depth(tasks: &[PlanTask]) -> Option<usize> {
    let kept: BTreeMap<&str, &PlanTask> = tasks
        .iter()
        .filter(|t| !t.removed)
        .map(|t| (t.key.as_str(), t))
        .collect();
    let mut depth: BTreeMap<&str, usize> = BTreeMap::new();
    let mut remaining: Vec<&str> = kept.keys().copied().collect();
    while !remaining.is_empty() {
        let before = remaining.len();
        remaining.retain(|key| {
            let task = kept[key];
            if task
                .depends_on
                .iter()
                .all(|dep| depth.contains_key(dep.as_str()))
            {
                let d = task
                    .depends_on
                    .iter()
                    .map(|dep| depth[dep.as_str()])
                    .max()
                    .unwrap_or(0)
                    + 1;
                depth.insert(key, d);
                false
            } else {
                true
            }
        });
        if remaining.len() == before {
            return None;
        }
    }
    if tasks
        .iter()
        .filter(|t| !t.removed)
        .any(|t| t.depends_on.iter().any(|d| !kept.contains_key(d.as_str())))
    {
        return None;
    }
    Some(depth.values().copied().max().unwrap_or(0))
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 32
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn validate_capabilities(field: &str, capabilities: &[String]) -> Result<BTreeSet<String>> {
    if capabilities.len() > MAX_CAPABILITIES {
        return Err(Error::new(
            "invalid_params",
            format!("{field}: name at most {MAX_CAPABILITIES} capabilities"),
        ));
    }
    let mut set = BTreeSet::new();
    for capability in capabilities {
        if capability.is_empty()
            || capability.len() > 48
            || !capability
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
        {
            return Err(Error::new(
                "invalid_params",
                format!("{field}: capabilities are lowercase words like code_edit"),
            ));
        }
        set.insert(capability.clone());
    }
    Ok(set)
}

impl State {
    #[allow(clippy::too_many_arguments)] // The declaration is the provider's whole record.
    pub(super) fn declare_provider(
        &mut self,
        actor: &Actor,
        provider_id: String,
        label: String,
        capabilities: Vec<String>,
        models: Vec<ModelDecl>,
        metering: Metering,
        hard_spend_limit: bool,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        validate_id("provider_id", &provider_id)?;
        validate_text("provider label", &label, MAX_NAME_BYTES, false)?;
        let capabilities = validate_capabilities("provider", &capabilities)?;
        if models.len() > MAX_CAPABILITIES {
            return Err(Error::new(
                "invalid_params",
                "a provider declares at most 24 models",
            ));
        }
        for model in &models {
            validate_id("model id", &model.id)?;
            validate_capabilities("model", &model.capabilities)?;
        }
        if !self.providers.contains_key(&provider_id) && self.providers.len() >= MAX_PROVIDERS {
            return Err(Error::new(
                "capacity_exceeded",
                "too many providers are declared",
            ));
        }
        let provider = Provider {
            id: provider_id.clone(),
            label,
            capabilities,
            models,
            metering,
            hard_spend_limit,
            verified: false,
            revision,
        };
        self.providers.insert(provider_id.clone(), provider.clone());
        Ok(applied(
            serialize_value(&provider)?,
            None,
            None,
            "provider_declared",
            json!({ "provider_id": provider_id, "metering": metering, "verified": false }),
        ))
    }

    pub(super) fn set_role_preference(
        &mut self,
        actor: &Actor,
        role_key: String,
        provider_id: String,
        model_id: Option<String>,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        self.validate_role_key(&role_key)?;
        self.require_declared(&provider_id, model_id.as_deref())?;
        let preference = Preference {
            role_key: role_key.clone(),
            provider_id,
            model_id,
            revision,
        };
        self.preferences
            .insert(role_key.clone(), preference.clone());
        Ok(applied(
            serialize_value(&preference)?,
            None,
            None,
            "role_preference_set",
            json!({ "role_key": role_key, "provider_id": preference.provider_id }),
        ))
    }

    fn validate_role_key(&self, role_key: &str) -> Result<()> {
        let ok = matches!(role_key, "lead" | "developer" | "qa" | "specialist")
            || role_key.strip_prefix("specialist:").is_some_and(valid_key);
        if !ok {
            return Err(Error::new(
                "invalid_params",
                "a role key is lead, developer, qa, specialist or specialist:<specialty>",
            ));
        }
        Ok(())
    }

    fn require_declared(&self, provider_id: &str, model_id: Option<&str>) -> Result<&Provider> {
        let provider = self
            .providers
            .get(provider_id)
            .ok_or_else(|| Error::new("not_found", "that provider is not declared"))?;
        if let Some(model) = model_id {
            if !provider.models.iter().any(|m| m.id == model) {
                return Err(Error::new(
                    "not_found",
                    "that model is not declared for the provider",
                ));
            }
        }
        Ok(provider)
    }

    pub(super) fn assign_role_provider(
        &mut self,
        actor: &Actor,
        team_id: String,
        role_key: String,
        provider_id: String,
        model_id: Option<String>,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        self.put_assignment(&team_id, role_key, provider_id, model_id, revision)
    }

    fn put_assignment(
        &mut self,
        team_id: &str,
        role_key: String,
        provider_id: String,
        model_id: Option<String>,
        revision: u64,
    ) -> Result<Applied> {
        let team = self.require_team(team_id)?.clone();
        self.validate_role_key(&role_key)?;
        let provider = self
            .require_declared(&provider_id, model_id.as_deref())?
            .clone();
        let needs = required_capabilities(&role_key);
        let model_caps = model_id
            .as_deref()
            .and_then(|id| provider.models.iter().find(|m| m.id == id));
        let missing: Vec<&str> = needs
            .iter()
            .copied()
            .filter(|need| {
                !provider.capabilities.contains(*need)
                    && !model_caps.is_some_and(|m| m.capabilities.iter().any(|c| c == need))
            })
            .collect();
        if !missing.is_empty() {
            return Err(Error::new(
                "capability_missing",
                format!(
                    "'{provider_id}' is not declared to offer {}",
                    missing.join(", ")
                ),
            ));
        }
        let assignment = RoleAssignment {
            team_id: team_id.to_owned(),
            role_key: role_key.clone(),
            provider_id,
            model_id,
            since_revision: revision,
        };
        self.role_assignments
            .insert(format!("{team_id}:{role_key}"), assignment.clone());
        Ok(applied(
            serialize_value(&assignment)?,
            Some(team_id.to_owned()),
            Some(team.project_id),
            "role_provider_assigned",
            json!({ "team_id": team_id, "role_key": assignment.role_key, "provider_id": assignment.provider_id,
                "model_id": assignment.model_id }),
        ))
    }

    /// A Lead's request to change a role's provider mid-run. It needs an Owner approval naming
    /// exactly this team, role, provider and model, spent here and nowhere else.
    pub(super) fn reassign_role_provider(
        &mut self,
        actor: &Actor,
        team_id: String,
        role_key: String,
        provider_id: String,
        model_id: Option<String>,
        approval_id: String,
        revision: u64,
    ) -> Result<Applied> {
        self.require_team_actor(actor, &team_id, Role::Lead)?;
        let action = reassign_action(&team_id, &role_key, &provider_id, model_id.as_deref());
        self.spend_approval(
            actor,
            &approval_id,
            &format!("reassign-{team_id}-{role_key}"),
            &action,
            revision,
        )?;
        self.put_assignment(&team_id, role_key, provider_id, model_id, revision)
    }

    // ---- plans ------------------------------------------------------------------------------

    pub(super) fn propose_plan(
        &mut self,
        actor: &Actor,
        plan_id: String,
        goal: String,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        validate_id("plan_id", &plan_id)?;
        validate_text("goal", &goal, MAX_GOAL_BYTES, false)?;
        if self.plans.contains_key(&plan_id) {
            return Err(Error::new("already_exists", "plan already exists"));
        }
        let open = self
            .plans
            .values()
            .filter(|p| p.status == PlanStatus::Draft)
            .count();
        if self.plans.len() >= MAX_PLANS || open >= 10 {
            return Err(Error::new(
                "capacity_exceeded",
                "too many plans; discard or launch some first",
            ));
        }
        let plan = self.plan_goal(&plan_id, &goal, revision);
        self.plans.insert(plan_id.clone(), plan.clone());
        Ok(applied(
            serialize_value(&plan)?,
            None,
            None,
            "plan_proposed",
            json!({ "plan_id": plan_id, "tasks": plan.tasks.len(), "specialists": plan.recruitments.len() }),
        ))
    }

    pub(super) fn edit_plan(
        &mut self,
        actor: &Actor,
        plan_id: String,
        edits: Vec<PlanEdit>,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        if edits.is_empty() || edits.len() > 32 {
            return Err(Error::new("invalid_params", "send between 1 and 32 edits"));
        }
        let mut plan = self
            .plans
            .get(&plan_id)
            .cloned()
            .ok_or_else(|| Error::new("not_found", "plan not found"))?;
        if plan.status != PlanStatus::Draft {
            return Err(Error::new(
                "invalid_state",
                "only a draft plan can be edited",
            ));
        }
        for edit in edits {
            self.apply_plan_edit(&mut plan, edit)?;
        }
        if plan.tasks.iter().filter(|t| !t.removed).count() > MAX_PLAN_TASKS {
            return Err(Error::new(
                "capacity_exceeded",
                format!("a plan has at most {MAX_PLAN_TASKS} tasks"),
            ));
        }
        match plan_depth(&plan.tasks) {
            None => {
                return Err(Error::new(
                    "dependency_cycle",
                    "task dependencies form a cycle or name a removed or unknown task",
                ))
            }
            Some(depth) if depth > MAX_DEPENDENCY_DEPTH => {
                return Err(Error::new(
                    "task_depth_limit",
                    format!("dependency chains are at most {MAX_DEPENDENCY_DEPTH} tasks deep"),
                ));
            }
            Some(_) => {}
        }
        // A specialist must still follow a task that exists in the plan.
        for recruitment in plan.recruitments.iter_mut().filter(|r| !r.dropped) {
            if let Some(after) = &recruitment.after_task {
                if !plan.tasks.iter().any(|t| !t.removed && &t.key == after) {
                    recruitment.after_task = None;
                }
            }
        }
        plan.revision = revision;
        self.plans.insert(plan_id.clone(), plan.clone());
        Ok(applied(
            serialize_value(&plan)?,
            None,
            None,
            "plan_edited",
            json!({ "plan_id": plan_id }),
        ))
    }

    fn apply_plan_edit(&self, plan: &mut Plan, edit: PlanEdit) -> Result<()> {
        let task_index = |plan: &Plan, key: &str| {
            plan.tasks
                .iter()
                .position(|t| t.key == key)
                .ok_or_else(|| Error::new("not_found", format!("task '{key}' is not in the plan")))
        };
        match edit {
            PlanEdit::RenameTeam { name } => {
                validate_text("team name", &name, MAX_NAME_BYTES, false)?;
                plan.draft.name = name;
            }
            PlanEdit::SetProject { project_id } => {
                if !project_id.is_empty() && !self.projects.contains_key(&project_id) {
                    return Err(Error::new("not_found", "project not found"));
                }
                plan.draft.project_id = project_id;
            }
            PlanEdit::RemoveTask { key } => {
                let i = task_index(plan, &key)?;
                plan.tasks[i].removed = true;
            }
            PlanEdit::RestoreTask { key } => {
                let i = task_index(plan, &key)?;
                plan.tasks[i].removed = false;
            }
            PlanEdit::SetTaskTitle { key, title } => {
                validate_text("task title", &title, MAX_TITLE_BYTES, false)?;
                let i = task_index(plan, &key)?;
                plan.tasks[i].title = title;
            }
            PlanEdit::SetTaskDependencies { key, depends_on } => {
                let i = task_index(plan, &key)?;
                plan.tasks[i].depends_on = depends_on;
            }
            PlanEdit::AddTask {
                key,
                title,
                description,
                depends_on,
            } => {
                if !valid_key(&key) {
                    return Err(Error::new(
                        "invalid_params",
                        "a task key is lowercase letters, digits and dashes",
                    ));
                }
                if plan.tasks.iter().any(|t| t.key == key) {
                    return Err(Error::new(
                        "already_exists",
                        "a task with that key is in the plan",
                    ));
                }
                validate_text("task title", &title, MAX_TITLE_BYTES, false)?;
                validate_text("task description", &description, MAX_MESSAGE_BYTES, false)?;
                plan.tasks.push(PlanTask {
                    key,
                    title,
                    description,
                    depends_on,
                    removed: false,
                });
            }
            PlanEdit::DropRecruitment { key } => {
                let r = plan
                    .recruitments
                    .iter_mut()
                    .find(|r| r.key == key)
                    .ok_or_else(|| Error::new("not_found", "that specialist is not in the plan"))?;
                r.dropped = true;
            }
            PlanEdit::RestoreRecruitment { key } => {
                let r = plan
                    .recruitments
                    .iter_mut()
                    .find(|r| r.key == key)
                    .ok_or_else(|| Error::new("not_found", "that specialist is not in the plan"))?;
                r.dropped = false;
            }
            PlanEdit::SetRoleProvider {
                role_key,
                provider_id,
                model_id,
            } => {
                self.validate_role_key(&role_key)?;
                let provider = self.require_declared(&provider_id, model_id.as_deref())?;
                let needs = required_capabilities(&role_key);
                let missing: Vec<&str> = needs
                    .iter()
                    .copied()
                    .filter(|n| !provider.capabilities.contains(*n))
                    .collect();
                if !missing.is_empty() {
                    return Err(Error::new(
                        "capability_missing",
                        format!(
                            "'{provider_id}' is not declared to offer {}",
                            missing.join(", ")
                        ),
                    ));
                }
                let entry = RolePlan {
                    role_key: role_key.clone(),
                    provider_id: Some(provider_id),
                    model_id,
                    rationale: "chosen by the Owner while reviewing the plan".into(),
                    unmet: Vec::new(),
                };
                if let Some(role) = plan.roles.iter_mut().find(|r| r.role_key == role_key) {
                    *role = entry;
                } else if let Some(r) = plan
                    .recruitments
                    .iter_mut()
                    .find(|r| format!("specialist:{}", r.specialty) == role_key)
                {
                    r.provider_id = entry.provider_id;
                    r.model_id = entry.model_id;
                } else {
                    return Err(Error::new("not_found", "that role is not in the plan"));
                }
            }
        }
        Ok(())
    }

    pub(super) fn discard_plan(
        &mut self,
        actor: &Actor,
        plan_id: String,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        let plan = self
            .plans
            .get_mut(&plan_id)
            .ok_or_else(|| Error::new("not_found", "plan not found"))?;
        if plan.status != PlanStatus::Draft {
            return Err(Error::new(
                "invalid_state",
                "only a draft plan can be discarded",
            ));
        }
        plan.status = PlanStatus::Discarded;
        plan.revision = revision;
        let result = serialize_value(&*plan)?;
        Ok(applied(
            result,
            None,
            None,
            "plan_discarded",
            json!({ "plan_id": plan_id }),
        ))
    }

    /// Recorded by the launch pipeline once the team and its tasks exist.
    pub(super) fn mark_plan_launched(
        &mut self,
        actor: &Actor,
        plan_id: String,
        team_id: String,
        revision: u64,
    ) -> Result<Applied> {
        require_role(actor, Role::Owner)?;
        self.require_team(&team_id)?;
        let plan = self
            .plans
            .get_mut(&plan_id)
            .ok_or_else(|| Error::new("not_found", "plan not found"))?;
        if plan.status != PlanStatus::Draft {
            return Err(Error::new(
                "invalid_state",
                "only a draft plan can be launched",
            ));
        }
        plan.status = PlanStatus::Launched {
            team_id: team_id.clone(),
        };
        plan.revision = revision;
        let result = serialize_value(&*plan)?;
        Ok(applied(
            result,
            Some(team_id.clone()),
            None,
            "plan_launched",
            json!({ "plan_id": plan_id, "team_id": team_id }),
        ))
    }

    pub(super) fn validate_planning(&self) -> Result<()> {
        let corrupt = |m: &str| Error::new("corrupt_state", m.to_owned());
        for pref in self.preferences.values() {
            if !self.providers.contains_key(&pref.provider_id) {
                return Err(corrupt("a preference names an undeclared provider"));
            }
        }
        for assignment in self.role_assignments.values() {
            if !self.teams.contains_key(&assignment.team_id)
                || !self.providers.contains_key(&assignment.provider_id)
            {
                return Err(corrupt(
                    "a role assignment names an unknown team or provider",
                ));
            }
        }
        for plan in self.plans.values() {
            if plan_depth(&plan.tasks).is_none() {
                return Err(corrupt("a plan's tasks are not a valid graph"));
            }
            if let PlanStatus::Launched { team_id } = &plan.status {
                if !self.teams.contains_key(team_id) {
                    return Err(corrupt("a launched plan names an unknown team"));
                }
            }
        }
        Ok(())
    }
}

/// The action an Owner approval must name before a Lead may change a role's provider.
pub fn reassign_action(
    team_id: &str,
    role_key: &str,
    provider_id: &str,
    model_id: Option<&str>,
) -> crate::teams::model::ProtectedAction {
    crate::teams::model::ProtectedAction {
        kind: super::governance::ProtectedActionKind::RightsEscalation,
        target: format!("{team_id}:{role_key}"),
        parameters: json!({ "provider_id": provider_id, "model_id": model_id }),
    }
}

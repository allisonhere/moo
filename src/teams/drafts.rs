//! Team drafts: construct a team by hand or from a goal, validate it, and launch it.
//!
//! This is the one implementation. The Teams screen and external clients call the same methods
//! (`teams.drafts.propose.v1`, `.validate.v1`, `.launch.v1`); nothing about how a draft becomes a
//! team lives in a client.

use std::io::{Read, Write};
use std::process::{Command as Process, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::model::{Actor, Command, Role, State};
use super::{Error, Result};

const MAX_NAME: usize = 128;
const MAX_GOAL_BYTES: usize = 2000;
const MAX_PROPOSAL_BYTES: u64 = 16 * 1024;
const PROPOSER_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const PROPOSER_ENV: &str = "MOO_TEAMS_PROPOSER";
/// Where launched teams' credential files go, under the session data directory.
pub(super) const KEYS_DIR: &str = "teams-keys";

/// What a client edits: the three roles and the project. Everything else is derived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct DraftInput {
    pub name: String,
    /// Empty means "no project yet": launching creates a default one.
    #[serde(default)]
    pub project_id: String,
    pub lead: String,
    pub developer: String,
    pub qa: String,
}

pub(super) fn slug(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_owned();
    out.chars()
        .take(40)
        .collect::<String>()
        .trim_end_matches('-')
        .to_owned()
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_NAME
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':'))
}

fn base_name(name: &str) -> String {
    let base = slug(name);
    if base.is_empty() {
        "team".to_owned()
    } else {
        base
    }
}

/// A team id nobody uses yet: the slug of the name, then `-2`, `-3`, ...
pub(super) fn team_id(state: &State, draft: &DraftInput) -> String {
    let base = base_name(&draft.name);
    let mut candidate = base.clone();
    let mut n = 1;
    while state.teams.contains_key(&candidate)
        || state.team_specs.contains_key(&format!("{candidate}-spec"))
    {
        n += 1;
        candidate = format!("{base}-{n}");
    }
    candidate
}

/// Everything that would make the server refuse the draft, said in advance and in plain words.
pub(super) fn problems(state: &State, draft: &DraftInput) -> Vec<String> {
    let mut problems = Vec::new();
    if draft.name.trim().is_empty() || draft.name.len() > MAX_NAME {
        problems.push(format!(
            "the team needs a name of 1 to {MAX_NAME} characters"
        ));
    }
    if !draft.project_id.is_empty() && !state.projects.contains_key(&draft.project_id) {
        problems.push(format!("project '{}' does not exist", draft.project_id));
    }
    for (role, id) in [
        ("lead", &draft.lead),
        ("developer", &draft.developer),
        ("qa", &draft.qa),
    ] {
        if !valid_id(id) {
            problems.push(format!("{role} id must be letters, digits, . _ - or :"));
        } else if state.actors.contains_key(id.as_str())
            || state.team_specs.values().any(|spec| {
                spec.enrollments
                    .values()
                    .any(|enrollment| &enrollment.actor_id == id)
            })
        {
            problems.push(format!("{role} id '{id}' is already taken"));
        }
    }
    if draft.lead == draft.developer || draft.lead == draft.qa || draft.developer == draft.qa {
        problems.push("the three roles need three different ids".into());
    }
    problems
}

/// A starting draft with ids derived from the name, never colliding with an existing team or agent.
pub(super) fn named(state: &State, name: &str) -> DraftInput {
    let mut candidate = base_name(name);
    let base = candidate.clone();
    let mut n = 1;
    while state.teams.contains_key(&candidate)
        || state.actors.contains_key(&format!("{candidate}-lead"))
    {
        n += 1;
        candidate = format!("{base}-{n}");
    }
    DraftInput {
        name: name.to_owned(),
        project_id: state.projects.keys().next().cloned().unwrap_or_default(),
        lead: format!("{candidate}-lead"),
        developer: format!("{candidate}-dev"),
        qa: format!("{candidate}-qa"),
    }
}

/// What a proposal returns: the draft, and where it came from.
#[derive(Debug)]
pub(super) struct Proposal {
    pub draft: DraftInput,
    pub source: &'static str,
}

/// A draft from a goal. With a proposer command configured (by the operator, in the server's
/// environment, never by the caller) it runs the command with the goal on stdin and reads a JSON
/// draft; otherwise the goal becomes a plain template named from its first words.
pub(super) fn propose(state: &State, goal: &str, command: Option<&str>) -> Result<Proposal> {
    let goal = goal.trim();
    if goal.is_empty() || goal.len() > MAX_GOAL_BYTES {
        return Err(Error::new(
            "invalid_params",
            format!("the goal must be 1 to {MAX_GOAL_BYTES} bytes"),
        ));
    }
    let Some(command) = command.map(str::trim).filter(|c| !c.is_empty()) else {
        let name: String = goal
            .split_whitespace()
            .take(5)
            .collect::<Vec<_>>()
            .join(" ");
        return Ok(Proposal {
            draft: named(state, &name),
            source: "template",
        });
    };
    let value = run_proposer(command, goal)?;
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| Error::new("proposer_failed", "the proposal has no team name"))?;
    let mut draft = named(state, name);
    for (field, key) in [
        (&mut draft.lead, "lead"),
        (&mut draft.developer, "developer"),
        (&mut draft.qa, "qa"),
    ] {
        if let Some(id) = value.get(key).and_then(Value::as_str) {
            *field = id.trim().to_owned();
        }
    }
    Ok(Proposal {
        draft,
        source: "proposer",
    })
}

fn run_proposer(command: &str, goal: &str) -> Result<Value> {
    let failed = |message: &str| Error::new("proposer_failed", message.to_owned());
    let mut words = command.split_whitespace();
    let program = words
        .next()
        .ok_or_else(|| failed("empty proposer command"))?;
    let mut child = Process::new(program)
        .args(words)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| failed("could not start the proposer"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(goal.as_bytes());
    }
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| failed("no proposer output"))?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = (&mut stdout)
            .take(MAX_PROPOSAL_BYTES + 1)
            .read_to_end(&mut bytes);
        let status = child.wait();
        let _ = tx.send((read, status, bytes));
    });
    let (read, status, bytes) = rx
        .recv_timeout(PROPOSER_TIMEOUT)
        .map_err(|_| failed("the proposer took too long"))?;
    if read.is_err() || !status.is_ok_and(|status| status.success()) {
        return Err(failed("the proposer failed"));
    }
    if bytes.len() as u64 > MAX_PROPOSAL_BYTES {
        return Err(failed("the proposal is too large"));
    }
    serde_json::from_slice(&bytes).map_err(|_| failed("the proposer did not return JSON"))
}

/// The Teams commands that realize a draft, in order. `digests` are the public digests of the three
/// freshly created credentials (lead, developer, qa).
pub(super) fn plan(state: &State, draft: &DraftInput, digests: &[String; 3]) -> Vec<Command> {
    let team_id = team_id(state, draft);
    let spec_id = format!("{team_id}-spec");
    let mut commands = Vec::new();
    let project_id = if draft.project_id.is_empty() {
        // "No project" means the default one: made the first time, reused after that.
        if !state.projects.contains_key("default") {
            commands.push(Command::CreateProject {
                project_id: "default".into(),
                name: "Default".into(),
            });
        }
        "default".to_owned()
    } else {
        draft.project_id.clone()
    };
    commands.push(Command::CreateTeamSpec {
        spec_id: spec_id.clone(),
        project_id,
        name: draft.name.trim().to_owned(),
    });
    for ((role, id), digest) in [
        (Role::Lead, &draft.lead),
        (Role::Developer, &draft.developer),
        (Role::Qa, &draft.qa),
    ]
    .into_iter()
    .zip(digests)
    {
        commands.push(Command::EnrollActor {
            spec_id: spec_id.clone(),
            actor_id: id.clone(),
            role,
            credential_digest: digest.clone(),
            run_generation: 1,
        });
    }
    commands.push(Command::LaunchTeam { spec_id, team_id });
    commands
}

/// The `teams.drafts.validate.v1` result.
pub(super) fn preview(state: &State, draft: &DraftInput) -> Value {
    let problems = problems(state, draft);
    let team_id = team_id(state, draft);
    json!({
        "type": "teams_draft_validation",
        "valid": problems.is_empty(),
        "problems": problems,
        "team_id": team_id,
        "spec_id": format!("{team_id}-spec"),
        "project_id": if draft.project_id.is_empty() { "default" } else { draft.project_id.as_str() },
        "creates_project": draft.project_id.is_empty() && !state.projects.contains_key("default"),
        "credential_dir": format!("{KEYS_DIR}/{team_id}"),
        "steps": ["create_team_spec", "enroll_actor", "enroll_actor", "enroll_actor", "launch_team"],
    })
}

/// Applies the plan to `state` as `owner`, all or nothing: the caller commits only if this returns Ok.
pub(super) fn apply(
    state: &mut State,
    owner: &Actor,
    commands: Vec<Command>,
    now: u64,
) -> Result<()> {
    for command in commands {
        state.execute_at(owner, command, now)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::model::State;

    fn owner_state() -> (State, Actor) {
        let digest = "0".repeat(64);
        let mut state = State::new("s".into(), digest.clone());
        let owner = state.authenticate(&digest).unwrap();
        state
            .execute(
                &owner,
                Command::CreateProject {
                    project_id: "moo".into(),
                    name: "Moo".into(),
                },
            )
            .unwrap();
        (state, owner)
    }

    fn digests() -> [String; 3] {
        ["a".repeat(64), "b".repeat(64), "c".repeat(64)]
    }

    fn draft(state: &State, name: &str) -> DraftInput {
        named(state, name)
    }

    #[test]
    fn a_draft_launches_a_real_team_in_one_all_or_nothing_step() {
        let (mut state, owner) = owner_state();
        let d = draft(&state, "Docs team");
        assert!(problems(&state, &d).is_empty());
        let commands = plan(&state, &d, &digests());
        assert_eq!(commands.len(), 5);
        apply(&mut state, &owner, commands, 10).unwrap();
        let team = &state.teams["docs-team"];
        assert_eq!(
            (
                team.lead_id.as_str(),
                team.developer_id.as_str(),
                team.qa_id.as_str()
            ),
            ("docs-team-lead", "docs-team-dev", "docs-team-qa")
        );
        // A second team of the same name gets its own id instead of colliding.
        let again = draft(&state, "Docs team");
        assert_eq!(team_id(&state, &again), "docs-team-2");
        assert!(problems(&state, &again).is_empty());
    }

    #[test]
    fn problems_are_reported_in_advance() {
        let (mut state, owner) = owner_state();
        let d = draft(&state, "First");
        let steps = plan(&state, &d, &digests());
        apply(&mut state, &owner, steps, 1).unwrap();
        let mut clash = draft(&state, "Second");
        clash.lead = "first-lead".into();
        clash.developer = "same".into();
        clash.qa = "same".into();
        clash.project_id = "nope".into();
        clash.name = " ".into();
        let found = problems(&state, &clash);
        for expected in [
            "needs a name",
            "does not exist",
            "already taken",
            "different ids",
        ] {
            assert!(
                found.iter().any(|p| p.contains(expected)),
                "{expected}: {found:?}"
            );
        }
        let bad_id = DraftInput {
            lead: "bad id!".into(),
            ..draft(&state, "Third")
        };
        assert!(problems(&state, &bad_id)
            .iter()
            .any(|p| p.contains("letters, digits")));
        let preview = preview(&state, &clash);
        assert_eq!(preview["valid"], false);
    }

    #[test]
    fn a_failed_step_leaves_the_state_untouched_for_the_caller_to_discard() {
        let (state, owner) = owner_state();
        let mut candidate = state.clone();
        let mut commands = plan(&state, &draft(&state, "Broken"), &digests());
        commands.insert(
            2,
            Command::CreateProject {
                project_id: "moo".into(),
                name: "Dup".into(),
            },
        );
        assert!(apply(&mut candidate, &owner, commands, 1).is_err());
        // The service commits only on Ok, so the original is what stays.
        assert!(state.teams.is_empty() && state.team_specs.is_empty());
    }

    #[test]
    fn two_projectless_teams_share_the_default_project() {
        let digest = "0".repeat(64);
        let mut state = State::new("s".into(), digest.clone());
        let owner = state.authenticate(&digest).unwrap();
        for (name, seed) in [("First", "a"), ("Second", "b")] {
            let d = draft(&state, name);
            let steps = plan(
                &state,
                &d,
                &[
                    seed.repeat(64),
                    "c".repeat(63) + seed,
                    "d".repeat(63) + seed,
                ],
            );
            apply(&mut state, &owner, steps, 1).unwrap();
        }
        assert_eq!(state.projects.len(), 1);
        assert_eq!(state.teams.len(), 2);
    }

    #[test]
    fn no_project_means_a_default_one_is_created() {
        let digest = "0".repeat(64);
        let mut state = State::new("s".into(), digest.clone());
        let owner = state.authenticate(&digest).unwrap();
        let d = draft(&state, "First");
        assert_eq!(d.project_id, "");
        assert_eq!(preview(&state, &d)["creates_project"], true);
        let steps = plan(&state, &d, &digests());
        apply(&mut state, &owner, steps, 1).unwrap();
        assert!(state.projects.contains_key("default") && state.teams.contains_key("first"));
    }

    #[test]
    fn a_goal_without_a_proposer_gives_a_template_and_bad_goals_are_refused() {
        let (state, _) = owner_state();
        let proposal = propose(
            &state,
            "Build the plugin settings screen and document it properly",
            None,
        )
        .unwrap();
        assert_eq!(
            (proposal.source, proposal.draft.name.as_str()),
            ("template", "Build the plugin settings screen")
        );
        assert_eq!(
            propose(&state, "  ", None).unwrap_err().code,
            "invalid_params"
        );
        assert_eq!(
            propose(&state, &"x".repeat(2001), None).unwrap_err().code,
            "invalid_params"
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_proposer_command_is_run_with_the_goal_and_its_output_is_checked() {
        use std::os::unix::fs::PermissionsExt;
        let (state, _) = owner_state();
        let dir =
            std::env::temp_dir().join(format!("moo-proposer-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = |name: &str, body: &str| {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path.display().to_string()
        };
        let good = script(
            "good",
            r#"read goal; printf '{"name":"Team for %s","lead":"p-lead"}' "$goal""#,
        );
        let proposal = propose(&state, "shipping", Some(&good)).unwrap();
        assert_eq!(proposal.source, "proposer");
        assert_eq!(
            (proposal.draft.name.as_str(), proposal.draft.lead.as_str()),
            ("Team for shipping", "p-lead")
        );
        let failing = script("failing", "exit 3");
        let junk = script("junk", "echo not json");
        let nameless = script("nameless", r#"echo '{"lead":"x"}'"#);
        for bad in [
            failing,
            junk,
            nameless,
            "/definitely/not/a/program".to_owned(),
            "true".to_owned(),
        ] {
            assert_eq!(
                propose(&state, "ship it", Some(&bad)).err().map(|e| e.code),
                Some("proposer_failed"),
                "{bad}"
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn slugs_are_stable_and_bounded() {
        assert_eq!(slug("  Hello, World!! "), "hello-world");
        assert_eq!(slug("!!!"), "");
        assert!(slug(&"word ".repeat(40)).len() <= 40);
    }
}

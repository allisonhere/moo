//! What an agent is told: accepted requirements and decisions, the task in front of it, the exact
//! handoff and review evidence, and summaries that say where they came from and when they stop
//! being true. Never chat transcripts, never scoped messages.
//!
//! A summary is only ever a pointer to its sources. When a source changes after the summary was
//! written, the summary is marked stale and left out, so an old conclusion cannot quietly become
//! the current truth.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    applied, serialize_value, validate_id, validate_text, Actor, Applied, KnowledgeScope, Role,
    State, TaskStatus,
};
use crate::teams::{Error, Result};

pub const DEFAULT_BUDGET: usize = 8 * 1024;
pub const MAX_BUDGET: usize = 32 * 1024;
const MAX_NOTES_PER_TEAM: usize = 200;
const MAX_SOURCES: usize = 8;
const MAX_SUMMARY_BYTES: usize = 2000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeKind {
    #[default]
    Note,
    Requirement,
    Adr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeStatus {
    Proposed,
    Accepted,
    Superseded,
}

impl Default for KnowledgeStatus {
    fn default() -> Self {
        // Entries written before kinds existed were plain notes and count as accepted.
        Self::Accepted
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Task,
    Handoff,
    Decision,
    Knowledge,
    Dispute,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceRef {
    pub kind: SourceKind,
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    /// A short account of a thread or task.
    Summary,
    /// What a specialist role learned, kept so the next engagement can start informed.
    RoleContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextNote {
    pub id: String,
    pub team_id: String,
    pub kind: NoteKind,
    /// The specialty this context is for (`security`), or none for the whole team.
    pub role_key: Option<String>,
    pub summary: String,
    pub sources: Vec<SourceRef>,
    /// The state of the world it describes: sources that changed after this make it stale.
    pub as_of_revision: u64,
    pub valid_until: Option<u64>,
    pub superseded_by: Option<String>,
    pub author_id: String,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Validity {
    Current,
    Stale(String),
}

impl State {
    fn source_revision(&self, source: &SourceRef) -> Option<u64> {
        match source.kind {
            SourceKind::Task => self.tasks.get(&source.id).map(|r| r.revision),
            SourceKind::Handoff => self.handoffs.get(&source.id).map(|r| r.revision),
            SourceKind::Decision => self.decisions.get(&source.id).map(|r| r.revision),
            SourceKind::Knowledge => self.knowledge.get(&source.id).map(|r| r.revision),
            SourceKind::Dispute => self.disputes.get(&source.id).map(|r| r.revision),
        }
    }

    /// Whether a note still describes the world. Stale notes are kept for the record but never served.
    pub fn note_validity(&self, note: &ContextNote, now: u64) -> Validity {
        if let Some(by) = &note.superseded_by {
            return Validity::Stale(format!("superseded by {by}"));
        }
        if note.valid_until.is_some_and(|until| now > until) {
            return Validity::Stale("its validity window ended".into());
        }
        for source in &note.sources {
            match self.source_revision(source) {
                None => {
                    return Validity::Stale(
                        format!("{:?} {} no longer exists", source.kind, source.id).to_lowercase(),
                    )
                }
                Some(revision) if revision > note.as_of_revision => {
                    return Validity::Stale(format!(
                        "{} {} changed at revision {revision}, after this was written at {}",
                        format!("{:?}", source.kind).to_lowercase(),
                        source.id,
                        note.as_of_revision
                    ));
                }
                Some(_) => {}
            }
        }
        Validity::Current
    }

    #[allow(clippy::too_many_arguments)] // A note is its text, its sources and its validity.
    pub(super) fn put_context_note(
        &mut self,
        actor: &Actor,
        team_id: String,
        note_id: String,
        kind: NoteKind,
        role_key: Option<String>,
        summary: String,
        sources: Vec<SourceRef>,
        valid_for_seconds: Option<u64>,
        revision: u64,
    ) -> Result<Applied> {
        let team = self.require_team_member_or_owner(actor, &team_id)?.clone();
        validate_id("note_id", &note_id)?;
        validate_text("summary", &summary, MAX_SUMMARY_BYTES, false)?;
        if let Some(role) = role_key.as_deref() {
            validate_id("role_key", role)?;
        }
        if sources.is_empty() || sources.len() > MAX_SOURCES {
            return Err(Error::new("invalid_params", format!("name between 1 and {MAX_SOURCES} sources: a summary without provenance is not accepted")));
        }
        for source in &sources {
            if !self.source_belongs_to_team(source, &team_id) {
                return Err(Error::new(
                    "invalid_reference",
                    format!("source {} is not a record of this team", source.id),
                ));
            }
        }
        if valid_for_seconds == Some(0) {
            return Err(Error::new(
                "invalid_params",
                "a validity window of zero seconds is already over",
            ));
        }
        if self.context_notes.contains_key(&note_id) {
            return Err(Error::new("already_exists", "note already exists"));
        }
        if self
            .context_notes
            .values()
            .filter(|n| n.team_id == team_id)
            .count()
            >= MAX_NOTES_PER_TEAM
        {
            return Err(Error::new(
                "capacity_exceeded",
                "the team has too many context notes; supersede old ones",
            ));
        }
        let note = ContextNote {
            id: note_id.clone(),
            team_id: team_id.clone(),
            kind,
            role_key,
            summary,
            sources,
            // Everything committed so far is what the note was written against.
            as_of_revision: revision.saturating_sub(1),
            valid_until: valid_for_seconds.map(|s| self.now.saturating_add(s)),
            superseded_by: None,
            author_id: actor.id.clone(),
            revision,
        };
        self.context_notes.insert(note_id.clone(), note.clone());
        Ok(applied(
            serialize_value(&note)?,
            Some(team_id),
            Some(team.project_id),
            "context_note_recorded",
            json!({ "note_id": note_id, "kind": kind }),
        ))
    }

    fn source_belongs_to_team(&self, source: &SourceRef, team_id: &str) -> bool {
        match source.kind {
            SourceKind::Task => self
                .tasks
                .get(&source.id)
                .is_some_and(|r| r.team_id == team_id),
            SourceKind::Handoff => self
                .handoffs
                .get(&source.id)
                .is_some_and(|r| r.team_id == team_id),
            SourceKind::Decision => self
                .decisions
                .get(&source.id)
                .is_some_and(|r| r.team_id == team_id),
            SourceKind::Knowledge => self.knowledge.get(&source.id).is_some_and(|r| {
                r.team_id == team_id
                    || matches!(&r.scope, KnowledgeScope::Project { project_id }
                        if self.teams.get(team_id).is_some_and(|t| &t.project_id == project_id))
            }),
            SourceKind::Dispute => self
                .disputes
                .get(&source.id)
                .is_some_and(|r| r.team_id == team_id),
        }
    }

    pub(super) fn supersede_context_note(
        &mut self,
        actor: &Actor,
        note_id: String,
        by_note_id: String,
        revision: u64,
    ) -> Result<Applied> {
        let note = self
            .context_notes
            .get(&note_id)
            .ok_or_else(|| Error::new("not_found", "note not found"))?
            .clone();
        self.require_team_member_or_owner(actor, &note.team_id)?;
        let by = self
            .context_notes
            .get(&by_note_id)
            .ok_or_else(|| Error::new("not_found", "replacement note not found"))?;
        if by.team_id != note.team_id || by_note_id == note_id {
            return Err(Error::new(
                "invalid_reference",
                "a note is replaced by another note of the same team",
            ));
        }
        let note = self
            .context_notes
            .get_mut(&note_id)
            .ok_or_else(|| Error::new("corrupt_state", "note disappeared"))?;
        note.superseded_by = Some(by_note_id.clone());
        note.revision = revision;
        let (team_id, result) = (note.team_id.clone(), serialize_value(&*note)?);
        let project_id = self.require_team(&team_id)?.project_id.clone();
        Ok(applied(
            result,
            Some(team_id),
            Some(project_id),
            "context_note_superseded",
            json!({ "note_id": note_id, "by": by_note_id }),
        ))
    }

    pub(super) fn accept_knowledge(
        &mut self,
        actor: &Actor,
        knowledge_id: String,
        revision: u64,
    ) -> Result<Applied> {
        super::require_role(actor, Role::Owner)?;
        self.set_knowledge_status(
            &knowledge_id,
            KnowledgeStatus::Accepted,
            None,
            revision,
            "knowledge_accepted",
        )
    }

    pub(super) fn supersede_knowledge(
        &mut self,
        actor: &Actor,
        knowledge_id: String,
        by_id: String,
        revision: u64,
    ) -> Result<Applied> {
        super::require_role(actor, Role::Owner)?;
        if !self.knowledge.contains_key(&by_id) || by_id == knowledge_id {
            return Err(Error::new(
                "invalid_reference",
                "an entry is superseded by a different, existing entry",
            ));
        }
        self.set_knowledge_status(
            &knowledge_id,
            KnowledgeStatus::Superseded,
            Some(by_id),
            revision,
            "knowledge_superseded",
        )
    }

    fn set_knowledge_status(
        &mut self,
        id: &str,
        status: KnowledgeStatus,
        by: Option<String>,
        revision: u64,
        kind: &'static str,
    ) -> Result<Applied> {
        let entry = self
            .knowledge
            .get_mut(id)
            .ok_or_else(|| Error::new("not_found", "knowledge entry not found"))?;
        if status == KnowledgeStatus::Accepted && entry.kind == KnowledgeKind::Note {
            return Err(Error::new(
                "invalid_state",
                "only a requirement or an ADR is accepted",
            ));
        }
        if entry.status == status {
            return Err(Error::new(
                "invalid_state",
                "the entry already has that status",
            ));
        }
        entry.status = status;
        entry.revision = revision;
        let (team_id, result) = (entry.team_id.clone(), serialize_value(&*entry)?);
        let project_id = self.require_team(&team_id)?.project_id.clone();
        Ok(applied(
            result,
            Some(team_id),
            Some(project_id),
            kind,
            json!({ "knowledge_id": id, "superseded_by": by }),
        ))
    }

    /// The bounded context an agent should start a task with. Items are added in priority order
    /// until the byte budget is spent; a whole item is either in or out, so exact evidence is never
    /// cut in the middle. What was left out is counted, not hidden.
    pub fn retrieve_memory(
        &self,
        actor: &Actor,
        team_id: Option<&str>,
        task_id: Option<&str>,
        budget: usize,
        now: u64,
    ) -> Result<Value> {
        if budget == 0 || budget > MAX_BUDGET {
            return Err(Error::new(
                "invalid_params",
                format!("budget_bytes must be between 1 and {MAX_BUDGET}"),
            ));
        }
        let team_id = match (actor.role, team_id) {
            (Role::Owner, Some(team)) => team.to_owned(),
            (Role::Owner, None) => {
                return Err(Error::new(
                    "invalid_params",
                    "name the team to build context for",
                ))
            }
            (_, requested) => {
                let own = actor
                    .team_id
                    .clone()
                    .ok_or_else(|| Error::new("forbidden", "actor has no team scope"))?;
                if requested.is_some_and(|r| r != own) {
                    return Err(Error::new("forbidden", "actor cannot read another team"));
                }
                own
            }
        };
        let team = self.require_team(&team_id)?;
        let specialist = actor.role == Role::Specialist;
        // A specialist's context is the tasks it was scoped to, and nothing else of the team's work.
        let scoped: Vec<&str> = self
            .recruitments
            .values()
            .filter(|r| {
                matches!(&r.status, super::governance::RecruitmentStatus::Active { actor_id, .. }
                    | super::governance::RecruitmentStatus::HandedOff { actor_id, .. } if actor_id == &actor.id)
            })
            .flat_map(|r| r.dependencies.iter().map(String::as_str))
            .collect();
        let task = match task_id {
            Some(id) => {
                let task = self
                    .tasks
                    .get(id)
                    .ok_or_else(|| Error::new("not_found", "task not found"))?;
                if task.team_id != team_id || (specialist && !scoped.contains(&id)) {
                    return Err(Error::new(
                        "forbidden",
                        "that task is not in this actor's scope",
                    ));
                }
                Some(task)
            }
            None if specialist => {
                return Err(Error::new(
                    "invalid_params",
                    "a specialist asks for the task it was scoped to",
                ))
            }
            None => None,
        };

        let mut sections: Vec<(&str, Vec<Value>)> = Vec::new();
        // 1. What the Owner accepted: requirements and ADRs, newest first.
        let mut accepted: Vec<&super::KnowledgeEntry> = self
            .knowledge
            .values()
            .filter(|k| {
                k.kind != KnowledgeKind::Note
                    && k.status == KnowledgeStatus::Accepted
                    && match &k.scope {
                        KnowledgeScope::Team { team_id: t } => t == &team_id,
                        KnowledgeScope::Project { project_id } => project_id == &team.project_id,
                    }
            })
            .collect();
        accepted.sort_by(|a, b| (b.revision, &b.id).cmp(&(a.revision, &a.id)));
        sections.push((
            "requirements",
            accepted
                .iter()
                .map(|k| json!({"id": k.id, "kind": k.kind, "title": k.title, "body": truncate(&k.body, 1200),
                    "provenance": {"kind": "knowledge", "id": k.id, "revision": k.revision, "accepted": true}}))
                .collect(),
        ));
        if let Some(task) = task {
            let deps: Vec<Value> = task
                .dependencies
                .iter()
                .filter_map(|d| self.tasks.get(d))
                .map(|d| json!({"id": d.id, "title": d.title, "status": tag_of(&d.status)}))
                .collect();
            sections.push(("task", vec![json!({
                "id": task.id, "title": task.title, "description": truncate(&task.description, 1500),
                "status": serde_json::to_value(&task.status).unwrap_or(Value::Null),
                "dependencies": deps,
                "provenance": {"kind": "task", "id": task.id, "revision": task.revision},
            })]));
            // 3. The newest handoff for the task, with its evidence exactly as submitted.
            let handoff = self
                .handoffs
                .values()
                .filter(|h| h.task_id == task.id)
                .max_by_key(|h| (h.revision, h.id.clone()));
            sections.push((
                "handoff",
                handoff
                    .map(|h| json!({"id": h.id, "summary": truncate(&h.summary, 1500), "evidence": h.evidence,
                        "provenance": {"kind": "handoff", "id": h.id, "revision": h.revision}}))
                    .into_iter()
                    .collect(),
            ));
            // 4. Review evidence: the decisions recorded on the task.
            let mut reviews: Vec<&super::Decision> = self
                .decisions
                .values()
                .filter(|d| d.task_id == task.id)
                .collect();
            reviews.sort_by(|a, b| (b.revision, &b.id).cmp(&(a.revision, &a.id)));
            sections.push((
                "reviews",
                reviews
                    .iter()
                    .map(|d| json!({"id": d.id, "kind": d.kind, "by": d.actor_id, "body": truncate(&d.body, 800),
                        "evidence": d.evidence, "provenance": {"kind": "decision", "id": d.id, "revision": d.revision}}))
                    .collect(),
            ));
            let disputes: Vec<Value> = self
                .disputes
                .values()
                .filter(|d| d.task_id.as_deref() == Some(task.id.as_str()) && !matches!(d.status, super::oversight::DisputeStatus::Resolved { .. }))
                .map(|d| json!({"id": d.id, "topic": d.topic, "status": serde_json::to_value(&d.status).unwrap_or(Value::Null),
                    "positions": d.positions.iter().map(|p| json!({"by": p.actor_id, "text": truncate(&p.text, 400)})).collect::<Vec<_>>(),
                    "provenance": {"kind": "dispute", "id": d.id, "revision": d.revision}}))
                .collect();
            sections.push(("disputes", disputes));
        }
        // 5. Summaries and role context that are still true, and a list of what was left out as stale.
        let wanted_role = if specialist {
            self.recruitments
                .values()
                .find(|r| matches!(&r.status, super::governance::RecruitmentStatus::Active { actor_id, .. } if actor_id == &actor.id))
                .map(|r| r.specialty.clone())
        } else {
            None
        };
        let mut notes: Vec<&ContextNote> = self
            .context_notes
            .values()
            .filter(|n| n.team_id == team_id)
            .collect();
        notes.sort_by(|a, b| (b.revision, &b.id).cmp(&(a.revision, &a.id)));
        let mut stale = Vec::new();
        let mut current = Vec::new();
        for note in notes {
            let relevant = match (&note.role_key, &wanted_role) {
                (None, _) => {
                    !specialist
                        || note
                            .sources
                            .iter()
                            .any(|s| s.kind == SourceKind::Task && scoped.contains(&s.id.as_str()))
                }
                (Some(role), Some(mine)) => role == mine,
                (Some(_), None) => !specialist,
            };
            if !relevant {
                continue;
            }
            match self.note_validity(note, now) {
                Validity::Current => current.push(json!({"id": note.id, "kind": note.kind, "role_key": note.role_key,
                    "summary": note.summary, "sources": note.sources,
                    "provenance": {"kind": "context_note", "id": note.id, "revision": note.revision, "author": note.author_id},
                    "valid": {"as_of_revision": note.as_of_revision, "until": note.valid_until}})),
                Validity::Stale(reason) => stale.push(json!({"id": note.id, "reason": reason})),
            }
        }
        sections.push(("notes", current));

        let mut used = 0usize;
        let mut out = serde_json::Map::new();
        let mut omitted = serde_json::Map::new();
        for (name, items) in sections {
            let total = items.len();
            let mut kept = Vec::new();
            for item in items {
                let size = serde_json::to_vec(&item).map_or(0, |b| b.len());
                if used + size <= budget {
                    used += size;
                    kept.push(item);
                }
            }
            if kept.len() < total {
                omitted.insert(name.to_owned(), json!(total - kept.len()));
            }
            out.insert(name.to_owned(), Value::Array(kept));
        }
        Ok(json!({
            "type": "teams_memory",
            "team_id": team_id,
            "task_id": task_id,
            "budget_bytes": budget,
            "used_bytes": used,
            "as_of_revision": self.revision,
            "generated_at": now,
            "valid_as_of_revision": self.revision,
            "context": out,
            "omitted_for_budget": omitted,
            "excluded_as_stale": stale,
            "never_included": "chat messages, scoped messages and transcripts",
            "note": "Refetch when the revision moves: this is a view of the record at as_of_revision, not a standing instruction.",
        }))
    }

    pub(super) fn validate_memory(&self) -> Result<()> {
        for note in self.context_notes.values() {
            if !self.teams.contains_key(&note.team_id) {
                return Err(Error::new(
                    "corrupt_state",
                    "a context note names an unknown team",
                ));
            }
        }
        Ok(())
    }
}

fn tag_of(status: &TaskStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.get("status").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default()
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

//! The team-draft form. It only holds what is being typed. Whether a draft is valid, what launching
//! would do, proposing one from a goal and creating the team are all done by the server
//! (`teams.drafts.*`), so any other client can do exactly the same.

use serde_json::{json, Value};

use super::data::Snapshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub name: String,
    /// Empty means "no project yet"; the server uses (or makes) the default one.
    pub project_id: String,
    pub lead: String,
    pub developer: String,
    pub qa: String,
    /// Where the first version came from, shown on the review screen.
    pub source: String,
    /// The three agent ids still follow the team name; editing one by hand stops that.
    pub auto_ids: bool,
}

/// What the server said about a draft (`teams.drafts.validate.v1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub valid: bool,
    pub problems: Vec<String>,
    pub team_id: String,
    pub credential_dir: String,
}

impl Preview {
    pub fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            valid: value.get("valid")?.as_bool()?,
            problems: value
                .get("problems")?
                .as_array()?
                .iter()
                .filter_map(|p| p.as_str().map(str::to_owned))
                .collect(),
            team_id: value.get("team_id")?.as_str()?.to_owned(),
            credential_dir: value.get("credential_dir")?.as_str()?.to_owned(),
        })
    }
}

/// Lowercase letters and digits joined by single dashes. Only used to pre-fill the id fields as the
/// name is typed; the server re-derives and checks everything.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(40).collect()
}

impl Draft {
    pub fn blank(snapshot: &Snapshot) -> Self {
        Self::named("", snapshot, "manual")
    }

    fn named(name: &str, snapshot: &Snapshot, source: &str) -> Self {
        let base = match slug(name) {
            s if s.is_empty() => "team".to_owned(),
            s => s,
        };
        let mut candidate = base.clone();
        let mut n = 1;
        while snapshot.team(&candidate).is_some()
            || snapshot.actor(&format!("{candidate}-lead")).is_some()
        {
            n += 1;
            candidate = format!("{base}-{n}");
        }
        Self {
            name: name.to_owned(),
            project_id: snapshot
                .projects
                .first()
                .map(|p| p.id.clone())
                .unwrap_or_default(),
            lead: format!("{candidate}-lead"),
            developer: format!("{candidate}-dev"),
            qa: format!("{candidate}-qa"),
            source: source.to_owned(),
            auto_ids: true,
        }
    }

    /// Renames the team; agent ids that were never hand-edited follow the new name.
    pub fn set_name(&mut self, name: &str, snapshot: &Snapshot) {
        if self.auto_ids {
            let renamed = Self::named(name, snapshot, &self.source);
            *self = Self {
                project_id: self.project_id.clone(),
                ..renamed
            };
        } else {
            self.name = name.to_owned();
        }
    }

    /// A draft from the server's proposal (`teams.drafts.propose.v1`).
    pub fn from_proposal(value: &Value) -> Option<Self> {
        let draft = value.get("draft")?;
        let text = |key: &str| draft.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(Self {
            name: text("name")?,
            project_id: text("project_id").unwrap_or_default(),
            lead: text("lead")?,
            developer: text("developer")?,
            qa: text("qa")?,
            source: value
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("proposal")
                .to_owned(),
            // A proposal's ids are its own; do not rewrite them under the user's feet.
            auto_ids: false,
        })
    }

    /// The `draft` parameter of the `teams.drafts.*` methods.
    pub fn to_params(&self) -> Value {
        json!({
            "name": self.name,
            "project_id": self.project_id,
            "lead": self.lead,
            "developer": self.developer,
            "qa": self.qa,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> Snapshot {
        serde_json::from_value(json!({
            "projects": [{"id": "moo", "name": "Moo"}],
            "teams": [{"id": "alpha", "name": "Alpha", "project_id": "moo"}],
            "actors": [{"id": "alpha-lead", "role": "lead", "team_id": "alpha"}]
        }))
        .unwrap()
    }

    #[test]
    fn the_form_prefills_ids_from_the_name_until_one_is_edited_by_hand() {
        let snap = snapshot();
        let mut draft = Draft::blank(&snap);
        assert_eq!(
            (draft.lead.as_str(), draft.project_id.as_str()),
            ("team-lead", "moo")
        );
        draft.set_name("Docs team", &snap);
        assert_eq!(
            (draft.lead.as_str(), draft.qa.as_str()),
            ("docs-team-lead", "docs-team-qa")
        );
        draft.lead = "my-lead".into();
        draft.auto_ids = false;
        draft.set_name("Renamed", &snap);
        assert_eq!(
            (draft.name.as_str(), draft.lead.as_str()),
            ("Renamed", "my-lead")
        );
        // A name that would collide with an existing team gets a free suffix.
        let mut clash = Draft::blank(&snap);
        clash.set_name("Alpha", &snap);
        assert_eq!(clash.lead, "alpha-2-lead");
    }

    #[test]
    fn the_params_are_exactly_what_the_server_method_takes() {
        let snap = snapshot();
        let mut draft = Draft::blank(&snap);
        draft.set_name("Docs team", &snap);
        let params = draft.to_params();
        let mut keys: Vec<&str> = params
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["developer", "lead", "name", "project_id", "qa"]);
    }

    #[test]
    fn a_proposal_and_a_preview_are_read_from_the_server_replies() {
        let proposal = json!({"source": "proposer", "draft": {"name": "N", "project_id": "", "lead": "a", "developer": "b", "qa": "c"}});
        let draft = Draft::from_proposal(&proposal).unwrap();
        assert_eq!(
            (draft.source.as_str(), draft.lead.as_str(), draft.auto_ids),
            ("proposer", "a", false)
        );
        assert!(Draft::from_proposal(&json!({"draft": {"name": "N"}})).is_none());
        let preview = Preview::from_value(&json!({"valid": false, "problems": ["bad"], "team_id": "n", "credential_dir": "teams-keys/n"})).unwrap();
        assert_eq!(
            (
                preview.valid,
                preview.problems.len(),
                preview.team_id.as_str()
            ),
            (false, 1, "n")
        );
        assert!(Preview::from_value(&json!({"valid": true})).is_none());
    }
}

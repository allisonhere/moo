//! Providers and models: the Owner declares which AI providers and models exist, which role prefers
//! which, and which one a team's role uses. This file only holds what is being typed; every change
//! is a Teams command (`declare_provider`, `set_role_preference`, `assign_role_provider`) that the
//! server checks, so any other client can do exactly the same.

use serde_json::{json, Value};

use super::data::Snapshot;

pub const ROLES: [&str; 4] = ["lead", "developer", "qa", "specialist"];
const METERING: [&str; 3] = ["estimated", "metered", "none"];
const NO_MODEL: &str = "(provider default)";
const DEFAULT_CAPABILITIES: &str = "code_edit, shell, review";

/// Agent CLIs Moo can start, with the model names they accept. Picking one in a form declares it
/// when it is not declared yet, so nobody types a provider that is already known. Models are
/// only a starting list: declare the provider yourself (`a`) to use others.
const KNOWN_PROVIDERS: &[(&str, &str, &[&str])] = &[
    ("claude", "Claude Code", &["opus", "sonnet", "haiku"]),
    ("codex", "Codex", &["gpt-5-codex", "gpt-5"]),
    (
        "gemini",
        "Gemini CLI",
        &["gemini-2.5-pro", "gemini-2.5-flash"],
    ),
    ("opencode", "opencode", &[]),
    ("amp", "Amp", &[]),
    ("cursor", "Cursor", &[]),
    ("droid", "Droid", &[]),
    ("kimi", "Kimi", &[]),
    ("qwen", "Qwen Code", &[]),
];

fn chosen_model(value: &str) -> Option<String> {
    (value != NO_MODEL && !value.is_empty()).then(|| value.to_owned())
}

fn known_provider(
    id: &str,
) -> Option<&'static (&'static str, &'static str, &'static [&'static str])> {
    KNOWN_PROVIDERS.iter().find(|(known, _, _)| *known == id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormKind {
    AddProvider,
    Assign,
    Prefer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    /// Typed text.
    Text,
    /// One of `options`, changed with left and right.
    Choice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub label: &'static str,
    pub kind: FieldKind,
    pub value: String,
    pub options: Vec<String>,
    pub hint: &'static str,
}

impl Field {
    fn text(label: &'static str, value: &str, hint: &'static str) -> Self {
        Self {
            label,
            kind: FieldKind::Text,
            value: value.to_owned(),
            options: Vec::new(),
            hint,
        }
    }

    fn choice(label: &'static str, options: Vec<String>, hint: &'static str) -> Self {
        Self {
            label,
            kind: FieldKind::Choice,
            value: options.first().cloned().unwrap_or_default(),
            options,
            hint,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub kind: FormKind,
    pub title: &'static str,
    pub fields: Vec<Field>,
    pub current: usize,
    pub error: Option<String>,
    /// The agent to start with the chosen provider once the form is saved.
    pub start: Option<String>,
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

impl Form {
    pub fn add_provider() -> Self {
        Self {
            kind: FormKind::AddProvider,
            title: "declare a provider",
            fields: vec![
                Field::text("id", "", "short name, for example claude or codex"),
                Field::text("label", "", "what you call it"),
                Field::text(
                    "can do",
                    DEFAULT_CAPABILITIES,
                    "what you say it can do, comma separated - Teams does not check",
                ),
                Field::text("models", "", "model names, comma separated"),
                Field::choice(
                    "usage",
                    METERING.iter().map(|m| (*m).to_owned()).collect(),
                    "metered = its own accounting, the only kind a limit is enforced on",
                ),
                Field::choice(
                    "spend cap",
                    vec!["no".into(), "yes".into()],
                    "yes only if the integration can really stop spending at a limit",
                ),
            ],
            current: 0,
            error: None,
            start: None,
        }
    }

    pub fn assign(snapshot: &Snapshot, team: Option<&str>) -> Self {
        let mut teams: Vec<String> = snapshot.teams.iter().map(|t| t.id.clone()).collect();
        if let Some(team) = team {
            if let Some(at) = teams.iter().position(|t| t == team) {
                teams.swap(0, at);
            }
        }
        let mut form = Self {
            kind: FormKind::Assign,
            title: "give a team's role a provider and model",
            fields: vec![
                Field::choice("team", teams, "left/right"),
                Field::choice(
                    "role",
                    ROLES[..3].iter().map(|r| (*r).to_owned()).collect(),
                    "left/right",
                ),
                Field::choice("provider", provider_ids(snapshot), "left/right"),
                Field::choice("model", Vec::new(), "left/right"),
            ],
            current: 0,
            error: None,
            start: None,
        };
        form.refresh(snapshot);
        form
    }

    /// Pick the agent for one role and start it: the assign form for that agent's team and role.
    pub fn start(snapshot: &Snapshot, team: &str, role: &str, actor_id: &str) -> Self {
        let mut form = Self::assign(snapshot, Some(team));
        form.title = "pick the agent for this role";
        form.start = Some(actor_id.to_owned());
        if let Some(field) = form.fields.iter_mut().find(|f| f.label == "role") {
            if field.options.iter().any(|o| o == role) {
                field.value = role.to_owned();
            }
        }
        form.current = 2;
        form.refresh(snapshot);
        form
    }

    pub fn prefer(snapshot: &Snapshot) -> Self {
        let mut form = Self {
            kind: FormKind::Prefer,
            title: "prefer a provider and model for a role",
            fields: vec![
                Field::choice(
                    "role",
                    ROLES.iter().map(|r| (*r).to_owned()).collect(),
                    "used when a plan picks providers",
                ),
                Field::choice("provider", provider_ids(snapshot), "left/right"),
                Field::choice("model", Vec::new(), "left/right"),
            ],
            current: 0,
            error: None,
            start: None,
        };
        form.refresh(snapshot);
        form
    }

    /// Models depend on the provider chosen, so they are recomputed after every change.
    pub fn refresh(&mut self, snapshot: &Snapshot) {
        let provider_at = match self.kind {
            FormKind::Assign => 2,
            FormKind::Prefer => 1,
            FormKind::AddProvider => return,
        };
        let provider = self.fields[provider_at].value.clone();
        let mut models = vec![NO_MODEL.to_owned()];
        if let Some(info) = snapshot.providers.iter().find(|p| p.id == provider) {
            models.extend(info.models.iter().map(|m| m.id.clone()));
        } else if let Some((_, _, known)) = known_provider(&provider) {
            models.extend(known.iter().map(|m| (*m).to_owned()));
        }
        let model = &mut self.fields[provider_at + 1];
        if !models.contains(&model.value) {
            model.value = models[0].clone();
        }
        model.options = models;
    }

    pub fn next(&mut self) {
        self.current = (self.current + 1) % self.fields.len();
    }

    pub fn previous(&mut self) {
        self.current = (self.current + self.fields.len() - 1) % self.fields.len();
    }

    pub fn is_last(&self) -> bool {
        self.current + 1 == self.fields.len()
    }

    pub fn type_char(&mut self, c: char) {
        let field = &mut self.fields[self.current];
        if field.kind == FieldKind::Text && field.value.len() < 200 {
            field.value.push(c);
            self.error = None;
        }
    }

    pub fn backspace(&mut self) {
        let field = &mut self.fields[self.current];
        if field.kind == FieldKind::Text {
            field.value.pop();
            self.error = None;
        }
    }

    pub fn step_choice(&mut self, forward: bool, snapshot: &Snapshot) {
        let field = &mut self.fields[self.current];
        if field.kind != FieldKind::Choice || field.options.is_empty() {
            return;
        }
        let count = field.options.len();
        let at = field
            .options
            .iter()
            .position(|o| *o == field.value)
            .unwrap_or(0);
        field.value = field.options[(at + if forward { 1 } else { count - 1 }) % count].clone();
        self.error = None;
        self.refresh(snapshot);
    }

    /// The commands this form sends: choosing a known provider that is not declared yet declares it
    /// first, so the role can be given it in one step.
    pub fn commands(&self, snapshot: &Snapshot) -> Result<Vec<Value>, String> {
        let command = self.command(snapshot)?;
        let provider = self.value("provider");
        let undeclared = self.kind != FormKind::AddProvider
            && !snapshot.providers.iter().any(|p| p.id == provider);
        let Some((id, label, models)) = known_provider(provider).filter(|_| undeclared) else {
            return Ok(vec![command]);
        };
        let declare = json!({
            "kind": "declare_provider",
            "provider_id": id,
            "label": label,
            "capabilities": words(DEFAULT_CAPABILITIES),
            "models": models.iter().map(|m| json!({ "id": m })).collect::<Vec<_>>(),
            "metering": "estimated",
            "hard_spend_limit": false,
        });
        Ok(vec![declare, command])
    }

    /// The provider chosen in the form.
    pub fn provider(&self) -> &str {
        self.value("provider")
    }

    /// The model chosen in the form, or none for the provider's default.
    pub fn model(&self) -> Option<String> {
        chosen_model(self.value("model"))
    }

    fn value(&self, label: &str) -> &str {
        self.fields
            .iter()
            .find(|f| f.label == label)
            .map_or("", |f| f.value.as_str())
    }

    /// The command this form sends, or why it cannot be sent yet.
    pub fn command(&self, snapshot: &Snapshot) -> Result<Value, String> {
        let model = chosen_model;
        match self.kind {
            FormKind::AddProvider => {
                let id = self.value("id").trim().to_owned();
                if !valid_id(&id) {
                    return Err("the id needs letters, digits, - _ or . (at most 64)".into());
                }
                if snapshot.providers.iter().any(|p| p.id == id) {
                    return Err(format!("{id} is already declared"));
                }
                let label = match self.value("label").trim() {
                    "" => id.clone(),
                    label => label.to_owned(),
                };
                let capabilities = words(self.value("can do"));
                if capabilities.is_empty() {
                    return Err("say what it can do, for example code_edit, shell".into());
                }
                let models: Vec<Value> = words(self.value("models"))
                    .into_iter()
                    .map(|id| json!({ "id": id }))
                    .collect();
                Ok(json!({
                    "kind": "declare_provider",
                    "provider_id": id,
                    "label": label,
                    "capabilities": capabilities,
                    "models": models,
                    "metering": self.value("usage"),
                    "hard_spend_limit": self.value("spend cap") == "yes",
                }))
            }
            FormKind::Assign => {
                let provider = self.value("provider");
                if provider.is_empty() {
                    return Err("declare a provider first (press a)".into());
                }
                if self.value("team").is_empty() {
                    return Err("there is no team yet".into());
                }
                Ok(json!({
                    "kind": "assign_role_provider",
                    "team_id": self.value("team"),
                    "role_key": self.value("role"),
                    "provider_id": provider,
                    "model_id": model(self.value("model")),
                }))
            }
            FormKind::Prefer => {
                let provider = self.value("provider");
                if provider.is_empty() {
                    return Err("declare a provider first (press a)".into());
                }
                Ok(json!({
                    "kind": "set_role_preference",
                    "role_key": self.value("role"),
                    "provider_id": provider,
                    "model_id": model(self.value("model")),
                }))
            }
        }
    }
}

/// Declared providers first, then the known ones that are not declared yet.
fn provider_ids(snapshot: &Snapshot) -> Vec<String> {
    let mut ids: Vec<String> = snapshot.providers.iter().map(|p| p.id.clone()).collect();
    for (known, _, _) in KNOWN_PROVIDERS {
        if !ids.iter().any(|id| id == known) {
            ids.push((*known).to_owned());
        }
    }
    ids
}

/// The providers screen: the list behind the forms.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Providers {
    pub cursor: usize,
    pub form: Option<Form>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> Snapshot {
        serde_json::from_value(json!({
            "teams": [{"id": "alpha"}, {"id": "beta"}],
            "providers": [
                {"id": "claude", "label": "Claude Code", "capabilities": ["code_edit"],
                 "models": [{"id": "opus"}, {"id": "sonnet"}], "metering": "estimated"},
                {"id": "codex", "models": [{"id": "gpt"}], "metering": "metered"}
            ]
        }))
        .unwrap()
    }

    #[test]
    fn adding_a_provider_builds_exactly_the_declare_command_and_checks_the_id() {
        let snap = snapshot();
        let mut form = Form::add_provider();
        assert!(form.command(&snap).is_err(), "an id is required");
        form.fields[0].value = "local-llm".into();
        form.fields[3].value = "qwen, llama".into();
        form.fields[4].value = "none".into();
        form.fields[5].value = "yes".into();
        let command = form.command(&snap).unwrap();
        assert_eq!(command["kind"], "declare_provider");
        assert_eq!(command["label"], "local-llm");
        assert_eq!(
            command["capabilities"],
            json!(["code_edit", "shell", "review"])
        );
        assert_eq!(command["models"], json!([{"id": "qwen"}, {"id": "llama"}]));
        assert_eq!(
            (
                command["metering"].as_str(),
                command["hard_spend_limit"].as_bool()
            ),
            (Some("none"), Some(true))
        );
        form.fields[0].value = "claude".into();
        assert!(form.command(&snap).unwrap_err().contains("already"));
        form.fields[0].value = "bad id!".into();
        assert!(form.command(&snap).is_err());
    }

    #[test]
    fn models_follow_the_chosen_provider_and_default_means_no_model() {
        let snap = snapshot();
        let mut form = Form::assign(&snap, Some("beta"));
        assert_eq!(
            form.fields[0].value, "beta",
            "the selected team comes first"
        );
        assert_eq!(
            form.fields[3].options,
            ["(provider default)", "opus", "sonnet"]
        );
        form.current = 3;
        form.step_choice(true, &snap);
        assert_eq!(form.fields[3].value, "opus");
        form.current = 2;
        form.step_choice(true, &snap);
        assert_eq!(form.fields[2].value, "codex");
        assert_eq!(form.fields[3].options, ["(provider default)", "gpt"]);
        assert_eq!(
            form.fields[3].value, "(provider default)",
            "a model of another provider is dropped"
        );
        let command = form.command(&snap).unwrap();
        assert_eq!(command["kind"], "assign_role_provider");
        assert_eq!(
            (command["team_id"].as_str(), command["provider_id"].as_str()),
            (Some("beta"), Some("codex"))
        );
        assert!(command["model_id"].is_null());
    }

    #[test]
    fn a_known_provider_is_listed_with_its_models_and_declared_when_chosen() {
        let empty: Snapshot = serde_json::from_value(json!({"teams": [{"id": "alpha"}]})).unwrap();
        let mut form = Form::assign(&empty, None);
        assert_eq!(form.fields[2].value, "claude");
        assert_eq!(
            form.fields[3].options,
            ["(provider default)", "opus", "sonnet", "haiku"]
        );
        form.current = 3;
        form.step_choice(true, &empty);
        let commands = form.commands(&empty).unwrap();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0]["kind"], "declare_provider");
        assert_eq!(commands[0]["provider_id"], "claude");
        assert_eq!(commands[1]["kind"], "assign_role_provider");
        assert_eq!(commands[1]["model_id"], "opus");

        // Once declared it is sent alone.
        let declared = snapshot();
        assert_eq!(
            Form::assign(&declared, None)
                .commands(&declared)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn a_preference_needs_a_declared_provider() {
        let snap = snapshot();
        let mut form = Form::prefer(&snap);
        form.current = 2;
        form.step_choice(true, &snap);
        let command = form.command(&snap).unwrap();
        assert_eq!(command["kind"], "set_role_preference");
        assert_eq!(
            (command["role_key"].as_str(), command["model_id"].as_str()),
            (Some("lead"), Some("opus"))
        );
    }
}

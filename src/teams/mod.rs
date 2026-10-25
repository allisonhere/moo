//! Local, server-owned collaboration. This namespace is independent of the TUI wire codecs.
mod audit;
mod catalog;
mod drafts;
mod error;
mod limits;
mod model;
mod pager;
mod params;
mod store;

pub(crate) use error::Error;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(crate) type Result<T> = std::result::Result<T, Error>;

pub(crate) struct Service {
    root: PathBuf,
    store: Mutex<Option<store::Store>>,
    limiter: Mutex<limits::RateLimiter>,
    waiters: Mutex<limits::Waiters>,
    /// The newest committed event sequence, so a waiting reader wakes the moment there is news.
    watermark: (Mutex<u64>, Condvar),
    audit: Mutex<audit::DenialLog>,
    started: Instant,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    id: String,
    method: String,
    #[serde(default)]
    auth: Option<Auth>,
    #[serde(default)]
    params: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Auth {
    bearer: String,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn proposer_command() -> Option<String> {
    std::env::var(drafts::PROPOSER_ENV)
        .ok()
        .filter(|c| !c.trim().is_empty())
}

impl Service {
    pub(crate) fn new(root: PathBuf) -> Self {
        let audit = audit::DenialLog::new(&root);
        Self {
            root,
            store: Mutex::new(None),
            limiter: Mutex::new(limits::RateLimiter::default()),
            waiters: Mutex::new(limits::Waiters::default()),
            watermark: (Mutex::new(0), Condvar::new()),
            audit: Mutex::new(audit),
            started: Instant::now(),
        }
    }

    /// Non-Teams requests continue through the unchanged core parser and dispatch.
    pub(crate) fn route(&self, line: &str) -> Option<Value> {
        let peek: Value = serde_json::from_str(line).ok()?;
        let method = peek.get("method")?.as_str()?.to_owned();
        if !method.starts_with("teams.") {
            return None;
        }
        let id = peek
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let mut who = "-".to_owned();
        let response = match serde_json::from_str::<Envelope>(line) {
            Ok(envelope) if !envelope.id.is_empty() && envelope.id.len() <= 128 => {
                self.handle(envelope, &mut who)
            }
            _ => Err(Error::new(
                "invalid_request",
                "invalid Teams request envelope",
            )),
        };
        Some(match response {
            Ok(result) => json!({"id": id, "result": result}),
            Err(error) => {
                if let Ok(mut log) = self.audit.lock() {
                    log.record(&who, &method, error.code, unix_now());
                }
                json!({"id": id, "error": error.to_value()})
            }
        })
    }

    fn mono(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    fn throttle(&self, who: &str, class: limits::Class) -> Result<()> {
        self.limiter
            .lock()
            .map_err(|_| Error::new("store_unavailable", "Teams limiter is unavailable"))?
            .take(who, class, self.mono())
    }

    /// Opens the store on first use, recovering any in-flight work a crash left behind.
    fn lock_store(&self) -> Result<MutexGuard<'_, Option<store::Store>>> {
        let mut guard = self
            .store
            .lock()
            .map_err(|_| Error::new("store_unavailable", "Teams store lock is unavailable"))?;
        if guard.is_none() {
            let mut store = store::Store::open(&self.root)?;
            store.recover()?;
            self.publish(store.state()?.next_sequence.saturating_sub(1));
            *guard = Some(store);
        }
        Ok(guard)
    }

    /// Tells waiting readers that events up to `sequence` are durable.
    fn publish(&self, sequence: u64) {
        if let Ok(mut current) = self.watermark.0.lock() {
            if sequence > *current {
                *current = sequence;
            }
            self.watermark.1.notify_all();
        }
    }

    fn handle(&self, envelope: Envelope, who: &mut String) -> Result<Value> {
        let info = catalog::method(&envelope.method)
            .ok_or_else(|| Error::new("unknown_method", "unsupported Teams method"))?;
        match envelope.method.as_str() {
            "teams.capabilities.v1" => {
                return Ok(catalog::capabilities(proposer_command().is_some()));
            }
            "teams.schema.v1" => {
                return Ok(json!({"type": "teams_schema", "schema": catalog::schema_document()}));
            }
            _ => {}
        }
        ensure_supported()?;
        // Failed authentications share one small bucket, so guessing a credential is slow and cannot
        // exhaust anyone else's allowance.
        let unauthorized = |this: &Self, message: &str| -> Error {
            match this.throttle("", limits::Class::Read) {
                Err(limited) => limited,
                Ok(()) => Error::new("unauthorized", message.to_owned()),
            }
        };
        let Some(auth) = envelope.auth else {
            return Err(unauthorized(self, "Teams credential required"));
        };
        if validate_token(&auth.bearer).is_err() {
            return Err(unauthorized(self, "invalid Teams credential"));
        }
        let digest = credential_digest(&auth.bearer);
        let actor = {
            let guard = self.lock_store()?;
            let store = guard
                .as_ref()
                .ok_or_else(|| Error::new("store_unavailable", "Teams store is unavailable"))?;
            match store.state()?.authenticate(&digest) {
                Ok(actor) => actor,
                Err(_) => {
                    drop(guard);
                    return Err(unauthorized(self, "invalid or revoked credential"));
                }
            }
        };
        *who = actor.id.clone();
        self.throttle(
            &actor.id,
            if info.class == catalog::Class::Write {
                limits::Class::Write
            } else {
                limits::Class::Read
            },
        )?;
        if info.owner_only && actor.role != model::Role::Owner {
            return Err(Error::new("forbidden", "this method is for the Owner"));
        }
        if envelope.method == "teams.events.wait.v1" {
            // Waiting must not hold the store: writers keep committing while readers sleep.
            return self.wait(&actor, decode(envelope.params)?);
        }
        let mut guard = self.lock_store()?;
        let store = guard
            .as_mut()
            .ok_or_else(|| Error::new("store_unavailable", "Teams store is unavailable"))?;
        self.dispatch(store, &actor, &envelope.method, envelope.params)
    }

    fn dispatch(
        &self,
        store: &mut store::Store,
        actor: &model::Actor,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        match method {
            "teams.snapshot.get.v1" => {
                let params: params::SnapshotParams = decode(params)?;
                store.state()?.snapshot(actor, params.team_id.as_deref())
            }
            "teams.list.v1" => store.state()?.list_teams(actor),
            "teams.get.v1" => {
                let params: params::TeamRef = decode(params)?;
                store.state()?.snapshot(actor, Some(&params.team_id))
            }
            "teams.conflicts.list.v1" | "teams.integrations.list.v1" => {
                let params: params::SnapshotParams = decode(params)?;
                let view = store.state()?.snapshot(actor, params.team_id.as_deref())?;
                let keys: &[&str] = if method == "teams.conflicts.list.v1" {
                    &["conflicts", "notices"]
                } else {
                    &["integrations", "worktrees", "conflicts", "notices"]
                };
                Ok(json!({
                    "revision": view["revision"],
                    "stream_id": view["stream_id"],
                    "last_event_sequence": view["last_event_sequence"],
                    "data": keys.iter().map(|key| (key.to_string(), view[*key].clone())).collect::<serde_json::Map<_, _>>(),
                }))
            }
            "teams.events.list.v1" | "teams.events.resume.v1" => {
                let params: params::EventsParams = decode(params)?;
                events_page(
                    store.state()?,
                    actor,
                    &params.stream_id,
                    params.after,
                    params.limit,
                )
            }
            "teams.messages.list.v1" => self.list(
                store,
                actor,
                params,
                "messages",
                pager::Order::Chronological,
            ),
            "teams.decisions.list.v1" => self.list(
                store,
                actor,
                params,
                "decisions",
                pager::Order::Chronological,
            ),
            "teams.tasks.list.v1" => self.list(store, actor, params, "tasks", pager::Order::ById),
            "teams.approvals.list.v1" => {
                self.list(store, actor, params, "approvals", pager::Order::ById)
            }
            "teams.audit.list.v1" => {
                let params: params::AuditParams = decode(params)?;
                let limit = params.limit.unwrap_or(50).clamp(1, 200);
                let denials = self
                    .audit
                    .lock()
                    .map(|log| log.recent(limit))
                    .unwrap_or_default();
                Ok(json!({"type": "teams_audit", "denials": denials,
                    "stream_id": store.state()?.stream_id,
                    "watermark": store.state()?.next_sequence.saturating_sub(1)}))
            }
            "teams.drafts.propose.v1" => {
                let params: params::ProposeParams = decode(params)?;
                let proposal =
                    drafts::propose(store.state()?, &params.goal, proposer_command().as_deref())?;
                Ok(
                    json!({"type": "teams_draft_proposal", "source": proposal.source, "draft": proposal.draft}),
                )
            }
            "teams.drafts.validate.v1" => {
                let params: params::DraftParams = decode(params)?;
                Ok(drafts::preview(store.state()?, &params.draft))
            }
            "teams.plans.launch.v1" => {
                let params: params::PlanLaunchParams = decode(params)?;
                self.launch_plan(store, actor, params)
            }
            "teams.memory.retrieve.v1" => {
                let params: params::MemoryParams = decode(params)?;
                store.state()?.retrieve_memory(
                    actor,
                    params.team_id.as_deref(),
                    params.task_id.as_deref(),
                    params.budget_bytes.unwrap_or(model::DEFAULT_BUDGET),
                    unix_now(),
                )
            }
            "teams.summary.v1" => {
                let params: params::SummaryParams = decode(params)?;
                store.state()?.summary(
                    actor,
                    unix_now(),
                    params
                        .stuck_after_seconds
                        .unwrap_or(model::DEFAULT_STUCK_SECONDS)
                        .max(1),
                )
            }
            "teams.plans.get.v1" => {
                let params: params::PlanRef = decode(params)?;
                let plan = store
                    .state()?
                    .plans
                    .get(&params.plan_id)
                    .ok_or_else(|| Error::new("not_found", "plan not found"))?;
                Ok(json!({"type": "teams_plan", "plan": plan}))
            }
            "teams.plans.list.v1" => {
                let plans: Vec<Value> = store
                    .state()?
                    .plans
                    .values()
                    .map(|p| json!({"id": p.id, "goal": p.goal, "status": p.status, "revision": p.revision}))
                    .collect();
                Ok(json!({"type": "teams_plans", "plans": plans}))
            }
            "teams.drafts.launch.v1" => {
                let params: params::DraftLaunchParams = decode(params)?;
                self.launch_draft(store, actor, params)
            }
            "teams.mutate.v1" | "teams.validate.v1" => {
                let params: params::MutationParams = decode(params)?;
                self.mutate(store, actor, params, method == "teams.validate.v1")
            }
            _ => Err(Error::new("unknown_method", "unsupported Teams method")),
        }
    }

    fn list(
        &self,
        store: &store::Store,
        actor: &model::Actor,
        params: Value,
        kind: &str,
        order: pager::Order,
    ) -> Result<Value> {
        let params: pager::ListParams = decode(params)?;
        let state = store.state()?;
        let view = state.snapshot(actor, params.team_id.as_deref())?;
        let items: Vec<Value> = view[kind]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| {
                params
                    .task_id
                    .as_ref()
                    .is_none_or(|task| item["task_id"].as_str() == Some(task.as_str()))
                    && params.status.as_ref().is_none_or(|status| {
                        item["status"]["status"].as_str() == Some(status.as_str())
                    })
            })
            .collect();
        let mut page = pager::page(&items, order, &params)?;
        if let Some(object) = page.as_object_mut() {
            object.insert("type".into(), json!("teams_page"));
            object.insert("kind".into(), json!(kind));
            object.insert("revision".into(), view["revision"].clone());
            object.insert("stream_id".into(), view["stream_id"].clone());
            object.insert("watermark".into(), view["last_event_sequence"].clone());
        }
        Ok(page)
    }

    /// Holds the request open until the actor has events past `after`, or the wait runs out.
    fn wait(&self, actor: &model::Actor, params: params::WaitParams) -> Result<Value> {
        let page = |this: &Self| -> Result<Value> {
            let guard = this.lock_store()?;
            let store = guard
                .as_ref()
                .ok_or_else(|| Error::new("store_unavailable", "Teams store is unavailable"))?;
            events_page(
                store.state()?,
                actor,
                &params.stream_id,
                params.after,
                params.limit,
            )
        };
        let first = page(self)?;
        let ready = |value: &Value| {
            value["events"]
                .as_array()
                .is_some_and(|events| !events.is_empty())
                || value["has_more"].as_bool() == Some(true)
        };
        let timeout = Duration::from_millis(params.timeout_ms.min(limits::MAX_WAIT_MS));
        if timeout.is_zero() || ready(&first) {
            return Ok(first);
        }
        self.waiters
            .lock()
            .map_err(|_| Error::new("store_unavailable", "Teams waiters are unavailable"))?
            .enter(&actor.id)?;
        struct Leave<'a>(&'a Service, &'a str);
        impl Drop for Leave<'_> {
            fn drop(&mut self) {
                if let Ok(mut waiters) = self.0.waiters.lock() {
                    waiters.leave(self.1);
                }
            }
        }
        let _leave = Leave(self, &actor.id);
        let deadline = Instant::now() + timeout;
        let mut seen = first["watermark"].as_u64().unwrap_or(0);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return page(self);
            }
            {
                let current = self.watermark.0.lock().map_err(|_| {
                    Error::new("store_unavailable", "Teams watermark is unavailable")
                })?;
                let (current, _) = self
                    .watermark
                    .1
                    .wait_timeout_while(current, remaining, |sequence| *sequence <= seen)
                    .map_err(|_| {
                        Error::new("store_unavailable", "Teams watermark is unavailable")
                    })?;
                seen = *current;
            }
            let latest = page(self)?;
            // Events the actor may not read move the watermark too; keep waiting for ones they may.
            if ready(&latest) {
                return Ok(latest);
            }
        }
    }

    fn mutate(
        &self,
        store: &mut store::Store,
        actor: &model::Actor,
        params: params::MutationParams,
        dry_run: bool,
    ) -> Result<Value> {
        if params.idempotency_key.is_empty() || params.idempotency_key.len() > 128 {
            return Err(Error::new(
                "invalid_params",
                "idempotency key must contain 1 to 128 bytes",
            ));
        }
        let command_bytes = serde_json::to_vec(&params.command)
            .map_err(|_| Error::new("invalid_params", "command cannot be encoded"))?;
        let command_digest = format!("{:x}", Sha256::digest(&command_bytes));
        let key = serde_json::to_string(&(actor.id.as_str(), params.idempotency_key.as_str()))
            .map_err(|_| Error::new("invalid_params", "request key cannot be encoded"))?;
        if !dry_run {
            if let Some(receipt) = store.state()?.receipts.get(&key) {
                if receipt.digest != command_digest {
                    return Err(Error::new(
                        "idempotency_conflict",
                        "request key was already used for a different command",
                    ));
                }
                return Ok(receipt.result.clone());
            }
        }
        if params.expected_revision != store.state()?.revision {
            return Err(Error::new(
                "revision_conflict",
                "read a fresh snapshot before retrying with a new request key",
            ));
        }
        let mut candidate = store.state()?.clone();
        let outcome = candidate.execute_at(actor, params.command, unix_now())?;
        let result = json!({"type": "teams_mutation", "revision": candidate.revision,
            "stream_id": candidate.stream_id, "watermark": candidate.next_sequence.saturating_sub(1),
            "outcome": outcome});
        if dry_run {
            candidate.validate()?;
            return Ok(json!({"type": "teams_validation", "valid": true, "preview": result}));
        }
        candidate.receipts.insert(
            key,
            model::StoredReceipt {
                digest: command_digest,
                result: result.clone(),
                revision: candidate.revision,
            },
        );
        candidate.trim_receipts();
        candidate.validate()?;
        let watermark = candidate.next_sequence.saturating_sub(1);
        store.commit(candidate)?;
        self.publish(watermark);
        Ok(result)
    }

    /// Launches a reviewed plan: the team, its tasks with their dependencies, and the role
    /// assignments, in one all-or-nothing commit. Specialists are listed, not started: the Lead
    /// requests them when the task they follow is done.
    fn launch_plan(
        &self,
        store: &mut store::Store,
        actor: &model::Actor,
        params: params::PlanLaunchParams,
    ) -> Result<Value> {
        if params.idempotency_key.is_empty() || params.idempotency_key.len() > 128 {
            return Err(Error::new(
                "invalid_params",
                "idempotency key must contain 1 to 128 bytes",
            ));
        }
        let command_digest = format!(
            "{:x}",
            Sha256::digest(format!("plan-launch\0{}", params.plan_id).as_bytes())
        );
        let key = serde_json::to_string(&(actor.id.as_str(), params.idempotency_key.as_str()))
            .map_err(|_| Error::new("invalid_params", "request key cannot be encoded"))?;
        if let Some(receipt) = store.state()?.receipts.get(&key) {
            if receipt.digest != command_digest {
                return Err(Error::new(
                    "idempotency_conflict",
                    "request key was already used for a different command",
                ));
            }
            return Ok(receipt.result.clone());
        }
        if params.expected_revision != store.state()?.revision {
            return Err(Error::new(
                "revision_conflict",
                "read a fresh snapshot before retrying with a new request key",
            ));
        }
        let state = store.state()?;
        let plan = state
            .plans
            .get(&params.plan_id)
            .cloned()
            .ok_or_else(|| Error::new("not_found", "plan not found"))?;
        if plan.status != model::PlanStatus::Draft {
            return Err(Error::new(
                "invalid_state",
                "only a draft plan can be launched",
            ));
        }
        let draft = drafts::DraftInput {
            name: plan.draft.name.clone(),
            project_id: plan.draft.project_id.clone(),
            lead: plan.draft.lead.clone(),
            developer: plan.draft.developer.clone(),
            qa: plan.draft.qa.clone(),
        };
        let problems = drafts::problems(state, &draft);
        if !problems.is_empty() {
            return Err(Error::new("draft_invalid", problems[0].clone())
                .with_details(json!({ "problems": problems })));
        }
        let team_id = drafts::team_id(state, &draft);
        let dir = self.root.join(drafts::KEYS_DIR).join(&team_id);
        make_private_dir(&dir)?;
        let mut steps = 1;
        let outcome = (|| -> Result<(Value, model::State)> {
            let mut digests = Vec::new();
            for role in ["lead", "developer", "qa"] {
                digests.push(create_credential(&dir.join(format!("{role}.key")))?);
            }
            let digests: [String; 3] = digests
                .try_into()
                .map_err(|_| Error::new("corrupt_state", "credential count"))?;
            let now = unix_now();
            let mut candidate = state.clone();
            let team_commands = drafts::plan(state, &draft, &digests);
            steps = team_commands.len() as u64;
            drafts::apply(&mut candidate, actor, team_commands, now)?;
            // The Lead creates the tasks, as the Lead would.
            let lead = candidate.authenticate(&digests[0])?;
            let kept: Vec<&model::PlanTaskView> =
                plan.tasks.iter().filter(|t| !t.removed).collect();
            let id_of = |key: &str| format!("{team_id}-{key}");
            let mut created: Vec<String> = Vec::new();
            let mut remaining: Vec<&model::PlanTaskView> = kept.clone();
            while !remaining.is_empty() {
                let before = remaining.len();
                let mut next = Vec::new();
                for task in remaining {
                    if task.depends_on.iter().all(|d| created.contains(d)) {
                        let digest = format!(
                            "{:x}",
                            Sha256::digest(
                                format!("{}\0{}", task.title, task.description).as_bytes()
                            )
                        );
                        candidate.execute_at(
                            &lead,
                            model::Command::CreateTask {
                                team_id: team_id.clone(),
                                task_id: id_of(&task.key),
                                title: task.title.clone(),
                                description: task.description.clone(),
                                developer_id: draft.developer.clone(),
                                dependencies: task.depends_on.iter().map(|d| id_of(d)).collect(),
                                assignment_message_id: format!("{}-assign", id_of(&task.key)),
                                dispatch: model::DispatchIntent {
                                    attempt_id: format!("{}-attempt-1", id_of(&task.key)),
                                    run_generation: 1,
                                    content_digest: digest,
                                },
                            },
                            now,
                        )?;
                        steps += 1;
                        created.push(task.key.clone());
                    } else {
                        next.push(task);
                    }
                }
                if next.len() == before {
                    return Err(Error::new(
                        "dependency_cycle",
                        "the plan's tasks do not form a valid graph",
                    ));
                }
                remaining = next;
            }
            for role in plan.roles.iter().filter(|r| r.provider_id.is_some()) {
                candidate.execute_at(
                    actor,
                    model::Command::AssignRoleProvider {
                        team_id: team_id.clone(),
                        role_key: role.role_key.clone(),
                        provider_id: role.provider_id.clone().unwrap_or_default(),
                        model_id: role.model_id.clone(),
                    },
                    now,
                )?;
                steps += 1;
            }
            candidate.execute_at(
                actor,
                model::Command::MarkPlanLaunched {
                    plan_id: plan.id.clone(),
                    team_id: team_id.clone(),
                },
                now,
            )?;
            steps += 1;
            let pending: Vec<Value> = plan
                .recruitments
                .iter()
                .filter(|r| !r.dropped)
                .map(|r| {
                    json!({
                        "key": r.key, "specialty": r.specialty, "deliverable": r.deliverable,
                        "request_after_task": r.after_task.as_deref().map(id_of),
                        "capabilities": r.capabilities, "because": r.because,
                        "provider_id": r.provider_id, "model_id": r.model_id,
                    })
                })
                .collect();
            let result = json!({
                "type": "teams_plan_launched",
                "plan_id": plan.id, "team_id": team_id,
                "task_ids": created.iter().map(|k| (k.clone(), json!(id_of(k)))).collect::<serde_json::Map<_, _>>(),
                "pending_specialists": pending,
                "credential_files": {
                    "lead": dir.join("lead.key"), "developer": dir.join("developer.key"), "qa": dir.join("qa.key"),
                },
                "revision": candidate.revision, "stream_id": candidate.stream_id,
                "watermark": candidate.next_sequence.saturating_sub(1),
            });
            candidate.receipts.insert(
                key,
                model::StoredReceipt {
                    digest: command_digest,
                    result: result.clone(),
                    revision: candidate.revision,
                },
            );
            candidate.trim_receipts();
            candidate.validate()?;
            Ok((result, candidate))
        })();
        let (result, candidate) = match outcome {
            Ok(done) => done,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(error);
            }
        };
        let watermark = candidate.next_sequence.saturating_sub(1);
        if let Err(error) = store.commit_steps(candidate, steps) {
            if error.code != "commit_unknown" {
                let _ = std::fs::remove_dir_all(&dir);
            }
            return Err(error);
        }
        self.publish(watermark);
        Ok(result)
    }

    /// Creates a team from a draft in one step: credentials, spec, enrollments and launch commit
    /// together or not at all.
    fn launch_draft(
        &self,
        store: &mut store::Store,
        actor: &model::Actor,
        params: params::DraftLaunchParams,
    ) -> Result<Value> {
        if params.idempotency_key.is_empty() || params.idempotency_key.len() > 128 {
            return Err(Error::new(
                "invalid_params",
                "idempotency key must contain 1 to 128 bytes",
            ));
        }
        let digest_input = serde_json::to_vec(&params.draft)
            .map_err(|_| Error::new("invalid_params", "draft cannot be encoded"))?;
        let command_digest = format!(
            "{:x}",
            Sha256::digest([b"draft-launch\0".as_slice(), &digest_input].concat())
        );
        let key = serde_json::to_string(&(actor.id.as_str(), params.idempotency_key.as_str()))
            .map_err(|_| Error::new("invalid_params", "request key cannot be encoded"))?;
        if let Some(receipt) = store.state()?.receipts.get(&key) {
            if receipt.digest != command_digest {
                return Err(Error::new(
                    "idempotency_conflict",
                    "request key was already used for a different command",
                ));
            }
            return Ok(receipt.result.clone());
        }
        if params.expected_revision != store.state()?.revision {
            return Err(Error::new(
                "revision_conflict",
                "read a fresh snapshot before retrying with a new request key",
            ));
        }
        let state = store.state()?;
        let problems = drafts::problems(state, &params.draft);
        if !problems.is_empty() {
            return Err(Error::new("draft_invalid", problems[0].clone())
                .with_details(json!({ "problems": problems })));
        }
        let team_id = drafts::team_id(state, &params.draft);
        let dir = self.root.join(drafts::KEYS_DIR).join(&team_id);
        make_private_dir(&dir)?;
        let mut steps = 1;
        let outcome = (|| -> Result<(Value, model::State)> {
            let mut digests = Vec::new();
            for role in ["lead", "developer", "qa"] {
                digests.push(create_credential(&dir.join(format!("{role}.key")))?);
            }
            let digests: [String; 3] = digests
                .try_into()
                .map_err(|_| Error::new("corrupt_state", "credential count"))?;
            let commands = drafts::plan(state, &params.draft, &digests);
            steps = commands.len() as u64;
            let mut candidate = state.clone();
            drafts::apply(&mut candidate, actor, commands, unix_now())?;
            let spec_id = format!("{team_id}-spec");
            let result = json!({
                "type": "teams_draft_launched",
                "team_id": team_id, "spec_id": spec_id,
                "project_id": candidate.teams.get(&team_id).map(|t| t.project_id.clone()),
                "credential_files": {
                    "lead": dir.join("lead.key"), "developer": dir.join("developer.key"), "qa": dir.join("qa.key"),
                },
                "revision": candidate.revision, "stream_id": candidate.stream_id,
                "watermark": candidate.next_sequence.saturating_sub(1),
            });
            candidate.receipts.insert(
                key,
                model::StoredReceipt {
                    digest: command_digest,
                    result: result.clone(),
                    revision: candidate.revision,
                },
            );
            candidate.trim_receipts();
            candidate.validate()?;
            Ok((result, candidate))
        })();
        let (result, candidate) = match outcome {
            Ok(done) => done,
            Err(error) => {
                // Nothing was committed, so the keys made for it are of no use to anyone.
                let _ = std::fs::remove_dir_all(&dir);
                return Err(error);
            }
        };
        let watermark = candidate.next_sequence.saturating_sub(1);
        if let Err(error) = store.commit_steps(candidate, steps) {
            if error.code != "commit_unknown" {
                let _ = std::fs::remove_dir_all(&dir);
            }
            return Err(error);
        }
        self.publish(watermark);
        Ok(result)
    }
}

fn events_page(
    state: &model::State,
    actor: &model::Actor,
    stream_id: &str,
    after: u64,
    limit: usize,
) -> Result<Value> {
    if !(1..=100).contains(&limit) {
        return Err(Error::new(
            "invalid_params",
            "event page limit must be between 1 and 100",
        ));
    }
    state.event_page(actor, stream_id, after, limit)
}

fn make_private_dir(dir: &Path) -> Result<()> {
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::create_dir(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(Error::new(
                "already_exists",
                "credential files for that team id already exist",
            ));
        }
        Err(error) => return Err(error.into()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn decode<T: serde::de::DeserializeOwned>(params: Value) -> Result<T> {
    // Parser diagnostics may contain bearer strings or message bodies. Never echo them.
    serde_json::from_value(params)
        .map_err(|_| Error::new("invalid_params", "invalid parameters for this Teams method"))
}

fn ensure_supported() -> Result<()> {
    if !crate::platform::private_credentials_supported() {
        return Err(Error::new(
            "unsupported_platform",
            "Teams private credential validation is unavailable on this platform",
        ));
    }
    Ok(())
}

fn validate_token(token: &str) -> Result<()> {
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::new("unauthorized", "invalid Teams credential"));
    }
    Ok(())
}

fn credential_digest(token: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"moo-teams-credential-v1\0");
    digest.update(token.as_bytes());
    format!("{:x}", digest.finalize())
}

pub(crate) fn create_credential(path: &Path) -> Result<String> {
    ensure_supported()?;
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|_| {
        Error::new(
            "entropy_unavailable",
            "could not generate a Teams credential",
        )
    })?;
    let token = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let mut file = crate::platform::create_private_state_file(path)?;
    crate::platform::validate_private_file(&file)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    crate::platform::sync_parent_directory(path.parent().unwrap_or_else(|| Path::new(".")))?;
    Ok(credential_digest(&token))
}

pub(crate) fn read_credential(path: &Path) -> Result<String> {
    ensure_supported()?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 128 {
        return Err(Error::new(
            "invalid_credential_file",
            "credential must be a bounded private regular file",
        ));
    }
    let file = std::fs::File::open(path)?;
    crate::platform::validate_private_file(&file)?;
    let mut token = String::new();
    file.take(129).read_to_string(&mut token)?;
    let token = token.trim().to_owned();
    validate_token(&token)?;
    Ok(token)
}

/// The machine-readable API schema this build serves (also printed by `moo teams schema`).
pub(crate) fn schema_document() -> Value {
    catalog::schema_document()
}

/// Whether this session has a Teams store, so the main screen can offer Teams commands only to
/// people who use Teams. One `stat`, made when commands are (re)loaded, never per frame.
pub(crate) fn store_present() -> bool {
    cfg!(unix) && crate::session::data_dir().join(store::STORE_FILE).is_file()
}

pub(crate) fn initialize(root: &Path) -> Result<PathBuf> {
    ensure_supported()?;
    std::fs::create_dir_all(root)?;
    let path = root.join("teams-owner.key");
    match create_credential(&path) {
        Ok(_) => {}
        Err(error) if path.exists() && error.code == "storage_error" => {
            read_credential(&path)?;
        }
        Err(error) => return Err(error),
    }
    let token = read_credential(&path)?;
    std::fs::File::open(&path)?.sync_all()?;
    crate::platform::sync_parent_directory(root)?;
    let owner_digest = credential_digest(&token);
    let mut stream = Sha256::new();
    stream.update(b"moo-teams-stream-v1\0");
    stream.update(token.as_bytes());
    let stream_id = format!("{:x}", stream.finalize());
    store::bootstrap(root, owner_digest, stream_id)?;
    Ok(path)
}

#[cfg(test)]
mod api_tests;
#[cfg(test)]
mod tests;

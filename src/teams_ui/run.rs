//! Terminal, event loop and the worker that talks to the Teams socket.

use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use serde_json::{json, Value};

use super::builder::{Draft, Preview};
use super::data::Snapshot;
use super::render::{render, resolve_palette};
use super::state::{Effect, Launch, Screen};
use crate::api::client::ApiClient;
use crate::app::state::Palette;

const POLL_EVERY: Duration = Duration::from_secs(1);

enum Job {
    Refresh,
    /// Commands sent one after another; the first failure stops the rest.
    Batch(Vec<Value>),
    /// `teams.drafts.propose.v1`
    Propose(String),
    /// `teams.drafts.validate.v1`
    CheckDraft(Value),
    /// `teams.drafts.launch.v1`
    LaunchDraft(Value),
}

enum Update {
    Snapshot(Box<Snapshot>),
    Failed(String),
    Done(Result<String, String>),
    /// A start moved on a step.
    Launch(String, Launch),
    /// A start finished: the note to show, or why it failed.
    Launched(String, Result<String, String>),
    Proposal(Result<Draft, String>),
    DraftChecked(Result<Preview, String>),
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        Terminal::new(CrosstermBackend::new(stdout))
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
    }
}

fn request(client: &ApiClient, token: &str, method: &str, params: Value) -> Result<Value, String> {
    let response = client
        .request_json_value(&json!({
            "id": "teams-ui",
            "method": method,
            "auth": { "bearer": token },
            "params": params,
        }))
        .map_err(|error| error.to_string())?;
    if let Some(error) = response.get("error") {
        let code = error["code"].as_str().unwrap_or("error");
        let message = error["message"].as_str().unwrap_or("");
        return Err(format!("{code}: {message}"));
    }
    Ok(response["result"].clone())
}

/// The agent to start: who it is, which team and role, and which CLI runs it.
struct Member {
    actor_id: String,
    team_id: String,
    role: String,
    kind: String,
    model: Option<String>,
    /// The pane the agent is recorded in. Nothing starts while that pane still exists.
    existing: Option<String>,
}

/// Launch arguments that select `model`; every agent CLI Moo starts takes `--model`.
fn model_args(model: &Option<String>) -> Vec<&str> {
    model.iter().flat_map(|m| ["--model", m.as_str()]).collect()
}

/// How long a started agent gets to report ready before the briefing is left for the user.
const READY_WITHIN: Duration = Duration::from_secs(60);
const START_POLL: Duration = Duration::from_millis(250);
/// How long an agent may wait for the user (a trust or login prompt) before the start gives up.
/// The server's own launch limit, so it keeps the launch for as long as we wait.
const ANSWER_WITHIN: Duration = crate::app::MAX_AGENT_START_TIMEOUT;

/// Starts `kind` in a background tab, records the pane as the agent's own and sends it the
/// briefing, so it joins its team and talks in Teams chat. The pane is bound even when the agent
/// is slow to report ready, so the role keeps its terminal.
fn start_agent(
    client: &ApiClient,
    jobs: &Sender<Job>,
    updates: &Sender<Update>,
    root: &Path,
    member: Member,
) {
    let Member {
        actor_id,
        team_id,
        role,
        kind,
        model,
        existing,
    } = member;
    let call = |method: &str, params: Value| -> Result<Value, String> {
        let reply = client
            .request_json_value(
                &json!({"id": "teams-ui-start", "method": method, "params": params}),
            )
            .map_err(|error| format!("could not reach Moo: {error}"))?;
        match reply.get("error") {
            Some(error) => Err(format!(
                "{}: {}",
                error["code"].as_str().unwrap_or("error"),
                error["message"].as_str().unwrap_or("unknown error")
            )),
            None => Ok(reply["result"].clone()),
        }
    };
    let step = |launch: Launch| {
        let _ = updates.send(Update::Launch(actor_id.clone(), launch));
    };
    let note = (|| {
        // A recorded pane that is gone (it was closed, or never existed) does not count.
        if let Some(pane) = existing {
            if call("pane.get", json!({"pane_id": pane})).is_ok() {
                return Ok(format!(
                    "{actor_id} is already in {pane} - enter to focus it"
                ));
            }
        }
        // An agent that cannot reach its team is no use, so nothing starts without a credential.
        let credential = super::start::credential_path(root, &team_id, &role)?;
        let tab = call("tab.create", json!({"label": actor_id, "focus": false}))
            .map_err(|e| format!("could not open a tab for {actor_id}: {e}"))?;
        let pane = tab["root_pane"]["pane_id"]
            .as_str()
            .ok_or_else(|| format!("no pane was created for {actor_id}"))?
            .to_owned();
        let deadline = Instant::now() + READY_WITHIN;
        // The new tab's shell is still starting, so the pane reads busy for a moment.
        let started = loop {
            let started = call(
                "agent.start",
                json!({"name": actor_id, "kind": kind, "pane_id": pane, "args": model_args(&model),
                       "timeout_ms": ANSWER_WITHIN.as_millis() as u64}),
            );
            match started {
                Err(e) if e.starts_with("agent_pane_busy") && Instant::now() < deadline => {
                    std::thread::sleep(START_POLL);
                }
                started => break started,
            }
        };
        let bind = json!({"kind": "bind_agent_pane", "actor_id": actor_id, "pane_id": pane, "provider": kind});
        let _ = jobs.send(Job::Batch(vec![bind]));
        started.map_err(|e| format!("{actor_id} has {pane}, but {kind} did not start: {e}"))?;
        // agent.start only launches the CLI; it takes prompts once it reports interactive_ready.
        // Time spent waiting on the user does not count, up to ANSWER_WITHIN.
        let give_up = Instant::now() + ANSWER_WITHIN;
        let mut deadline = deadline;
        let mut blocked = false;
        loop {
            if Instant::now() >= deadline.min(give_up) {
                let why = if blocked {
                    "is waiting for an answer"
                } else {
                    "was not ready in time"
                };
                return Err(format!(
                    "{actor_id} has {pane}, but {kind} {why} - send it its briefing by hand"
                ));
            }
            if let Ok(got) = call("agent.get", json!({"target": pane})) {
                let agent = &got["agent"];
                let was_blocked = blocked;
                blocked = agent["agent_status"].as_str() == Some("blocked");
                if blocked != was_blocked {
                    step(if blocked {
                        Launch::NeedsYou
                    } else {
                        Launch::Starting
                    });
                }
                if blocked {
                    deadline = deadline.max(Instant::now() + READY_WITHIN);
                }
                if !blocked && agent["interactive_ready"].as_bool() == Some(true) {
                    break;
                }
                if matches!(agent["agent_status"].as_str(), Some("idle" | "done"))
                    && !agent["launch_pending"].as_bool().unwrap_or(false)
                {
                    return Err(format!(
                        "{actor_id} has {pane}, but {kind} exited before it was ready"
                    ));
                }
            }
            std::thread::sleep(START_POLL);
        }
        step(Launch::Briefing);
        let text = super::start::briefing(&actor_id, &role, &team_id, &credential);
        call("agent.prompt", json!({"target": pane, "text": text})).map_err(|e| {
            format!("{actor_id} is running {kind} in {pane}, but its briefing failed: {e}")
        })?;
        Ok(format!("{actor_id} is running {kind} - see Chat"))
    })();
    let _ = updates.send(Update::Launched(actor_id, note));
}

fn poll(client: &ApiClient, token: &str, revision: &mut u64, updates: &Sender<Update>) {
    match request(client, token, "teams.snapshot.get.v1", json!({})) {
        Ok(value) => match serde_json::from_value::<Snapshot>(value) {
            Ok(snapshot) => {
                *revision = snapshot.revision;
                let _ = updates.send(Update::Snapshot(Box::new(snapshot)));
            }
            Err(_) => {
                let _ = updates.send(Update::Failed("unreadable snapshot".into()));
            }
        },
        Err(message) => {
            let _ = updates.send(Update::Failed(message));
        }
    }
}

fn worker(client: ApiClient, token: String, jobs: Receiver<Job>, updates: Sender<Update>) {
    let mut revision = 0u64;
    let mut serial = 0u64;
    loop {
        match jobs.recv_timeout(POLL_EVERY) {
            Ok(Job::Refresh) | Err(mpsc::RecvTimeoutError::Timeout) => {
                poll(&client, &token, &mut revision, &updates);
            }
            Ok(Job::Batch(commands)) => {
                let total = commands.len();
                let mut outcome = Ok("sent".to_owned());
                for (index, command) in commands.into_iter().enumerate() {
                    serial += 1;
                    let key = format!("ui-{}-{}-{serial}", std::process::id(), unix_nanos());
                    let mut result = Err(String::new());
                    for _ in 0..2 {
                        result = request(
                            &client,
                            &token,
                            "teams.mutate.v1",
                            json!({ "expected_revision": revision, "idempotency_key": key, "command": command }),
                        );
                        // Someone else wrote first: read the new revision and send the same command once more.
                        if matches!(&result, Err(m) if m.starts_with("revision_conflict")) {
                            poll(&client, &token, &mut revision, &updates);
                            continue;
                        }
                        break;
                    }
                    match result {
                        Ok(value) => revision = value["revision"].as_u64().unwrap_or(revision),
                        Err(message) => {
                            outcome = Err(if total > 1 {
                                format!("step {} of {total} failed - {message}", index + 1)
                            } else {
                                message
                            });
                            break;
                        }
                    }
                }
                let _ = updates.send(Update::Done(outcome));
                poll(&client, &token, &mut revision, &updates);
            }
            Ok(Job::Propose(goal)) => {
                let result = request(
                    &client,
                    &token,
                    "teams.drafts.propose.v1",
                    json!({"goal": goal}),
                )
                .and_then(|value| {
                    Draft::from_proposal(&value).ok_or_else(|| "unreadable proposal".to_owned())
                });
                let _ = updates.send(Update::Proposal(result));
            }
            Ok(Job::CheckDraft(draft)) => {
                let result = request(
                    &client,
                    &token,
                    "teams.drafts.validate.v1",
                    json!({"draft": draft}),
                )
                .and_then(|value| {
                    Preview::from_value(&value).ok_or_else(|| "unreadable draft check".to_owned())
                });
                let _ = updates.send(Update::DraftChecked(result));
            }
            Ok(Job::LaunchDraft(draft)) => {
                serial += 1;
                let key = format!("ui-launch-{}-{}-{serial}", std::process::id(), unix_nanos());
                let mut result = Err(String::new());
                for _ in 0..2 {
                    result = request(
                        &client,
                        &token,
                        "teams.drafts.launch.v1",
                        json!({"expected_revision": revision, "idempotency_key": key, "draft": draft}),
                    );
                    if matches!(&result, Err(m) if m.starts_with("revision_conflict")) {
                        poll(&client, &token, &mut revision, &updates);
                        continue;
                    }
                    break;
                }
                let _ = updates.send(Update::Done(result.map(|value| {
                    format!(
                        "created {} - keys in {}",
                        value["team_id"].as_str().unwrap_or("team"),
                        value["credential_files"]["lead"]
                            .as_str()
                            .and_then(|p| std::path::Path::new(p).parent())
                            .map_or_else(String::new, |p| p.display().to_string())
                    )
                })));
                poll(&client, &token, &mut revision, &updates);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn unix_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

fn load_state(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn save_state(path: &Path, value: &Value) {
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, value.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// The user's configured theme (name, custom colors, legacy accent), as the main screen resolves it.
fn configured_palette() -> Palette {
    let loaded = crate::config::Config::load();
    let theme = &loaded.config.theme;
    let name = theme.name.as_deref().unwrap_or("catppuccin");
    let mut palette = Palette::from_name(name).unwrap_or_else(Palette::catppuccin);
    if let Some(custom) = &theme.custom {
        palette = palette.with_overrides(custom);
    }
    palette
}

/// Where to open, when launched from a command rather than by hand.
#[derive(Debug, Default)]
pub(crate) struct Start {
    pub view: Option<String>,
    pub builder: bool,
}

pub(crate) fn run(
    client: ApiClient,
    token: String,
    state_path: PathBuf,
    start: Start,
) -> io::Result<()> {
    let configured = configured_palette();
    let (job_tx, job_rx) = mpsc::channel();
    let (update_tx, update_rx) = mpsc::channel();
    let focus_client = client.clone();
    {
        let (client, token, update_tx) = (client, token, update_tx.clone());
        std::thread::Builder::new()
            .name("teams-ui-worker".into())
            .spawn(move || worker(client, token, job_rx, update_tx))?;
    }
    let mut terminal = TerminalGuard::enter()?;
    let _guard = TerminalGuard;
    let size = terminal.size()?;
    let mut screen = Screen::new(Rect::new(0, 0, size.width, size.height));
    if let Some(saved) = load_state(&state_path) {
        screen.restore_memory(&saved);
    }
    if let Some(view) = start.view.as_deref().and_then(Screen::view_named) {
        screen.open_at(view);
    }
    let mut open_builder = start.builder;
    let mut dirty = true;
    'main: loop {
        while let Ok(update) = update_rx.try_recv() {
            dirty = true;
            match update {
                Update::Snapshot(snapshot) => {
                    screen.apply_snapshot(*snapshot);
                    // The builder needs the snapshot (taken ids, projects), so it opens on the first one.
                    if std::mem::take(&mut open_builder) {
                        screen.run_intent(super::state::Intent::NewTeam);
                    }
                }
                Update::Failed(message) => screen.set_link_error(message),
                Update::Done(Ok(note)) => screen.toast = Some(note),
                Update::Done(Err(message)) => screen.toast = Some(message),
                Update::Launch(actor_id, launch) => {
                    screen.launches.insert(actor_id, launch);
                }
                Update::Launched(actor_id, outcome) => screen.finish_launch(&actor_id, outcome),
                Update::Proposal(Ok(draft)) => screen.open_builder(draft),
                Update::Proposal(Err(message)) => screen.toast = Some(message),
                Update::DraftChecked(result) => screen.apply_draft_check(result),
            }
        }
        if dirty {
            let palette = resolve_palette(&screen, &configured);
            terminal.draw(|frame| {
                let area = frame.area();
                render(frame.buffer_mut(), area, &screen, &palette);
            })?;
            dirty = false;
        }
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let effects = match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => screen.key(key),
            Event::Mouse(mouse) => screen.mouse(mouse),
            Event::Resize(width, height) => {
                screen.resize(Rect::new(0, 0, width, height));
                Vec::new()
            }
            _ => Vec::new(),
        };
        dirty = true;
        for effect in effects {
            match effect {
                Effect::Quit => break 'main,
                Effect::Refresh => {
                    let _ = job_tx.send(Job::Refresh);
                }
                Effect::Mutate { command } => {
                    let _ = job_tx.send(Job::Batch(vec![command]));
                }
                Effect::MutateAll { commands } => {
                    let _ = job_tx.send(Job::Batch(commands));
                }
                Effect::FocusAgent { actor_id, pane_id } => {
                    screen.toast = Some(match pane_id {
                        None => format!("{actor_id} has no bound pane - press b to bind one"),
                        Some(pane) => {
                            let reply = focus_client.request_json_value(
                                &json!({"id": "teams-ui-focus", "method": "pane.focus", "params": {"pane_id": pane}}),
                            );
                            match reply {
                                Ok(value) if value.get("error").is_none() => {
                                    format!("focused {pane} - close this screen (q) to see it")
                                }
                                Ok(value) => format!(
                                    "could not focus {pane}: {}",
                                    value["error"]["message"]
                                        .as_str()
                                        .unwrap_or("unknown error")
                                ),
                                Err(error) => {
                                    format!("could not reach Moo to focus {pane}: {error}")
                                }
                            }
                        }
                    });
                }
                Effect::StartAgent {
                    actor_id,
                    team_id,
                    role,
                    kind,
                    model,
                    existing,
                } => {
                    screen.toast = Some(format!("starting {kind} for {actor_id}..."));
                    screen.launches.insert(actor_id.clone(), Launch::Starting);
                    let (client, jobs, updates) =
                        (focus_client.clone(), job_tx.clone(), update_tx.clone());
                    let root = state_path
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_default();
                    let member = Member {
                        actor_id,
                        team_id,
                        role,
                        kind,
                        model,
                        existing,
                    };
                    let _ = std::thread::Builder::new()
                        .name("teams-ui-start-agent".into())
                        .spawn(move || start_agent(&client, &jobs, &updates, &root, member));
                }
                Effect::ValidateDraft { draft } => {
                    let _ = job_tx.send(Job::CheckDraft(draft.to_params()));
                }
                Effect::LaunchDraft { draft } => {
                    screen.toast = Some(format!("creating {}...", draft.name));
                    let _ = job_tx.send(Job::LaunchDraft(draft.to_params()));
                }
                Effect::Propose { goal } => {
                    screen.toast = Some("asking for a proposal...".into());
                    let _ = job_tx.send(Job::Propose(goal));
                }
            }
        }
    }
    save_state(&state_path, &screen.memory_snapshot());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::model_args;

    #[test]
    fn a_chosen_model_becomes_a_launch_argument_and_no_model_adds_none() {
        assert_eq!(model_args(&Some("opus".into())), ["--model", "opus"]);
        assert!(model_args(&None).is_empty());
    }
}

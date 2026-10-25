//! Terminal, event loop and the worker that talks to the Teams socket.

use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

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
use super::state::{Effect, Screen};
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

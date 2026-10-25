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

use super::data::Snapshot;
use super::render::render;
use super::state::{Effect, Screen};
use crate::api::client::ApiClient;
use crate::app::state::Palette;

const POLL_EVERY: Duration = Duration::from_secs(1);

enum Job {
    Refresh,
    Mutate(Value),
}

enum Update {
    Snapshot(Box<Snapshot>),
    Failed(String),
    Mutated(Result<String, String>),
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

fn worker(client: ApiClient, token: String, jobs: Receiver<Job>, updates: Sender<Update>) {
    let mut revision = 0u64;
    let mut serial = 0u64;
    let poll = |revision: &mut u64, updates: &Sender<Update>| match request(
        &client,
        &token,
        "teams.snapshot.get.v1",
        json!({}),
    ) {
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
    };
    loop {
        match jobs.recv_timeout(POLL_EVERY) {
            Ok(Job::Refresh) | Err(mpsc::RecvTimeoutError::Timeout) => {
                poll(&mut revision, &updates)
            }
            Ok(Job::Mutate(command)) => {
                serial += 1;
                let key = format!(
                    "ui-{}-{}-{serial}",
                    std::process::id(),
                    Instant::now().elapsed().as_nanos()
                );
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
                        poll(&mut revision, &updates);
                        continue;
                    }
                    break;
                }
                let _ = updates.send(Update::Mutated(result.map(|_| "done".to_owned())));
                poll(&mut revision, &updates);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
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

pub(crate) fn run(client: ApiClient, token: String, state_path: PathBuf) -> io::Result<()> {
    let palette = Palette::catppuccin();
    let (job_tx, job_rx) = mpsc::channel();
    let (update_tx, update_rx) = mpsc::channel();
    {
        let (client, token) = (client, token);
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
    let mut dirty = true;
    'main: loop {
        while let Ok(update) = update_rx.try_recv() {
            dirty = true;
            match update {
                Update::Snapshot(snapshot) => screen.apply_snapshot(*snapshot),
                Update::Failed(message) => screen.set_link_error(message),
                Update::Mutated(Ok(_)) => screen.toast = Some("sent".into()),
                Update::Mutated(Err(message)) => screen.toast = Some(message),
            }
        }
        if dirty {
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
                    let _ = job_tx.send(Job::Mutate(command));
                }
                Effect::FocusAgent { actor_id } => {
                    screen.toast = Some(format!(
                        "no terminal is bound to {actor_id}: Teams does not link agents to panes yet"
                    ));
                }
            }
        }
    }
    save_state(&state_path, &screen.memory_snapshot());
    Ok(())
}

use std::time::{Duration, Instant};

use super::{App, SESSION_SAVE_DEBOUNCE};

enum SessionSaveJob {
    Clear,
    Save {
        snapshot: crate::persist::SessionSnapshot,
        history: Option<crate::persist::SessionHistorySnapshot>,
    },
}

impl App {
    pub(super) fn schedule_session_save(&mut self) {
        if self.policy.persist_session {
            self.pane_exit_checkpoint_pending = false;
            self.session_save_deadline = Some(Instant::now() + SESSION_SAVE_DEBOUNCE);
        }
    }

    pub(crate) fn sync_session_save_schedule(&mut self) {
        if self.state.session_dirty {
            self.state.session_dirty = false;
            self.schedule_session_save();
        }
    }

    fn reap_finished_session_save(&mut self) {
        if self
            .session_save_thread
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
        {
            if let Some(thread) = self.session_save_thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn capture_session_save_job(
        &self,
    ) -> (SessionSaveJob, Vec<crate::api::schema::AgentHistoryEntry>) {
        let agent_history = self.agent_history.iter().cloned().collect();
        let job = if self.state.workspaces.is_empty() {
            SessionSaveJob::Clear
        } else {
            // Commands opened over the user's work (the palette's pane commands) are temporary: a
            // restart cannot bring their program back, so the saved session leaves them out.
            let overlays: Vec<crate::persist::OverlayPane> = self
                .overlay_panes
                .iter()
                .map(|(pane_id, overlay)| crate::persist::OverlayPane {
                    workspace_index: overlay.ws_idx,
                    tab_index: overlay.tab_idx,
                    pane_id: *pane_id,
                    previous_focus: overlay.previous_focus,
                    previous_zoomed: overlay.previous_zoomed,
                })
                .collect();
            let snapshot = crate::persist::capture_without_overlays(
                &self.state.workspaces,
                &self.state.terminals,
                &self.terminal_runtimes,
                self.state.active,
                self.state.selected,
                &overlays,
            );
            let history = self.persist_pane_history.then(|| {
                crate::persist::capture_history_without_overlays(
                    &snapshot,
                    &self.state.workspaces,
                    &self.terminal_runtimes,
                    &overlays,
                )
            });
            SessionSaveJob::Save { snapshot, history }
        };
        (job, agent_history)
    }

    pub(crate) fn start_background_session_save(&mut self) {
        if !self.policy.persist_session {
            self.session_save_deadline = None;
            return;
        }

        self.reap_finished_session_save();
        if self.session_save_thread.is_some() {
            self.session_save_deadline = Some(Instant::now() + Duration::from_millis(250));
            return;
        }

        let (job, agent_history) = self.capture_session_save_job();
        self.pane_exit_checkpoint_pending = false;
        self.session_save_deadline = None;
        let writer = self.session_writer.clone();
        match std::thread::Builder::new()
            .name("herdr-session-save".into())
            .spawn(move || run_session_save_job(job, &agent_history, &writer))
        {
            Ok(thread) => self.session_save_thread = Some(thread),
            Err(err) => {
                tracing::warn!(err = %err, "failed to spawn session save thread; saving inline");
                {
                    let (job, agent_history) = self.capture_session_save_job();
                    run_session_save_job(job, &agent_history, &self.session_writer);
                }
            }
        }
    }

    pub(crate) fn save_session_now(&mut self) {
        if let Some(thread) = self.session_save_thread.take() {
            let _ = thread.join();
        }

        if !self.policy.persist_session {
            self.session_save_deadline = None;
            return;
        }

        {
            let (job, agent_history) = self.capture_session_save_job();
            run_session_save_job(job, &agent_history, &self.session_writer);
        }
        self.pane_exit_checkpoint_pending = false;
        self.session_save_deadline = None;
    }

    pub(crate) fn checkpoint_session_before_pane_exit(&mut self) {
        if !self.policy.persist_session
            || (self.pane_exit_checkpoint_pending && !self.state.session_dirty)
        {
            return;
        }
        self.save_session_now();
        self.pane_exit_checkpoint_pending = true;
        self.state.session_dirty = false;
    }

    pub(crate) fn finish_checkpointed_pane_exit(&mut self) {
        if self.pane_exit_checkpoint_pending {
            self.state.session_dirty = false;
            self.session_save_deadline = Some(Instant::now() + SESSION_SAVE_DEBOUNCE);
        }
    }

    pub(crate) fn save_session_on_shutdown(&mut self) {
        if self.pane_exit_checkpoint_pending && !self.state.session_dirty {
            self.session_save_deadline = None;
            return;
        }
        self.save_session_now();
    }
}

fn run_session_save_job(
    job: SessionSaveJob,
    agent_history: &[crate::api::schema::AgentHistoryEntry],
    writer: &std::sync::Mutex<crate::persist::SessionWriter>,
) {
    let mut writer = match writer.lock() {
        Ok(writer) => writer,
        Err(err) => {
            tracing::warn!(err = %err, "session writer is poisoned; refusing to modify session");
            return;
        }
    };
    match job {
        SessionSaveJob::Clear => writer.clear(),
        SessionSaveJob::Save { snapshot, history } => writer.save(&snapshot, history.as_ref()),
    }
    crate::persist::agent_history::save(agent_history);
}

use super::*;

use crate::api::schema::{AgentHistoryParams, Method, ResponseResult};

const PAGE_ROWS: usize = 8;

impl ClientShellState {
    /// Opens the replay timeline for the focused agent pane and asks the server for its history.
    pub(super) fn open_session_replay(&mut self, outcome: &mut ClientShellInput) {
        let Some(pane_id) = self
            .snapshot
            .as_deref()
            .and_then(|snapshot| snapshot.focused_pane_id.clone())
        else {
            return;
        };
        self.overlay = Some(ClientShellOverlay::SessionReplay(
            ClientSessionReplayOverlay {
                pane_id: pane_id.clone(),
                entries: None,
                back: 0,
            },
        ));
        self.chrome_drag = None;
        let method = Method::AgentHistory(AgentHistoryParams {
            limit: None,
            pane_id: Some(pane_id),
        });
        if !self.push_endpoint_method_with_kind(method, PendingEndpointKind::AgentHistory, outcome)
        {
            self.overlay = None;
        }
    }

    pub(super) fn handle_session_replay_result(
        &mut self,
        result: Result<ResponseResult, ClientShellEndpointError>,
    ) -> bool {
        let Some(ClientShellOverlay::SessionReplay(replay)) = self.overlay.as_mut() else {
            return false;
        };
        match result {
            Ok(ResponseResult::AgentHistory { entries }) => replay.entries = Some(entries),
            Ok(_) => {
                self.overlay = None;
                self.set_endpoint_error("endpoint returned an unexpected agent history result");
            }
            Err(_) => self.overlay = None,
        }
        true
    }

    pub(super) fn route_session_replay_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) {
        let Some(ClientShellOverlay::SessionReplay(replay)) = self.overlay.as_mut() else {
            return;
        };
        let newest = replay
            .entries
            .as_ref()
            .map_or(0, |entries| entries.len().saturating_sub(1));
        let back = replay.back;
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                self.overlay = None;
                outcome.repaint = true;
                return;
            }
            KeyCode::Up | KeyCode::Char('k') => replay.back = (back + 1).min(newest),
            KeyCode::Down | KeyCode::Char('j') => replay.back = back.saturating_sub(1),
            KeyCode::PageUp => replay.back = (back + PAGE_ROWS).min(newest),
            KeyCode::PageDown => replay.back = back.saturating_sub(PAGE_ROWS),
            KeyCode::Home => replay.back = newest,
            KeyCode::End => replay.back = 0,
            _ => return,
        }
        outcome.repaint = true;
    }

    pub(super) fn scroll_session_replay(&mut self, rows: isize) {
        if let Some(ClientShellOverlay::SessionReplay(replay)) = self.overlay.as_mut() {
            let newest = replay
                .entries
                .as_ref()
                .map_or(0, |entries| entries.len().saturating_sub(1));
            replay.back = replay.back.saturating_add_signed(-rows).min(newest);
        }
    }
}

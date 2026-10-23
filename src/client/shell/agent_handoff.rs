use super::*;
use crate::protocol::ClientShellAgent;

use crate::api::schema::{
    AgentPromptParams, AgentReadParams, Method, ReadFormat, ReadSource, ResponseResult,
};

/// Same precedence as the agents panel: a user rename outranks integration metadata.
pub(super) fn agent_label(agent: &ClientShellAgent) -> &str {
    agent
        .name
        .as_deref()
        .or(agent.display_agent.as_deref())
        .or(agent.agent.as_deref())
        .or(agent.title.as_deref())
        .unwrap_or(&agent.pane_id)
}

/// Moves `current` by `delta` within `agents`, staying put at either end.
fn step(agents: &[ClientShellAgent], current: &str, delta: isize) -> Option<String> {
    let index = agents.iter().position(|agent| agent.pane_id == current);
    let next = (index.unwrap_or(0) as isize + delta).clamp(0, agents.len() as isize - 1);
    agents
        .get(usize::try_from(next).ok()?)
        .map(|a| a.pane_id.clone())
}

impl ClientShellState {
    fn handoff_agents(&self) -> &[ClientShellAgent] {
        self.snapshot
            .as_deref()
            .map_or(&[], |snapshot| snapshot.agents.as_slice())
    }

    pub(super) fn open_agent_handoff(&mut self) {
        let agents = self.handoff_agents();
        let focused = agents.iter().find(|agent| agent.focused).or(agents.first());
        let Some(from) = focused.map(|agent| agent.pane_id.clone()) else {
            return;
        };
        let Some(to) = agents
            .iter()
            .find(|agent| agent.pane_id != from)
            .map(|agent| agent.pane_id.clone())
        else {
            return;
        };
        self.overlay = Some(ClientShellOverlay::AgentHandoff(
            ClientAgentHandoffOverlay {
                from,
                to,
                column: ClientHandoffColumn::From,
                sending: false,
            },
        ));
        self.chrome_drag = None;
    }

    pub(super) fn select_handoff_agent(&mut self, column: ClientHandoffColumn, pane_id: String) {
        if let Some(ClientShellOverlay::AgentHandoff(handoff)) = self.overlay.as_mut() {
            handoff.column = column;
            match column {
                ClientHandoffColumn::From => handoff.from = pane_id,
                ClientHandoffColumn::To => handoff.to = pane_id,
            }
        }
    }

    pub(super) fn route_agent_handoff_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) {
        let Some(ClientShellOverlay::AgentHandoff(handoff)) = self.overlay.as_ref() else {
            return;
        };
        let (column, active) = match handoff.column {
            ClientHandoffColumn::From => (ClientHandoffColumn::From, handoff.from.clone()),
            ClientHandoffColumn::To => (ClientHandoffColumn::To, handoff.to.clone()),
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.overlay = None,
            KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Char('h')
            | KeyCode::Char('l') => {
                if let Some(ClientShellOverlay::AgentHandoff(handoff)) = self.overlay.as_mut() {
                    handoff.column = match column {
                        ClientHandoffColumn::From => ClientHandoffColumn::To,
                        ClientHandoffColumn::To => ClientHandoffColumn::From,
                    };
                }
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Down | KeyCode::Char('j') => {
                let delta = if matches!(key.code, KeyCode::Up | KeyCode::Char('k')) {
                    -1
                } else {
                    1
                };
                if let Some(next) = step(self.handoff_agents(), &active, delta) {
                    self.select_handoff_agent(column, next);
                }
            }
            KeyCode::Enter => self.send_agent_handoff(outcome),
            _ => return,
        }
        outcome.repaint = true;
    }

    fn send_agent_handoff(&mut self, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::AgentHandoff(handoff)) = self.overlay.as_ref() else {
            return;
        };
        if handoff.sending {
            return;
        }
        if handoff.from == handoff.to {
            self.set_endpoint_error("pick two different agents");
            return;
        }
        let (from, to) = (handoff.from.clone(), handoff.to.clone());
        let from_label = self
            .handoff_agents()
            .iter()
            .find(|agent| agent.pane_id == from)
            .map_or_else(|| from.clone(), |agent| agent_label(agent).to_owned());
        let method = Method::AgentRead(AgentReadParams {
            target: from,
            source: ReadSource::Recent,
            lines: Some(crate::api::handoff::DEFAULT_LINES),
            format: ReadFormat::Text,
            strip_ansi: true,
        });
        if self.push_endpoint_method_with_kind(
            method,
            PendingEndpointKind::HandoffRead { from_label, to },
            outcome,
        ) {
            self.set_handoff_sending(true);
        }
    }

    fn set_handoff_sending(&mut self, sending: bool) {
        if let Some(ClientShellOverlay::AgentHandoff(handoff)) = self.overlay.as_mut() {
            handoff.sending = sending;
        }
    }

    /// Chains the two public requests: `agent.read` of the source, then `agent.prompt` of the
    /// target, so the server's existing idle and blocked checks stay the only authority.
    pub(super) fn handle_agent_handoff_result(
        &mut self,
        kind: PendingEndpointKind,
        result: Result<ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        let mut outcome = ClientShellInput::default();
        match (kind, result) {
            (
                PendingEndpointKind::HandoffRead { from_label, to },
                Ok(ResponseResult::PaneRead { read }),
            ) => match crate::api::handoff::prompt(&from_label, None, &read.text) {
                Some(text) => {
                    let method = Method::AgentPrompt(AgentPromptParams {
                        target: to,
                        text,
                        wait: None,
                    });
                    if !self.push_endpoint_method_with_kind(
                        method,
                        PendingEndpointKind::HandoffPrompt,
                        &mut outcome,
                    ) {
                        self.set_handoff_sending(false);
                    }
                }
                None => {
                    self.set_handoff_sending(false);
                    self.set_endpoint_error(format!("{from_label} has no output to hand off"));
                }
            },
            (PendingEndpointKind::HandoffPrompt, Ok(ResponseResult::AgentPrompted { .. })) => {
                if matches!(self.overlay, Some(ClientShellOverlay::AgentHandoff(_))) {
                    self.overlay = None;
                }
            }
            (_, Ok(_)) => {
                self.set_handoff_sending(false);
                self.set_endpoint_error("endpoint returned an unexpected handoff result");
            }
            // The failure is already surfaced by the endpoint layer; keep the modal open to retry.
            (_, Err(_)) => self.set_handoff_sending(false),
        }
        (true, outcome.actions)
    }
}

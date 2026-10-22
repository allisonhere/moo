use crate::app;
use crate::app::state::AppState;
use crate::config;
use crate::detect::AgentState;
use crate::layout::PaneId;
use crate::protocol;
use crate::terminal::TerminalRuntimeRegistry;

pub(crate) fn should_forward_toast_to_clients(delivery: config::ToastDelivery) -> bool {
    toast_notify_kind(delivery).is_some()
}

pub(crate) fn toast_notify_kind(delivery: config::ToastDelivery) -> Option<protocol::NotifyKind> {
    match delivery {
        config::ToastDelivery::Terminal => Some(protocol::NotifyKind::Toast),
        config::ToastDelivery::System => Some(protocol::NotifyKind::SystemToast),
        config::ToastDelivery::Off | config::ToastDelivery::Herdr => None,
    }
}

pub(crate) fn toast_message_from_state_change(
    state: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    pane_id: PaneId,
    suppress_active_tab_notifications: bool,
    prev_state: AgentState,
    new_state: AgentState,
) -> Option<String> {
    state
        .workspaces
        .iter()
        .enumerate()
        .find_map(|(ws_idx, ws)| {
            ws.tabs.iter().find_map(|tab| {
                let pane = tab.panes.get(&pane_id)?;
                let agent_label = state
                    .terminals
                    .get(&pane.attached_terminal_id)
                    .and_then(|terminal| terminal.effective_agent_label())?;
                let kind = app::actions::notification_toast_for_state_change(
                    suppress_active_tab_notifications,
                    prev_state,
                    new_state,
                )?;
                let workspace_label = ws.display_name_from(&state.terminals, terminal_runtimes);
                Some(format!(
                    "{} {}: {}",
                    agent_label,
                    toast_event_text(kind),
                    app::actions::notification_context(ws, &workspace_label, ws_idx, pane_id)
                ))
            })
        })
}

/// Title and body for one notification covering a batch of agent deliveries
/// that came due together. `None` when fewer than two would reach the client,
/// so a lone event keeps its own toast.
pub(crate) fn agent_digest(
    deliveries: &[app::state::AgentNotificationDelivery],
) -> Option<(String, String)> {
    let toasts: Vec<_> = deliveries
        .iter()
        .filter_map(|delivery| delivery.client_notification.as_ref())
        .collect();
    if toasts.len() < 2 {
        return None;
    }
    let count = |kind| toasts.iter().filter(|toast| toast.kind == kind).count();
    let mut parts = Vec::new();
    for (kind, text) in [
        (app::state::ToastKind::NeedsAttention, "need attention"),
        (app::state::ToastKind::Finished, "finished"),
    ] {
        match count(kind) {
            0 => {}
            n => parts.push(format!("{n} {text}")),
        }
    }
    let labels: Vec<_> = deliveries
        .iter()
        .filter(|delivery| delivery.client_notification.is_some())
        .map(|delivery| delivery.agent_label.as_str())
        .collect();
    Some((parts.join(" · "), labels.join(", ")))
}

fn toast_event_text(kind: app::state::ToastKind) -> &'static str {
    match kind {
        app::state::ToastKind::NeedsAttention => "needs attention",
        app::state::ToastKind::Finished => "finished",
        app::state::ToastKind::UpdateInstalled => "updated",
    }
}

#[cfg(test)]
mod digest_tests {
    use super::*;
    use crate::app::state::{AgentNotificationDelivery, ToastKind, ToastNotification};

    fn delivery(label: &str, kind: ToastKind, notified: bool) -> AgentNotificationDelivery {
        let toast = ToastNotification {
            kind,
            title: String::new(),
            context: String::new(),
            position: None,
            target: None,
        };
        AgentNotificationDelivery {
            pane_id: PaneId::from_raw(1),
            workspace_id: "w".into(),
            agent_label: label.into(),
            known_agent: None,
            kind,
            toast: None,
            client_notification: notified.then_some(toast),
            sound: None,
        }
    }

    #[test]
    fn digest_counts_each_kind_and_names_agents() {
        let batch = [
            delivery("a", ToastKind::NeedsAttention, true),
            delivery("b", ToastKind::Finished, true),
            delivery("c", ToastKind::NeedsAttention, true),
        ];
        assert_eq!(
            agent_digest(&batch),
            Some(("2 need attention · 1 finished".into(), "a, b, c".into()))
        );
    }

    #[test]
    fn digest_ignores_deliveries_without_a_client_notification() {
        let batch = [
            delivery("a", ToastKind::Finished, true),
            delivery("b", ToastKind::Finished, false),
        ];
        assert_eq!(agent_digest(&batch), None);
        assert_eq!(agent_digest(&[]), None);
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::*;
    #[cfg(unix)]
    use crate::detect::Agent;
    #[cfg(unix)]
    use crate::terminal::TerminalState;

    #[cfg(unix)]
    fn init_repo(path: &std::path::Path) {
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(path)
            .status()
            .unwrap();
        assert!(status.success(), "git init failed for {}", path.display());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn toast_message_uses_live_root_runtime_cwd_label() {
        let mut state = AppState::test_new();
        state
            .workspaces
            .push(crate::workspace::Workspace::test_new("stale"));
        state.ensure_test_terminals();
        let root = state.workspaces[0].tabs[0].root_pane;
        let terminal_id = state.workspaces[0].terminal_id(root).cloned().unwrap();
        let temp_root = std::env::temp_dir().join(format!(
            "herdr-forwarded-toast-context-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let stale_cwd = temp_root.join("__herdr_original__");
        let live_cwd = temp_root.join("__herdr_projects__");
        std::fs::create_dir_all(&stale_cwd).unwrap();
        std::fs::create_dir_all(&live_cwd).unwrap();
        init_repo(&stale_cwd);
        init_repo(&live_cwd);
        state.workspaces[0].custom_name = None;
        state.workspaces[0].identity_cwd = stale_cwd.clone();
        let mut terminal = TerminalState::new(terminal_id.clone(), stale_cwd);
        terminal.set_detected_state(Some(Agent::Codex), AgentState::Idle);
        state.terminals.insert(terminal_id.clone(), terminal);
        let (events, _) = tokio::sync::mpsc::channel(4);
        let runtime = crate::terminal::TerminalRuntime::spawn(
            root,
            24,
            80,
            live_cwd.clone(),
            0,
            crate::terminal_theme::TerminalTheme::default(),
            None,
            crate::pane::PaneShellConfig::new("/bin/sh", crate::config::ShellModeConfig::NonLogin),
            &crate::pane::PaneLaunchEnv::default(),
            events,
            std::sync::Arc::new(tokio::sync::Notify::new()),
            std::sync::Arc::new(crate::render_signal::RenderSignal::new()),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while runtime.cwd() != Some(live_cwd.clone()) && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let mut terminal_runtimes = TerminalRuntimeRegistry::new();
        terminal_runtimes.insert(terminal_id, runtime);

        let message = toast_message_from_state_change(
            &state,
            &terminal_runtimes,
            root,
            false,
            AgentState::Working,
            AgentState::Idle,
        );

        assert_eq!(
            message.as_deref(),
            Some("codex finished: __herdr_projects__ · 1")
        );

        for (_, runtime) in terminal_runtimes.drain() {
            runtime.shutdown();
        }
        let _ = std::fs::remove_dir_all(temp_root);
    }
}

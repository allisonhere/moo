use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ClientGlobalMenuAction {
    Binding(crate::input::KeybindAction),
    Command {
        command_id: String,
        action: crate::protocol::ClientShellCommandAction,
    },
    WhatsNew,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ClientGlobalMenuItem {
    pub(super) label: String,
    pub(super) shortcut: Option<String>,
    pub(super) action: ClientGlobalMenuAction,
}

pub(super) fn global_menu_attention(snapshot: &ClientShellSnapshot) -> bool {
    snapshot.update_available.is_some() || snapshot.integration_updates_available
}

pub(super) fn global_menu_item_has_badge(
    snapshot: &ClientShellSnapshot,
    action: &ClientGlobalMenuAction,
) -> bool {
    (action == &ClientGlobalMenuAction::WhatsNew && snapshot.update_available.is_some())
        || (action == &ClientGlobalMenuAction::Binding(crate::input::KeybindAction::Settings)
            && snapshot.integration_updates_available)
}

pub(super) fn global_menu_items(
    snapshot: &ClientShellSnapshot,
    keybinds: &crate::config::Keybinds,
) -> Vec<ClientGlobalMenuItem> {
    let mut items = [
        ("settings", crate::input::KeybindAction::Settings),
        ("keybinds", crate::input::KeybindAction::Help),
        (
            "session navigator",
            crate::input::KeybindAction::OpenNavigator,
        ),
        ("new workspace", crate::input::KeybindAction::NewWorkspace),
        ("new worktree", crate::input::KeybindAction::NewWorktree),
        ("open worktree", crate::input::KeybindAction::OpenWorktree),
        (
            "split pane right",
            crate::input::KeybindAction::SplitVertical,
        ),
        (
            "split pane down",
            crate::input::KeybindAction::SplitHorizontal,
        ),
        ("reload config", crate::input::KeybindAction::ReloadConfig),
    ]
    .into_iter()
    .map(|(label, action)| ClientGlobalMenuItem {
        label: label.to_owned(),
        shortcut: global_menu_action_shortcut(keybinds, action),
        action: ClientGlobalMenuAction::Binding(action),
    })
    .collect::<Vec<_>>();
    items.extend(snapshot.commands.iter().filter_map(|command| {
        let label = command.description.as_deref()?.trim();
        (!label.is_empty()).then(|| ClientGlobalMenuItem {
            label: label.to_owned(),
            shortcut: (!command.binding_label.is_empty()).then(|| command.binding_label.clone()),
            action: ClientGlobalMenuAction::Command {
                command_id: command.command_id.clone(),
                action: command.action,
            },
        })
    }));
    if snapshot.update_available.is_some() || snapshot.latest_release_notes_available {
        items.push(ClientGlobalMenuItem {
            label: if snapshot.update_available.is_some() {
                "update ready"
            } else {
                "what's new"
            }
            .to_owned(),
            shortcut: None,
            action: ClientGlobalMenuAction::WhatsNew,
        });
    }
    items.push(ClientGlobalMenuItem {
        label: "detach".to_owned(),
        shortcut: global_menu_action_shortcut(keybinds, crate::input::KeybindAction::Detach),
        action: ClientGlobalMenuAction::Binding(crate::input::KeybindAction::Detach),
    });
    items
}

fn global_menu_action_shortcut(
    keybinds: &crate::config::Keybinds,
    action: crate::input::KeybindAction,
) -> Option<String> {
    match action {
        crate::input::KeybindAction::Settings => keybinds.settings.label(),
        crate::input::KeybindAction::Help => keybinds.help.label(),
        crate::input::KeybindAction::OpenNavigator => keybinds.goto.label(),
        crate::input::KeybindAction::NewWorkspace => keybinds.new_workspace.label(),
        crate::input::KeybindAction::NewWorktree => keybinds.new_worktree.label(),
        crate::input::KeybindAction::OpenWorktree => keybinds.open_worktree.label(),
        crate::input::KeybindAction::SplitVertical => keybinds.split_vertical.label(),
        crate::input::KeybindAction::SplitHorizontal => keybinds.split_horizontal.label(),
        crate::input::KeybindAction::ReloadConfig => keybinds.reload_config.label(),
        crate::input::KeybindAction::Detach => keybinds.detach.label(),
        _ => None,
    }
}

pub(super) fn filtered_global_menu_items(
    snapshot: &ClientShellSnapshot,
    keybinds: &crate::config::Keybinds,
    query: &str,
) -> Vec<ClientGlobalMenuItem> {
    let mut items = global_menu_items(snapshot, keybinds)
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            fuzzy_score(&item.label, query).map(|score| (score, index, item))
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    items.into_iter().map(|(_, _, item)| item).collect()
}

fn fuzzy_score(label: &str, query: &str) -> Option<usize> {
    let label: Vec<char> = label.to_ascii_lowercase().chars().collect();
    let query: Vec<char> = query.to_ascii_lowercase().chars().collect();
    if query.is_empty() {
        return Some(0);
    }
    if query.len() > label.len() {
        return None;
    }

    // Choose the best-scoring subsequence instead of the leftmost one: a
    // word-start match ("new **work**space") should outrank an earlier
    // mid-word match ("ne**w** workspace").
    const UNKNOWN: i32 = -1;
    const IMPOSSIBLE: i32 = -2;
    let mut memo = vec![UNKNOWN; (label.len() + 1) * (query.len() + 1) * 2];

    fn best(
        label: &[char],
        query: &[char],
        label_index: usize,
        query_index: usize,
        adjacent: bool,
        memo: &mut [i32],
    ) -> i32 {
        if query_index == query.len() {
            return 0;
        }
        let slot = (label_index * (query.len() + 1) + query_index) * 2 + usize::from(adjacent);
        if memo[slot] != UNKNOWN {
            return memo[slot];
        }
        let mut best_score = IMPOSSIBLE;
        for position in label_index..label.len() {
            if label[position] != query[query_index] {
                continue;
            }
            let rest = best(label, query, position + 1, query_index + 1, true, memo);
            if rest == IMPOSSIBLE {
                continue;
            }
            let word_start = position == 0 || matches!(label[position - 1], ' ' | '-');
            let consecutive = adjacent && position == label_index;
            let score = 1 + if word_start { 8 } else { 0 } + if consecutive { 4 } else { 0 } + rest;
            best_score = best_score.max(score);
        }
        memo[slot] = best_score;
        best_score
    }

    let mut score = best(&label, &query, 0, 0, false, &mut memo);
    if score == IMPOSSIBLE {
        return None;
    }
    if label.starts_with(&query) {
        score += 16;
    }
    Some(score as usize)
}

impl ClientShellState {
    pub(super) fn toggle_global_menu(&mut self) {
        if matches!(self.overlay, Some(ClientShellOverlay::GlobalMenu(_))) {
            self.overlay = None;
        } else {
            self.open_command_palette();
        }
    }

    pub(super) fn open_command_palette(&mut self) {
        self.overlay = Some(ClientShellOverlay::GlobalMenu(ClientGlobalMenuOverlay {
            query: TextEditor::default(),
            highlighted: 0,
        }));
        self.chrome_drag = None;
    }

    pub(super) fn move_global_menu_selection(&mut self, delta: isize) {
        let item_count = self
            .snapshot
            .as_deref()
            .map(|snapshot| {
                let query = match self.overlay.as_ref() {
                    Some(ClientShellOverlay::GlobalMenu(menu)) => menu.query.as_str(),
                    _ => "",
                };
                filtered_global_menu_items(snapshot, &self.config.keybinds.keybinds, query)
            })
            .map_or(0, |items| items.len());
        let Some(ClientShellOverlay::GlobalMenu(menu)) = self.overlay.as_mut() else {
            return;
        };
        menu.highlighted = (menu.highlighted as isize + delta)
            .clamp(0, item_count.saturating_sub(1) as isize) as usize;
    }

    pub(super) fn activate_global_menu_item(
        &mut self,
        index: usize,
        outcome: &mut ClientShellInput,
    ) {
        let query = match self.overlay.as_ref() {
            Some(ClientShellOverlay::GlobalMenu(menu)) => menu.query.as_str(),
            _ => "",
        };
        let Some(action) = self.snapshot.as_deref().and_then(|snapshot| {
            filtered_global_menu_items(snapshot, &self.config.keybinds.keybinds, query)
                .get(index)
                .map(|item| item.action.clone())
        }) else {
            return;
        };
        if action == ClientGlobalMenuAction::WhatsNew
            && self
                .snapshot
                .as_deref()
                .and_then(|snapshot| snapshot.release_notes.as_ref())
                .is_none()
        {
            return;
        }
        self.overlay = None;
        match action {
            ClientGlobalMenuAction::Binding(binding) => {
                self.record_binding(crate::input::KeybindMatch::Action(binding), outcome)
            }
            ClientGlobalMenuAction::Command { command_id, action } => {
                self.invoke_endpoint_command(command_id, action, outcome)
            }
            ClientGlobalMenuAction::WhatsNew => self.open_release_notes(),
        }
        outcome.repaint = true;
    }
}

#[cfg(test)]
mod tests {
    use super::fuzzy_score;

    #[test]
    fn fuzzy_score_accepts_subsequences_and_rejects_missing_characters() {
        assert!(fuzzy_score("new workspace", "nws").is_some());
        assert_eq!(fuzzy_score("new workspace", "nz"), None);
    }

    #[test]
    fn fuzzy_score_prefers_prefix_word_start_and_consecutive_matches() {
        let prefix = fuzzy_score("reload config", "rel").unwrap();
        let word_start = fuzzy_score("config reload", "rel").unwrap();
        let scattered = fuzzy_score("barrel", "rel").unwrap();

        assert!(prefix > word_start);
        assert!(word_start > scattered);
    }
}

use super::*;

use crate::api::schema::{
    Method, PluginActionInfo, PluginActionInvokeParams, PluginActionListParams, ResponseResult,
};

/// Actions this client offers: those the server's host can run. A server that does not report
/// `supported` is treated as able to run everything it lists.
pub(super) fn offered_plugin_actions(actions: Vec<PluginActionInfo>) -> Vec<PluginActionInfo> {
    actions
        .into_iter()
        .filter(|action| action.supported != Some(false))
        .collect()
}

impl ClientShellState {
    pub(super) fn queue_plugin_action_list(&mut self, outcome: &mut ClientShellInput) {
        if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
            settings.loading_plugins = true;
            settings.plugin_notice = None;
        }
        if !self.push_endpoint_method_with_kind(
            Method::PluginActionList(PluginActionListParams::default()),
            PendingEndpointKind::PluginActionList,
            outcome,
        ) {
            if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                settings.loading_plugins = false;
            }
        }
    }

    pub(super) fn run_selected_plugin_action(&mut self, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() else {
            return;
        };
        let Some(action) = settings.plugin_actions.get(settings.selected).cloned() else {
            return;
        };
        settings.plugin_notice = Some(format!("{}…", action.title));
        let method = Method::PluginActionInvoke(PluginActionInvokeParams {
            action_id: action.action_id,
            plugin_id: Some(action.plugin_id),
            context: None,
        });
        if !self.push_endpoint_method_with_kind(
            method,
            PendingEndpointKind::PluginActionInvoke {
                title: action.title,
            },
            outcome,
        ) {
            if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                settings.plugin_notice = None;
            }
        }
    }

    pub(super) fn handle_settings_plugin_result(
        &mut self,
        kind: PendingEndpointKind,
        result: Result<ResponseResult, ClientShellEndpointError>,
    ) -> bool {
        let unexpected = match (&kind, &result) {
            (
                PendingEndpointKind::PluginActionList,
                Ok(ResponseResult::PluginActionList { .. }),
            )
            | (
                PendingEndpointKind::PluginActionInvoke { .. },
                Ok(ResponseResult::PluginActionInvoked { .. }),
            ) => false,
            (_, Ok(_)) => true,
            (_, Err(_)) => false,
        };
        if unexpected {
            self.set_endpoint_error("endpoint returned an unexpected plugin result");
        }
        let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() else {
            return true;
        };
        match (kind, result) {
            (PendingEndpointKind::PluginActionList, result) => {
                settings.loading_plugins = false;
                if let Ok(ResponseResult::PluginActionList { actions }) = result {
                    settings.plugin_actions = offered_plugin_actions(actions);
                    settings.selected = settings
                        .selected
                        .min(settings.plugin_actions.len().saturating_sub(1));
                }
            }
            (PendingEndpointKind::PluginActionInvoke { title }, Ok(_)) if !unexpected => {
                settings.plugin_notice = Some(format!("{title}: started"));
            }
            (_, _) => settings.plugin_notice = None,
        }
        true
    }
}

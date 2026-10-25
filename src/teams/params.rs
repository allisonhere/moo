//! Request parameters for every Teams method. Unknown fields are refused, so a typo is an error and
//! not a silently ignored option; new optional fields arrive only with a new advertised method or a
//! documented additive default.

use schemars::JsonSchema;
use serde::Deserialize;

use super::drafts::DraftInput;
use super::model;

#[derive(Deserialize, Default, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SnapshotParams {
    #[serde(default)]
    pub team_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct TeamRef {
    pub team_id: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct EventsParams {
    pub stream_id: String,
    /// The last sequence you have; you get events after it.
    pub after: u64,
    #[serde(default = "page_limit")]
    pub limit: usize,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct WaitParams {
    pub stream_id: String,
    pub after: u64,
    #[serde(default = "page_limit")]
    pub limit: usize,
    /// How long to hold the request open waiting for an event. 0 polls; capped by the server.
    #[serde(default = "default_wait")]
    pub timeout_ms: u64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct MutationParams {
    pub expected_revision: u64,
    pub idempotency_key: String,
    pub command: model::Command,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct DraftParams {
    pub draft: DraftInput,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct DraftLaunchParams {
    pub expected_revision: u64,
    pub idempotency_key: String,
    pub draft: DraftInput,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ProposeParams {
    pub goal: String,
}

#[derive(Deserialize, Default, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AuditParams {
    #[serde(default)]
    pub limit: Option<usize>,
}

pub(super) fn page_limit() -> usize {
    100
}

fn default_wait() -> u64 {
    20_000
}

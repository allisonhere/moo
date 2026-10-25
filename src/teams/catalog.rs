//! The published shape of the Teams API: which methods exist and since when, what each operation
//! needs, the error catalog, the limits, and a machine-readable schema. Capabilities and
//! `teams.schema.v1` are built from here, so what is advertised is what is implemented.

use schemars::schema_for;
use serde_json::{json, Value};

use super::error::ERRORS;
use super::limits::{MAX_WAITERS, MAX_WAITERS_PER_ACTOR, MAX_WAIT_MS, RATES};
use super::model::{self, Command, Role};
use super::{drafts, pager, params};

/// Bumped when methods or optional fields are added; `api_version` changes only on a break.
pub(super) const API_VERSION: u32 = 1;
pub(super) const API_MINOR: u32 = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Class {
    /// No credential needed.
    Open,
    Read,
    Write,
}

pub(super) struct MethodInfo {
    pub name: &'static str,
    /// The minor version that introduced it.
    pub since: u32,
    pub class: Class,
    pub owner_only: bool,
    pub summary: &'static str,
}

const fn m(
    name: &'static str,
    since: u32,
    class: Class,
    owner_only: bool,
    summary: &'static str,
) -> MethodInfo {
    MethodInfo {
        name,
        since,
        class,
        owner_only,
        summary,
    }
}

pub(super) const METHODS: &[MethodInfo] = &[
    m(
        "teams.capabilities.v1",
        0,
        Class::Open,
        false,
        "Discover methods, operations, limits and errors.",
    ),
    m(
        "teams.validate.v1",
        0,
        Class::Read,
        false,
        "Dry-run one command; nothing is saved.",
    ),
    m(
        "teams.snapshot.get.v1",
        0,
        Class::Read,
        false,
        "Everything the caller may see, with the event watermark.",
    ),
    m(
        "teams.events.list.v1",
        0,
        Class::Read,
        false,
        "A page of events after a cursor.",
    ),
    m(
        "teams.mutate.v1",
        0,
        Class::Write,
        false,
        "Apply one command, idempotently.",
    ),
    m(
        "teams.list.v1",
        1,
        Class::Read,
        false,
        "One line per team the caller may see.",
    ),
    m(
        "teams.get.v1",
        1,
        Class::Read,
        false,
        "A snapshot limited to one team.",
    ),
    m(
        "teams.conflicts.list.v1",
        1,
        Class::Read,
        false,
        "Cross-team conflicts and change notices.",
    ),
    m(
        "teams.integrations.list.v1",
        1,
        Class::Read,
        false,
        "Integration queue, worktrees, conflicts and notices.",
    ),
    m(
        "teams.events.resume.v1",
        1,
        Class::Read,
        false,
        "Alias of events.list for resuming from a cursor.",
    ),
    m(
        "teams.schema.v1",
        2,
        Class::Open,
        false,
        "The machine-readable schema of this API.",
    ),
    m(
        "teams.events.wait.v1",
        2,
        Class::Read,
        false,
        "Like events.list, but holds the request open until there is something to return.",
    ),
    m(
        "teams.messages.list.v1",
        2,
        Class::Read,
        false,
        "Chat messages the caller may read, with cursor pagination.",
    ),
    m(
        "teams.tasks.list.v1",
        2,
        Class::Read,
        false,
        "Tasks with cursor pagination.",
    ),
    m(
        "teams.approvals.list.v1",
        2,
        Class::Read,
        false,
        "Approvals with cursor pagination.",
    ),
    m(
        "teams.decisions.list.v1",
        2,
        Class::Read,
        false,
        "Decision records with cursor pagination.",
    ),
    m(
        "teams.audit.list.v1",
        2,
        Class::Read,
        true,
        "Recent refused requests (Owner only).",
    ),
    m(
        "teams.drafts.propose.v1",
        2,
        Class::Write,
        true,
        "A team draft from a goal (Owner only).",
    ),
    m(
        "teams.drafts.validate.v1",
        2,
        Class::Read,
        true,
        "Check a team draft and preview what launching would do (Owner only).",
    ),
    m(
        "teams.drafts.launch.v1",
        2,
        Class::Write,
        true,
        "Create a team from a draft in one all-or-nothing step (Owner only).",
    ),
];

pub(super) fn method(name: &str) -> Option<&'static MethodInfo> {
    METHODS.iter().find(|info| info.name == name)
}

const OPERATIONS_JSON: &str = include_str!("operations.json");

fn operations_source() -> Vec<Value> {
    serde_json::from_str(OPERATIONS_JSON).unwrap_or_default()
}

/// Every command kind, with who may send it, from the embedded samples and the enforcing code itself.
pub(super) fn operations() -> Vec<Value> {
    operations_source()
        .into_iter()
        .filter_map(|entry| {
            let command: Command = serde_json::from_value(entry["sample"].clone()).ok()?;
            Some(json!({
                "kind": command.kind(),
                "area": entry["area"],
                "summary": entry["summary"],
                "authority": command.authority(),
                "roles": command.permitted_roles().iter().map(|role: &Role| json!(role)).collect::<Vec<_>>(),
                "method": "teams.mutate.v1",
                "sample": entry["sample"],
            }))
        })
        .collect()
}

fn methods_json() -> Vec<Value> {
    METHODS
        .iter()
        .map(|info| {
            json!({
                "name": info.name,
                "since_minor": info.since,
                "auth": info.class != Class::Open,
                "owner_only": info.owner_only,
                "writes": info.class == Class::Write,
                "summary": info.summary,
            })
        })
        .collect()
}

fn errors_json() -> Vec<Value> {
    ERRORS
        .iter()
        .map(|e| json!({"code": e.code, "retryable": e.retryable, "description": e.description}))
        .collect()
}

pub(super) fn limits_json() -> Value {
    json!({
        "max_store_bytes": 16 * 1024 * 1024,
        "max_records": model::MAX_RECORDS,
        "max_message_bytes": model::MAX_MESSAGE_BYTES,
        "max_page_size": pager::MAX_LIMIT,
        "default_page_size": pager::DEFAULT_LIMIT,
        "max_event_page": 100,
        "events_retained_max": model::RETAIN_EVENTS_MAX,
        "events_retained_after_compaction": model::RETAIN_EVENTS_KEEP,
        "idempotency_receipts_kept": model::MAX_RECEIPTS,
        "rate_limits": {
            "read_per_second": RATES.read_per_second, "read_burst": RATES.read_burst,
            "write_per_second": RATES.write_per_second, "write_burst": RATES.write_burst,
            "unauthenticated_per_second": RATES.anonymous_per_second,
            "unauthenticated_burst": RATES.anonymous_burst,
        },
        "event_waits": {
            "max_open": MAX_WAITERS, "max_open_per_actor": MAX_WAITERS_PER_ACTOR, "max_timeout_ms": MAX_WAIT_MS,
        },
    })
}

pub(super) fn capabilities(proposer_configured: bool) -> Value {
    json!({
        "type": "teams_capabilities",
        "api_version": API_VERSION,
        "api_minor": API_MINOR,
        "store_version": 1,
        "methods": METHODS.iter().map(|info| info.name).collect::<Vec<_>>(),
        "method_catalog": methods_json(),
        "operations": operations(),
        "errors": errors_json(),
        "limits": limits_json(),
        "local_only": true,
        "credential_security_supported": crate::platform::private_credentials_supported(),
        "max_store_bytes": 16 * 1024 * 1024, "max_records": model::MAX_RECORDS,
        "max_message_bytes": model::MAX_MESSAGE_BYTES, "max_page_size": 100,
        "adapters": {
            "structured_manual": true, "automatic_dispatch": false,
            "verified_provider_receipts": false,
            "goal_proposals": true, "proposer_configured": proposer_configured,
            "network_gateway": false,
        },
        "command_schema": schema_for!(model::Command),
        "enforcement": {
            "api_authorization": true, "os_isolation": false,
            "credential_broker": false, "worktree_confinement": false,
        },
        "security": "API authorization only; unrestricted same-user shells are not isolated",
    })
}

/// The document `teams.schema.v1` returns and `docs/next/api/teams-api.schema.json` records.
pub(super) fn schema_document() -> Value {
    let params = json!({
        "teams.validate.v1": schema_for!(params::MutationParams),
        "teams.snapshot.get.v1": schema_for!(params::SnapshotParams),
        "teams.events.list.v1": schema_for!(params::EventsParams),
        "teams.events.resume.v1": schema_for!(params::EventsParams),
        "teams.events.wait.v1": schema_for!(params::WaitParams),
        "teams.mutate.v1": schema_for!(params::MutationParams),
        "teams.list.v1": {},
        "teams.get.v1": schema_for!(params::TeamRef),
        "teams.conflicts.list.v1": schema_for!(params::SnapshotParams),
        "teams.integrations.list.v1": schema_for!(params::SnapshotParams),
        "teams.messages.list.v1": schema_for!(pager::ListParams),
        "teams.tasks.list.v1": schema_for!(pager::ListParams),
        "teams.approvals.list.v1": schema_for!(pager::ListParams),
        "teams.decisions.list.v1": schema_for!(pager::ListParams),
        "teams.audit.list.v1": schema_for!(params::AuditParams),
        "teams.drafts.propose.v1": schema_for!(params::ProposeParams),
        "teams.drafts.validate.v1": schema_for!(params::DraftParams),
        "teams.drafts.launch.v1": schema_for!(params::DraftLaunchParams),
    });
    json!({
        "title": "Moo Teams local API",
        "api_version": API_VERSION,
        "api_minor": API_MINOR,
        "transport": "one JSON object per line over the local Moo API socket; one request, one response line",
        "request_envelope": {
            "id": "string, 1-128 bytes, echoed in the response",
            "method": "one of method_catalog[].name",
            "auth": {"bearer": "64 hex characters from a credential file; omit for open methods"},
            "params": "object; see params[method]",
        },
        "response_envelope": {
            "success": {"id": "echoed", "result": "object with a `type` field"},
            "error": {"id": "echoed", "error": {"code": "see errors", "message": "string", "retryable": "bool", "retry_after_ms": "optional", "details": "optional"}},
        },
        "method_catalog": methods_json(),
        "params": params,
        "command": schema_for!(model::Command),
        "operations": operations(),
        "draft": schema_for!(drafts::DraftInput),
        "event": schema_for!(model::Event),
        "errors": errors_json(),
        "limits": limits_json(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_kinds() -> std::collections::BTreeSet<String> {
        let schema = serde_json::to_value(schema_for!(model::Command)).unwrap();
        let mut kinds = std::collections::BTreeSet::new();
        for variant in schema["oneOf"].as_array().unwrap() {
            if let Some(kind) = variant["properties"]["kind"]["const"].as_str() {
                kinds.insert(kind.to_owned());
            }
        }
        kinds
    }

    #[test]
    fn every_command_kind_has_exactly_one_documented_operation() {
        let documented: Vec<String> = operations_source()
            .iter()
            .map(|entry| entry["kind"].as_str().unwrap().to_owned())
            .collect();
        let unique: std::collections::BTreeSet<String> = documented.iter().cloned().collect();
        assert_eq!(unique.len(), documented.len(), "a kind is listed twice");
        assert_eq!(
            unique,
            schema_kinds(),
            "operations.json and the Command enum disagree"
        );
        // Each sample parses, is the kind it claims, and the advertised authority is the enforced one.
        let ops = operations();
        assert_eq!(ops.len(), documented.len(), "a sample no longer parses");
        for (entry, op) in operations_source().iter().zip(&ops) {
            assert_eq!(entry["kind"], op["kind"]);
        }
    }

    #[test]
    fn the_method_catalog_is_unique_versioned_and_consistent_with_capabilities() {
        let mut names = std::collections::BTreeSet::new();
        assert!(METHODS.iter().all(|info| names.insert(info.name)));
        assert!(METHODS
            .iter()
            .all(|info| info.name.starts_with("teams.") && info.name.ends_with(".v1")));
        assert!(METHODS.iter().all(|info| info.since <= API_MINOR));
        assert_eq!(METHODS.iter().map(|i| i.since).max(), Some(API_MINOR));
        let caps = capabilities(false);
        assert_eq!(caps["methods"].as_array().unwrap().len(), METHODS.len());
        assert_eq!(caps["api_version"], 1);
        // The schema documents params for every method that takes any.
        let doc = schema_document();
        for info in METHODS {
            if !matches!(info.name, "teams.capabilities.v1" | "teams.schema.v1") {
                assert!(
                    doc["params"].get(info.name).is_some(),
                    "{} has no params schema",
                    info.name
                );
            }
        }
    }

    #[test]
    fn a_proposer_is_reported_as_configured_only_when_it_is() {
        assert_eq!(capabilities(true)["adapters"]["proposer_configured"], true);
        assert_eq!(
            capabilities(false)["adapters"]["proposer_configured"],
            false
        );
        assert_eq!(capabilities(false)["adapters"]["network_gateway"], false);
    }

    /// The checked-in schema is exactly what this build serves, so docs and code cannot drift.
    /// `UPDATE_TEAMS_SCHEMA=1 cargo test teams::catalog` rewrites it after an intentional, additive change.
    #[test]
    fn the_checked_in_schema_matches_what_the_server_serves() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/docs/next/api/teams-api.schema.json"
        );
        let current = serde_json::to_string_pretty(&schema_document()).unwrap() + "\n";
        if std::env::var_os("UPDATE_TEAMS_SCHEMA").is_some() {
            std::fs::write(path, &current).unwrap();
        }
        let recorded = std::fs::read_to_string(path).unwrap_or_default();
        assert!(
            recorded == current,
            "docs/next/api/teams-api.schema.json is stale; rerun with UPDATE_TEAMS_SCHEMA=1 if the change is additive"
        );
    }
}

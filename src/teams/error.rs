//! Typed errors for the Teams API: a stable machine-readable code, a human message, and, where the
//! caller can act on it, a retry hint or structured details. `ERRORS` is the published catalog.

use serde_json::{json, Value};

#[derive(Debug)]
pub(crate) struct Error {
    pub code: &'static str,
    pub message: String,
    /// Milliseconds the caller should wait before retrying, when waiting can help.
    pub retry_after_ms: Option<u64>,
    /// Structured extras, e.g. where to resume after `cursor_expired`.
    pub details: Option<Value>,
}

impl Error {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after_ms: None,
            details: None,
        }
    }

    pub fn retry_after(mut self, ms: u64) -> Self {
        self.retry_after_ms = Some(ms);
        self
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    /// The `error` object of a response. Old clients read `code` and `message`; the rest is additive.
    pub fn to_value(&self) -> Value {
        let mut error = json!({
            "code": self.code,
            "message": self.message,
            "retryable": info(self.code).is_some_and(|info| info.retryable),
        });
        if let (Some(object), Some(ms)) = (error.as_object_mut(), self.retry_after_ms) {
            object.insert("retry_after_ms".into(), json!(ms));
        }
        if let (Some(object), Some(details)) = (error.as_object_mut(), &self.details) {
            object.insert("details".into(), details.clone());
        }
        error
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::new("storage_error", error.to_string())
    }
}

pub(crate) struct ErrorInfo {
    pub code: &'static str,
    /// The same request may succeed later without changing it.
    pub retryable: bool,
    pub description: &'static str,
}

const fn e(code: &'static str, retryable: bool, description: &'static str) -> ErrorInfo {
    ErrorInfo {
        code,
        retryable,
        description,
    }
}

/// Every code the Teams API can return. A test keeps this in step with the source.
pub(crate) const ERRORS: &[ErrorInfo] = &[
    e("invalid_request", false, "The request envelope is malformed."),
    e("invalid_params", false, "A parameter is missing, malformed or out of range."),
    e("invalid_cursor", false, "A pagination cursor is not one this server issued."),
    e("unknown_method", false, "The server does not advertise this method; check capabilities."),
    e("unauthorized", false, "The credential is missing, invalid or revoked."),
    e("forbidden", false, "The actor's role or team does not allow this."),
    e("not_found", false, "The named record does not exist or is not visible to the actor."),
    e("already_exists", false, "A record with that id exists."),
    e("invalid_reference", false, "A record names something that does not belong to it."),
    e("invalid_state", false, "The record is not in a state that allows this."),
    e("revision_conflict", true, "Read a fresh snapshot, then retry with a new idempotency key."),
    e("idempotency_conflict", false, "The idempotency key was used for a different command."),
    e("rate_limited", true, "Too many requests; wait retry_after_ms."),
    e("too_many_subscriptions", true, "Too many open event waits; retry later or poll."),
    e("cursor_expired", false, "The cursor is older than the retained log; resync from a snapshot (details.retained_from, details.watermark)."),
    e("capacity_exceeded", false, "A hard limit on records or items was reached."),
    e("project_capacity", false, "The project already runs its limit of teams or worktrees."),
    e("team_suspended", false, "The team is suspended; resume it first."),
    e("team_cancelled", false, "The team was cancelled."),
    e("not_approved", false, "No granted approval covers this action."),
    e("approval_expired", false, "The approval's time window has passed."),
    e("approval_used", false, "The approval was already spent."),
    e("approval_mismatch", false, "The action differs from the one approved."),
    e("escalation_required", false, "The request needs an Owner-approved rights escalation."),
    e("deadline_exceeded", false, "The engagement passed its time allocation."),
    e("dependency_blocked", false, "A task dependency is not finished."),
    e("dependency_cycle", false, "Task dependencies would form a cycle."),
    e("stale_handoff", false, "The decision names a handoff that is no longer the latest."),
    e("stale_dispatch", false, "A later dispatch attempt already exists."),
    e("stale_run", false, "The agent's run generation changed."),
    e("retry_exhausted", false, "The dispatch used all its attempts."),
    e("not_ready", false, "The integration needs an accepted review and no open conflict."),
    e("integration_open", false, "An open integration still uses this worktree."),
    e("worktree_conflict", false, "The worktree path or branch overlaps another claim."),
    e("worktree_in_use", false, "The worktree is still active."),
    e("worktree_has_work", false, "The worktree carried submitted work and is preserved."),
    e("capability_missing", false, "The provider is not declared to offer what the role needs."),
    e("task_depth_limit", false, "Dependency chains are limited in depth."),
    e("concurrency_limit", false, "The team is already working on its maximum number of tasks."),
    e("usage_limit_reached", false, "The team reached its reliably metered usage limit; the Owner can raise it."),
    e("agent_idle", false, "The agent is idle or released and takes no new work."),
    e("dispute_settled", false, "That question was already decided and the dispute adds no new evidence."),
    e("unknown_surface", false, "The shared surface is not registered for the project."),
    e("draft_invalid", false, "The team draft has problems; see details.problems."),
    e("proposer_failed", true, "The configured goal proposer failed or timed out."),
    e("unsupported_platform", false, "Teams credentials are not supported on this platform."),
    e("entropy_unavailable", true, "The OS could not supply randomness for a credential."),
    e("invalid_credential_file", false, "The credential file is not a bounded private regular file."),
    e("store_unavailable", true, "The store is temporarily unavailable."),
    e("store_locked", true, "Another process holds the store."),
    e("store_full", false, "The store reached its size limit."),
    e("store_too_large", false, "The store file exceeds the size limit."),
    e("store_poisoned", true, "The store hit an internal error; retry or restart."),
    e("storage_unavailable", true, "A storage operation failed."),
    e("storage_error", true, "An I/O error occurred."),
    e("commit_unknown", false, "A write may or may not have been saved; read a snapshot before retrying."),
    e("corrupt_state", false, "Stored state failed validation."),
    e("not_initialized", false, "The session has no Teams store; run `moo teams init`."),
    e("already_initialized", false, "The store already exists with different contents."),
    e("invalid_store", false, "The store file is not valid."),
    e("invalid_store_lock", false, "The store lock file is not valid."),
    e("invalid_store_root", false, "The store directory is not valid."),
    e("invalid_store_state", false, "The stored state is not valid."),
    e("unsupported_store_version", false, "The store was written by a newer version."),
    e("revision_exhausted", false, "The revision counter is exhausted."),
];

pub(crate) fn info(code: &str) -> Option<&'static ErrorInfo> {
    ERRORS.iter().find(|info| info.code == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every code written in the Teams source is in the catalog, so clients can rely on it.
    #[test]
    fn the_catalog_lists_every_code_the_source_can_return() {
        let mut missing = Vec::new();
        let mut stack = vec![std::path::PathBuf::from("src/teams")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let source = std::fs::read_to_string(&path).unwrap();
                    let marker = ["Error::new(", "storage_error("];
                    for pattern in marker {
                        for part in source.split(pattern).skip(1) {
                            let trimmed = part.trim_start();
                            let Some(rest) = trimmed.strip_prefix('"') else {
                                continue;
                            };
                            let code: String = rest.chars().take_while(|c| *c != '"').collect();
                            if code
                                .chars()
                                .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
                                && !code.is_empty()
                                && info(&code).is_none()
                            {
                                missing.push(format!("{} ({})", code, path.display()));
                            }
                        }
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "codes missing from ERRORS: {missing:?}");
    }

    #[test]
    fn codes_are_unique_and_the_wire_form_carries_the_hints() {
        let mut seen = std::collections::BTreeSet::new();
        assert!(ERRORS.iter().all(|info| seen.insert(info.code)));
        let value = Error::new("rate_limited", "slow down")
            .retry_after(250)
            .to_value();
        assert_eq!(value["retryable"], true);
        assert_eq!(value["retry_after_ms"], 250);
        let expired = Error::new("cursor_expired", "old").with_details(json!({"retained_from": 7}));
        assert_eq!(expired.to_value()["details"]["retained_from"], 7);
        assert_eq!(Error::new("forbidden", "no").to_value()["retryable"], false);
        assert!(Error::new("forbidden", "no")
            .to_value()
            .get("retry_after_ms")
            .is_none());
    }
}

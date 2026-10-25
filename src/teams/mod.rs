//! Local, server-owned collaboration. This namespace is independent of the TUI wire codecs.
mod model;
mod store;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub(crate) struct Error {
    pub code: &'static str,
    pub message: String,
}

impl Error {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
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

const METHODS: &[&str] = &[
    "teams.capabilities.v1",
    "teams.validate.v1",
    "teams.snapshot.get.v1",
    "teams.events.list.v1",
    "teams.mutate.v1",
    "teams.list.v1",
    "teams.get.v1",
    "teams.conflicts.list.v1",
    "teams.integrations.list.v1",
    "teams.events.resume.v1",
];

pub(crate) struct Service {
    root: PathBuf,
    store: Mutex<Option<store::Store>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    id: String,
    method: String,
    #[serde(default)]
    auth: Option<Auth>,
    #[serde(default)]
    params: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Auth {
    bearer: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mutation {
    expected_revision: u64,
    idempotency_key: String,
    command: model::Command,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    #[serde(default)]
    team_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamRef {
    team_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Events {
    stream_id: String,
    after: u64,
    #[serde(default = "page_limit")]
    limit: usize,
}

fn page_limit() -> usize {
    100
}

impl Service {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            store: Mutex::new(None),
        }
    }

    /// Non-Teams requests continue through the unchanged core parser and dispatch.
    pub(crate) fn route(&self, line: &str) -> Option<Value> {
        let peek: Value = serde_json::from_str(line).ok()?;
        let method = peek.get("method")?.as_str()?;
        if !method.starts_with("teams.") {
            return None;
        }
        let id = peek
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let response = match serde_json::from_str::<Envelope>(line) {
            Ok(envelope) if !envelope.id.is_empty() && envelope.id.len() <= 128 => {
                self.handle(envelope)
            }
            _ => Err(Error::new(
                "invalid_request",
                "invalid Teams request envelope",
            )),
        };
        Some(match response {
            Ok(result) => json!({"id": id, "result": result}),
            Err(error) => {
                json!({"id": id, "error": {"code": error.code, "message": error.message}})
            }
        })
    }

    fn handle(&self, envelope: Envelope) -> Result<Value> {
        if !METHODS.contains(&envelope.method.as_str()) {
            return Err(Error::new("unknown_method", "unsupported Teams method"));
        }
        if envelope.method == "teams.capabilities.v1" {
            return Ok(json!({
                "type": "teams_capabilities", "api_version": 1, "store_version": 1,
                "methods": METHODS, "local_only": true,
                "credential_security_supported": crate::platform::private_credentials_supported(),
                "max_store_bytes": 16 * 1024 * 1024, "max_records": 10000,
                "max_message_bytes": 16 * 1024, "max_page_size": 100,
                "adapters": {"structured_manual": true, "automatic_dispatch": false,
                    "verified_provider_receipts": false, "goal_proposals": false},
                "command_schema": schemars::schema_for!(model::Command),
                "enforcement": {"api_authorization": true, "os_isolation": false,
                    "credential_broker": false, "worktree_confinement": false},
                "security": "API authorization only; unrestricted same-user shells are not isolated"
            }));
        }
        ensure_supported()?;
        let bearer = envelope
            .auth
            .ok_or_else(|| Error::new("unauthorized", "Teams credential required"))?
            .bearer;
        validate_token(&bearer)?;
        let digest = credential_digest(&bearer);
        let mut guard = self
            .store
            .lock()
            .map_err(|_| Error::new("store_unavailable", "Teams store lock is unavailable"))?;
        if guard.is_none() {
            let mut store = store::Store::open(&self.root)?;
            store.recover()?;
            *guard = Some(store);
        }
        let store = guard
            .as_mut()
            .ok_or_else(|| Error::new("store_unavailable", "Teams store is unavailable"))?;
        let actor = store.state()?.authenticate(&digest)?;
        match envelope.method.as_str() {
            "teams.snapshot.get.v1" => {
                let params: Snapshot = decode(envelope.params)?;
                store.state()?.snapshot(&actor, params.team_id.as_deref())
            }
            "teams.list.v1" => store.state()?.list_teams(&actor),
            "teams.get.v1" => {
                let params: TeamRef = decode(envelope.params)?;
                store.state()?.snapshot(&actor, Some(&params.team_id))
            }
            "teams.conflicts.list.v1" | "teams.integrations.list.v1" => {
                let params: Snapshot = decode(envelope.params)?;
                let view = store.state()?.snapshot(&actor, params.team_id.as_deref())?;
                let keys: &[&str] = if envelope.method == "teams.conflicts.list.v1" {
                    &["conflicts", "notices"]
                } else {
                    &["integrations", "worktrees", "conflicts", "notices"]
                };
                Ok(json!({
                    "revision": view["revision"],
                    "stream_id": view["stream_id"],
                    "last_event_sequence": view["last_event_sequence"],
                    "data": keys.iter().map(|key| (key.to_string(), view[*key].clone())).collect::<serde_json::Map<_, _>>(),
                }))
            }
            "teams.events.list.v1" | "teams.events.resume.v1" => {
                let params: Events = decode(envelope.params)?;
                if !(1..=100).contains(&params.limit) {
                    return Err(Error::new(
                        "invalid_params",
                        "event page limit must be between 1 and 100",
                    ));
                }
                store
                    .state()?
                    .event_page(&actor, &params.stream_id, params.after, params.limit)
            }
            "teams.mutate.v1" | "teams.validate.v1" => {
                let params: Mutation = decode(envelope.params)?;
                if params.idempotency_key.is_empty() || params.idempotency_key.len() > 128 {
                    return Err(Error::new(
                        "invalid_params",
                        "idempotency key must contain 1 to 128 bytes",
                    ));
                }
                let command_bytes = serde_json::to_vec(&params.command)
                    .map_err(|_| Error::new("invalid_params", "command cannot be encoded"))?;
                let command_digest = format!("{:x}", Sha256::digest(&command_bytes));
                let key =
                    serde_json::to_string(&(actor.id.as_str(), params.idempotency_key.as_str()))
                        .map_err(|_| {
                            Error::new("invalid_params", "request key cannot be encoded")
                        })?;
                if envelope.method == "teams.mutate.v1" {
                    if let Some(receipt) = store.state()?.receipts.get(&key) {
                        if receipt.digest != command_digest {
                            return Err(Error::new(
                                "idempotency_conflict",
                                "request key was already used for a different command",
                            ));
                        }
                        return Ok(receipt.result.clone());
                    }
                }
                if params.expected_revision != store.state()?.revision {
                    return Err(Error::new(
                        "revision_conflict",
                        "read a fresh snapshot before retrying with a new request key",
                    ));
                }
                let mut candidate = store.state()?.clone();
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |elapsed| elapsed.as_secs());
                let outcome = candidate.execute_at(&actor, params.command, now)?;
                let result = json!({"type": "teams_mutation", "revision": candidate.revision,
                    "stream_id": candidate.stream_id, "watermark": candidate.next_sequence.saturating_sub(1),
                    "outcome": outcome});
                if envelope.method == "teams.validate.v1" {
                    candidate.validate()?;
                    return Ok(
                        json!({"type": "teams_validation", "valid": true, "preview": result}),
                    );
                }
                candidate.receipts.insert(
                    key,
                    model::StoredReceipt {
                        digest: command_digest,
                        result: result.clone(),
                    },
                );
                candidate.validate()?;
                store.commit(candidate)?;
                Ok(result)
            }
            _ => Err(Error::new("unknown_method", "unsupported Teams method")),
        }
    }
}

fn decode<T: serde::de::DeserializeOwned>(params: Value) -> Result<T> {
    // Parser diagnostics may contain bearer strings or message bodies. Never echo them.
    serde_json::from_value(params)
        .map_err(|_| Error::new("invalid_params", "invalid parameters for this Teams method"))
}

fn ensure_supported() -> Result<()> {
    if !crate::platform::private_credentials_supported() {
        return Err(Error::new(
            "unsupported_platform",
            "Teams private credential validation is unavailable on this platform",
        ));
    }
    Ok(())
}

fn validate_token(token: &str) -> Result<()> {
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::new("unauthorized", "invalid Teams credential"));
    }
    Ok(())
}

fn credential_digest(token: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"moo-teams-credential-v1\0");
    digest.update(token.as_bytes());
    format!("{:x}", digest.finalize())
}

pub(crate) fn create_credential(path: &Path) -> Result<String> {
    ensure_supported()?;
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|_| {
        Error::new(
            "entropy_unavailable",
            "could not generate a Teams credential",
        )
    })?;
    let token = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let mut file = crate::platform::create_private_state_file(path)?;
    crate::platform::validate_private_file(&file)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    crate::platform::sync_parent_directory(path.parent().unwrap_or_else(|| Path::new(".")))?;
    Ok(credential_digest(&token))
}

pub(crate) fn read_credential(path: &Path) -> Result<String> {
    ensure_supported()?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 128 {
        return Err(Error::new(
            "invalid_credential_file",
            "credential must be a bounded private regular file",
        ));
    }
    let file = std::fs::File::open(path)?;
    crate::platform::validate_private_file(&file)?;
    let mut token = String::new();
    file.take(129).read_to_string(&mut token)?;
    let token = token.trim().to_owned();
    validate_token(&token)?;
    Ok(token)
}

pub(crate) fn initialize(root: &Path) -> Result<PathBuf> {
    ensure_supported()?;
    std::fs::create_dir_all(root)?;
    let path = root.join("teams-owner.key");
    match create_credential(&path) {
        Ok(_) => {}
        Err(error) if path.exists() && error.code == "storage_error" => {
            read_credential(&path)?;
        }
        Err(error) => return Err(error),
    }
    let token = read_credential(&path)?;
    std::fs::File::open(&path)?.sync_all()?;
    crate::platform::sync_parent_directory(root)?;
    let owner_digest = credential_digest(&token);
    let mut stream = Sha256::new();
    stream.update(b"moo-teams-stream-v1\0");
    stream.update(token.as_bytes());
    let stream_id = format!("{:x}", stream.finalize());
    store::bootstrap(root, owner_digest, stream_id)?;
    Ok(path)
}

#[cfg(test)]
mod tests;

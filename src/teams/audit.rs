//! A bounded log of refused requests. Accepted changes are already in the hash-chained event log;
//! this covers what that cannot: bad credentials, forbidden actions, rate limits, spent approvals.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use serde_json::{json, Value};

const MAX_BYTES: u64 = 256 * 1024;
const FILE: &str = "teams-audit.log";
/// Codes worth a line: someone asked for something they were not allowed or not able to have.
const DENIALS: &[&str] = &[
    "unauthorized",
    "forbidden",
    "rate_limited",
    "too_many_subscriptions",
    "not_approved",
    "approval_expired",
    "approval_used",
    "approval_mismatch",
    "escalation_required",
];

pub(crate) struct DenialLog {
    path: PathBuf,
    /// Floods are summarized: one line per actor, method and code per second.
    last: HashMap<String, u64>,
}

impl DenialLog {
    pub(crate) fn new(root: &std::path::Path) -> Self {
        Self {
            path: root.join(FILE),
            last: HashMap::new(),
        }
    }

    pub(crate) fn record(&mut self, actor: &str, method: &str, code: &str, now: u64) {
        if !DENIALS.contains(&code) {
            return;
        }
        let key = format!("{actor}\0{method}\0{code}");
        if self.last.get(&key) == Some(&now) {
            return;
        }
        if self.last.len() > 1024 {
            self.last.clear();
        }
        self.last.insert(key, now);
        // Best effort: an audit write must never turn a refusal into a different failure.
        let _ = self.append(&json!({ "t": now, "actor": actor, "method": method, "code": code }));
    }

    fn append(&self, line: &Value) -> std::io::Result<()> {
        if std::fs::metadata(&self.path).is_ok_and(|meta| meta.len() > MAX_BYTES) {
            let old = self.path.with_extension("log.1");
            let _ = std::fs::rename(&self.path, old);
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&self.path)?;
        writeln!(file, "{line}")
    }

    /// The newest `limit` lines, oldest first.
    pub(crate) fn recent(&self, limit: usize) -> Vec<Value> {
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return Vec::new();
        };
        let len = file.metadata().map_or(0, |meta| meta.len());
        let start = len.saturating_sub(MAX_BYTES);
        let mut text = String::new();
        if file.seek(SeekFrom::Start(start)).is_err()
            || file.take(MAX_BYTES).read_to_string(&mut text).is_err()
        {
            return Vec::new();
        }
        let mut lines: Vec<Value> = text
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let skip = lines.len().saturating_sub(limit);
        lines.drain(..skip);
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("moo-audit-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn denials_are_recorded_and_the_rest_is_not() {
        let dir = temp("a");
        let mut log = DenialLog::new(&dir);
        log.record("dev", "teams.mutate.v1", "forbidden", 1);
        log.record("dev", "teams.mutate.v1", "not_found", 1);
        log.record("-", "teams.snapshot.get.v1", "unauthorized", 2);
        let lines = log.recent(10);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            (lines[0]["actor"].as_str(), lines[0]["code"].as_str()),
            (Some("dev"), Some("forbidden"))
        );
        assert_eq!(lines[1]["t"], 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_flood_is_summarized_and_the_file_stays_bounded() {
        let dir = temp("b");
        let mut log = DenialLog::new(&dir);
        for _ in 0..500 {
            log.record("noisy", "m", "rate_limited", 7);
        }
        assert_eq!(
            log.recent(1000).len(),
            1,
            "one line per second per actor/method/code"
        );
        for second in 0..20_000u64 {
            log.record("noisy", "m", "rate_limited", 100 + second);
        }
        let size = std::fs::metadata(dir.join(FILE)).unwrap().len();
        assert!(size <= MAX_BYTES + 200, "{size}");
        assert!(log.recent(5).len() == 5);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("c");
        let mut log = DenialLog::new(&dir);
        log.record("x", "m", "forbidden", 1);
        let mode = std::fs::metadata(dir.join(FILE))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

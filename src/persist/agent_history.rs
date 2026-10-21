use std::path::PathBuf;

use crate::api::schema::AgentHistoryEntry;

pub(crate) const CAPACITY: usize = 500;

/// Stored beside the session file; survives restart and reboot, and is kept when the
/// workspace list is empty.
fn path() -> PathBuf {
    super::io::session_path().with_file_name("agent-history.json")
}

pub fn load() -> Vec<AgentHistoryEntry> {
    let path = path();
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            tracing::warn!(path = %path.display(), err = %err, "failed to read agent history");
            return Vec::new();
        }
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|err| {
        tracing::warn!(path = %path.display(), err = %err, "ignoring unreadable agent history");
        Vec::new()
    })
}

pub(crate) fn save(entries: &[AgentHistoryEntry]) {
    let path = path();
    if let Err(err) = super::io::save_json_to_path(&path, &entries) {
        tracing::warn!(path = %path.display(), err = %err, "failed to save agent history");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::AgentStatus;

    #[test]
    fn entries_round_trip_and_unreadable_file_yields_empty() {
        let _guard = crate::config::test_config_env_lock().lock().unwrap();
        let dir = std::env::temp_dir().join(format!("herdr-agent-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let saved = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", &dir);

        assert!(load().is_empty());
        let entry = AgentHistoryEntry {
            at_ms: 7,
            pane_id: "w1:p1".into(),
            workspace_id: "w1".into(),
            agent: Some("claude".into()),
            from: AgentStatus::Working,
            to: AgentStatus::Blocked,
            acked: false,
        };
        save(std::slice::from_ref(&entry));
        assert_eq!(load(), vec![entry]);

        std::fs::write(path(), b"not json").unwrap();
        assert!(load().is_empty());

        match saved {
            Some(saved) => std::env::set_var("XDG_CONFIG_HOME", saved),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

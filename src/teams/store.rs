use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::model::State;
use super::{Error, Result};

const STORE_FILE: &str = "teams-v1.json";
const LOCK_FILE: &str = ".teams-v1.lock";
const STORE_VERSION: u32 = 1;
const MAX_STORE_BYTES: usize = 16 * 1024 * 1024;
const TEMP_ATTEMPTS: u64 = 128;

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(1);

/// The one process-local view of the authoritative Teams store.
///
/// `_lock` owns the cross-process exclusive lock for the full lifetime of the
/// loaded state. `state` becomes `None` only when an uncertain commit cannot be
/// read back safely.
pub struct Store {
    path: PathBuf,
    _lock: File,
    state: Option<State>,
    poison: Option<String>,
}

impl Store {
    /// Opens an initialized store and takes its non-blocking exclusive lock.
    /// Missing state is not initialization; callers must run [`bootstrap`].
    pub fn open(root: &Path) -> Result<Self> {
        validate_root(root, false)?;
        let lock = acquire_lock(root)?;
        let path = root.join(STORE_FILE);
        let state = load_state(&path)?;
        Ok(Self {
            path,
            _lock: lock,
            state: Some(state),
            poison: None,
        })
    }

    /// Returns the last state proven to match the authoritative file.
    pub fn state(&self) -> Result<&State> {
        if let Some(reason) = &self.poison {
            return Err(Error::new(
                "store_poisoned",
                format!("Teams store state is uncertain: {reason}"),
            ));
        }
        self.state.as_ref().ok_or_else(|| {
            Error::new(
                "store_poisoned",
                "Teams store has no verified in-memory state".to_string(),
            )
        })
    }

    /// Publishes a validated next state.
    ///
    /// Errors before replacement leave both the file and memory at the old
    /// revision. If syncing the parent fails after replacement, this method
    /// reloads the target or poisons the store, then returns `commit_unknown`.
    pub fn commit(&mut self, candidate: State) -> Result<()> {
        candidate.validate()?;
        validate_store_version(candidate.version)?;

        let current = self.state()?;
        if candidate.stream_id != current.stream_id {
            return Err(Error::new(
                "invalid_store_state",
                "candidate state changes the Teams stream id".to_string(),
            ));
        }
        let expected_revision = current.revision.checked_add(1).ok_or_else(|| {
            Error::new(
                "revision_exhausted",
                "Teams store revision is exhausted".to_string(),
            )
        })?;
        if candidate.revision != expected_revision {
            return Err(Error::new(
                "invalid_store_state",
                format!(
                    "candidate revision {} must follow current revision {}",
                    candidate.revision, current.revision
                ),
            ));
        }

        let bytes = encode_state(&candidate)?;
        ensure_regular_file(&self.path)?;
        match publish(&self.path, &bytes, false) {
            Ok(()) => {
                self.state = Some(candidate);
                self.poison = None;
                Ok(())
            }
            Err(PublishError::Before(error)) => Err(storage_error(
                "storage_unavailable",
                "failed to publish Teams store",
                error,
            )),
            Err(PublishError::After(error)) => {
                let reload = load_state(&self.path);
                match reload {
                    Ok(state) => {
                        self.state = Some(state);
                        self.poison = None;
                    }
                    Err(reload_error) => {
                        let message = reload_error.message.clone();
                        self.state = None;
                        self.poison = Some(message);
                    }
                }
                Err(Error::new(
                    "commit_unknown",
                    format!(
                        "Teams store replacement completed, but its directory sync failed: {error}"
                    ),
                ))
            }
        }
    }

    /// Converts crash-left `attempted` dispatches to `unknown` once per server
    /// boot. It never resends an external effect.
    pub(crate) fn recover(&mut self) -> Result<()> {
        let mut candidate = self.state()?.clone();
        if !candidate.recover()? {
            return Ok(());
        }
        self.commit(candidate)
    }
}

/// Creates the initial state. An exact pristine retry is idempotent; every
/// other existing target is preserved and rejected.
pub fn bootstrap(root: &Path, owner_digest: String, stream_id: String) -> Result<()> {
    validate_root(root, true)?;
    let _lock = acquire_lock(root)?;
    let path = root.join(STORE_FILE);
    let initial = State::new(stream_id, owner_digest);
    initial.validate()?;
    validate_store_version(initial.version)?;

    match std::fs::symlink_metadata(&path) {
        Ok(_) => {
            let existing = load_state(&path)?;
            if states_match(&existing, &initial)? {
                return Ok(());
            }
            return Err(Error::new(
                "already_initialized",
                format!("Teams store already exists at {}", path.display()),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(storage_error(
                "storage_unavailable",
                "failed to inspect Teams store",
                error,
            ));
        }
    }

    let bytes = encode_state(&initial)?;
    match publish(&path, &bytes, true) {
        Ok(()) => Ok(()),
        Err(PublishError::Before(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // A compliant writer cannot race while `_lock` is held. Preserve an
            // unexpected target so the caller can inspect it.
            Err(Error::new(
                "already_initialized",
                format!("Teams store appeared at {}", path.display()),
            ))
        }
        Err(PublishError::Before(error)) => Err(storage_error(
            "storage_unavailable",
            "failed to initialize Teams store",
            error,
        )),
        Err(PublishError::After(error)) => Err(Error::new(
            "commit_unknown",
            format!(
                "Teams store initialization was replaced, but its directory sync failed: {error}"
            ),
        )),
    }
}

fn validate_root(root: &Path, create: bool) -> Result<()> {
    match std::fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            Ok(())
        }
        Ok(_) => Err(Error::new(
            "invalid_store_root",
            format!(
                "Teams store root must be a local directory, not a link: {}",
                root.display()
            ),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
            crate::platform::create_remote_private_dir(root).map_err(|error| {
                storage_error(
                    "storage_unavailable",
                    "failed to create private Teams store directory",
                    error,
                )
            })?;
            validate_root(root, false)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(Error::new(
            "not_initialized",
            format!("Teams store is not initialized at {}", root.display()),
        )),
        Err(error) => Err(storage_error(
            "storage_unavailable",
            "failed to inspect Teams store directory",
            error,
        )),
    }
}

fn acquire_lock(root: &Path) -> Result<File> {
    let path = root.join(LOCK_FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(Error::new(
                "invalid_store_lock",
                format!("Teams store lock is not a regular file: {}", path.display()),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match crate::platform::create_private_state_file(&path) {
                Ok(file) => drop(file),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(storage_error(
                        "storage_unavailable",
                        "failed to create Teams store lock",
                        error,
                    ));
                }
            }
        }
        Err(error) => {
            return Err(storage_error(
                "storage_unavailable",
                "failed to inspect Teams store lock",
                error,
            ));
        }
    }

    // Recheck after the create/open race. Same-user adversarial replacement is
    // outside the local socket trust boundary, but links still fail closed.
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
        storage_error(
            "storage_unavailable",
            "failed to inspect Teams store lock",
            error,
        )
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(Error::new(
            "invalid_store_lock",
            format!("Teams store lock is not a regular file: {}", path.display()),
        ));
    }

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| {
            storage_error(
                "storage_unavailable",
                "failed to open Teams store lock",
                error,
            )
        })?;
    if !file
        .metadata()
        .map(|value| value.is_file())
        .unwrap_or(false)
    {
        return Err(Error::new(
            "invalid_store_lock",
            format!("Teams store lock is not a regular file: {}", path.display()),
        ));
    }
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(Error::new(
            "store_locked",
            format!("another process owns the Teams store at {}", root.display()),
        )),
        Err(TryLockError::Error(error)) => Err(storage_error(
            "storage_unavailable",
            "failed to lock Teams store",
            error,
        )),
    }
}

fn load_state(path: &Path) -> Result<State> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::new(
                "not_initialized",
                format!("Teams store is not initialized at {}", path.display()),
            ));
        }
        Err(error) => {
            return Err(storage_error(
                "storage_unavailable",
                "failed to inspect Teams store",
                error,
            ));
        }
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(Error::new(
            "invalid_store",
            format!("Teams store is not a regular file: {}", path.display()),
        ));
    }
    if metadata.len() > MAX_STORE_BYTES as u64 {
        return Err(store_too_large(path, metadata.len()));
    }

    let mut file = File::open(path).map_err(|error| {
        storage_error("storage_unavailable", "failed to open Teams store", error)
    })?;
    let opened = file.metadata().map_err(|error| {
        storage_error(
            "storage_unavailable",
            "failed to inspect open Teams store",
            error,
        )
    })?;
    if !opened.is_file() {
        return Err(Error::new(
            "invalid_store",
            format!("Teams store is not a regular file: {}", path.display()),
        ));
    }
    if opened.len() > MAX_STORE_BYTES as u64 {
        return Err(store_too_large(path, opened.len()));
    }

    let mut bytes = Vec::with_capacity(opened.len() as usize);
    Read::by_ref(&mut file)
        .take((MAX_STORE_BYTES as u64) + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            storage_error("storage_unavailable", "failed to read Teams store", error)
        })?;
    if bytes.len() > MAX_STORE_BYTES {
        return Err(store_too_large(path, bytes.len() as u64));
    }

    let state: State = serde_json::from_slice(&bytes).map_err(|error| {
        Error::new(
            "invalid_store",
            format!(
                "Teams store at {} is corrupt and was preserved: {error}",
                path.display()
            ),
        )
    })?;
    validate_store_version(state.version)?;
    state.validate()?;
    Ok(state)
}

fn validate_store_version(version: u32) -> Result<()> {
    if version == STORE_VERSION {
        return Ok(());
    }
    Err(Error::new(
        "unsupported_store_version",
        format!(
            "Teams store version {version} is unsupported; this binary supports {STORE_VERSION}"
        ),
    ))
}

fn encode_state(state: &State) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(state).map_err(|error| {
        Error::new(
            "storage_unavailable",
            format!("failed to encode Teams store: {error}"),
        )
    })?;
    if bytes.len() > MAX_STORE_BYTES {
        return Err(Error::new(
            "store_full",
            format!(
                "Teams store would use {} bytes, above the {MAX_STORE_BYTES}-byte limit",
                bytes.len()
            ),
        ));
    }
    Ok(bytes)
}

fn states_match(left: &State, right: &State) -> Result<bool> {
    let left = serde_json::to_value(left).map_err(|error| {
        Error::new(
            "storage_unavailable",
            format!("failed to compare Teams bootstrap state: {error}"),
        )
    })?;
    let right = serde_json::to_value(right).map_err(|error| {
        Error::new(
            "storage_unavailable",
            format!("failed to compare Teams bootstrap state: {error}"),
        )
    })?;
    Ok(left == right)
}

fn ensure_regular_file(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        storage_error(
            "storage_unavailable",
            "failed to inspect Teams store before commit",
            error,
        )
    })?;
    if metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(Error::new(
            "invalid_store",
            format!("Teams store is not a regular file: {}", path.display()),
        ))
    }
}

enum PublishError {
    Before(std::io::Error),
    After(std::io::Error),
}

fn publish(
    destination: &Path,
    bytes: &[u8],
    destination_must_be_missing: bool,
) -> std::result::Result<(), PublishError> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let (temporary_path, mut temporary) = create_temporary(destination)?;
    let mut cleanup = TemporaryCleanup::new(temporary_path.clone());

    injected_failure(FailureStage::Write).map_err(PublishError::Before)?;
    temporary.write_all(bytes).map_err(PublishError::Before)?;
    injected_failure(FailureStage::FileSync).map_err(PublishError::Before)?;
    temporary.sync_all().map_err(PublishError::Before)?;
    drop(temporary);

    if destination_must_be_missing {
        match std::fs::symlink_metadata(destination) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(PublishError::Before(error)),
            Ok(_) => {
                return Err(PublishError::Before(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "Teams store already exists",
                )));
            }
        }
    }

    injected_failure(FailureStage::Replace).map_err(PublishError::Before)?;
    crate::platform::replace_file(&temporary_path, destination).map_err(PublishError::Before)?;
    cleanup.published = true;

    injected_failure(FailureStage::DirectorySync).map_err(PublishError::After)?;
    crate::platform::sync_parent_directory(parent).map_err(PublishError::After)
}

fn create_temporary(destination: &Path) -> std::result::Result<(PathBuf, File), PublishError> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(STORE_FILE);
    for _ in 0..TEMP_ATTEMPTS {
        let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".{name}.tmp-{}-{sequence}", std::process::id()));
        match crate::platform::create_private_state_file(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(PublishError::Before(error)),
        }
    }
    Err(PublishError::Before(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique Teams store temporary",
    )))
}

struct TemporaryCleanup {
    path: PathBuf,
    published: bool,
}

impl TemporaryCleanup {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            published: false,
        }
    }
}

impl Drop for TemporaryCleanup {
    fn drop(&mut self) {
        if !self.published {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn store_too_large(path: &Path, size: u64) -> Error {
    Error::new(
        "store_too_large",
        format!(
            "Teams store at {} uses {size} bytes, above the {MAX_STORE_BYTES}-byte limit",
            path.display()
        ),
    )
}

fn storage_error(code: &'static str, context: &str, error: std::io::Error) -> Error {
    Error::new(code, format!("{context}: {error}"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailureStage {
    Write,
    FileSync,
    Replace,
    DirectorySync,
}

#[cfg(not(test))]
fn injected_failure(_stage: FailureStage) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
thread_local! {
    static INJECTED_FAILURE: std::cell::Cell<Option<FailureStage>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn injected_failure(stage: FailureStage) -> std::io::Result<()> {
    INJECTED_FAILURE.with(|failure| {
        if failure.get() == Some(stage) {
            failure.set(None);
            Err(std::io::Error::other(format!("injected {stage:?} failure")))
        } else {
            Ok(())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(1);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let sequence = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "moo-teams-store-{label}-{}-{sequence}",
                std::process::id()
            ));
            crate::platform::create_remote_private_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn owner_digest() -> String {
        "a".repeat(64)
    }

    fn stream_id() -> String {
        "b".repeat(32)
    }

    fn bootstrap_test_store(root: &Path) {
        bootstrap(root, owner_digest(), stream_id()).unwrap();
    }

    fn next_state(store: &Store) -> State {
        let mut state = store.state().unwrap().clone();
        state.revision += 1;
        state
    }

    fn fail_at(stage: FailureStage) {
        INJECTED_FAILURE.with(|failure| failure.set(Some(stage)));
    }

    #[test]
    fn missing_store_is_not_initialized_and_bootstrap_retry_is_exact() {
        let root = TestDir::new("bootstrap");
        let error = Store::open(root.path()).err().unwrap();
        assert_eq!(error.code, "not_initialized");

        bootstrap_test_store(root.path());
        bootstrap_test_store(root.path());
        let state = Store::open(root.path()).unwrap();
        assert_eq!(state.state().unwrap().stream_id, stream_id());
        drop(state); // An open store holds the lock; bootstrap must reach its own check.

        let error = bootstrap(root.path(), "c".repeat(64), stream_id())
            .err()
            .unwrap();
        assert_eq!(error.code, "already_initialized");
    }

    #[test]
    fn commit_survives_close_and_reopen() {
        let root = TestDir::new("restart");
        bootstrap_test_store(root.path());
        {
            let mut store = Store::open(root.path()).unwrap();
            store.commit(next_state(&store)).unwrap();
            assert_eq!(store.state().unwrap().revision, 1);
        }
        let store = Store::open(root.path()).unwrap();
        assert_eq!(store.state().unwrap().revision, 1);
    }

    #[test]
    fn lock_contention_is_nonblocking() {
        let root = TestDir::new("lock");
        bootstrap_test_store(root.path());
        let _owner = Store::open(root.path()).unwrap();
        let error = Store::open(root.path()).err().unwrap();
        assert_eq!(error.code, "store_locked");
    }

    #[test]
    fn corrupt_bytes_are_preserved() {
        let root = TestDir::new("corrupt");
        bootstrap_test_store(root.path());
        let path = root.path().join(STORE_FILE);
        let corrupt = b"{not-json";
        std::fs::write(&path, corrupt).unwrap();

        let error = Store::open(root.path()).err().unwrap();
        assert_eq!(error.code, "invalid_store");
        assert_eq!(std::fs::read(path).unwrap(), corrupt);
    }

    #[test]
    fn newer_store_is_preserved() {
        let root = TestDir::new("newer");
        bootstrap_test_store(root.path());
        let path = root.path().join(STORE_FILE);
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["version"] = serde_json::json!(STORE_VERSION + 1);
        let newer = serde_json::to_vec(&value).unwrap();
        std::fs::write(&path, &newer).unwrap();

        let error = Store::open(root.path()).err().unwrap();
        assert_eq!(error.code, "unsupported_store_version");
        assert_eq!(std::fs::read(path).unwrap(), newer);
    }

    #[test]
    fn oversized_store_is_rejected_without_parsing() {
        let root = TestDir::new("cap");
        bootstrap_test_store(root.path());
        let path = root.path().join(STORE_FILE);
        let oversized = vec![b' '; MAX_STORE_BYTES + 1];
        std::fs::write(&path, &oversized).unwrap();

        let error = Store::open(root.path()).err().unwrap();
        assert_eq!(error.code, "store_too_large");
        assert_eq!(
            std::fs::metadata(path).unwrap().len(),
            oversized.len() as u64
        );
    }

    #[test]
    fn failures_before_replace_leave_old_disk_and_memory() {
        for stage in [
            FailureStage::Write,
            FailureStage::FileSync,
            FailureStage::Replace,
        ] {
            let root = TestDir::new(&format!("before-{stage:?}"));
            bootstrap_test_store(root.path());
            let mut store = Store::open(root.path()).unwrap();
            let old_bytes = std::fs::read(root.path().join(STORE_FILE)).unwrap();

            fail_at(stage);
            let error = store.commit(next_state(&store)).unwrap_err();
            assert_eq!(error.code, "storage_unavailable");
            assert_eq!(store.state().unwrap().revision, 0);
            assert_eq!(
                std::fs::read(root.path().join(STORE_FILE)).unwrap(),
                old_bytes
            );
        }
    }

    #[test]
    fn directory_sync_failure_reloads_new_state_and_reports_unknown() {
        let root = TestDir::new("dir-sync");
        bootstrap_test_store(root.path());
        let mut store = Store::open(root.path()).unwrap();

        fail_at(FailureStage::DirectorySync);
        let error = store.commit(next_state(&store)).unwrap_err();
        assert_eq!(error.code, "commit_unknown");
        assert_eq!(store.state().unwrap().revision, 1);
        drop(store);

        let reopened = Store::open(root.path()).unwrap();
        assert_eq!(reopened.state().unwrap().revision, 1);
    }
}

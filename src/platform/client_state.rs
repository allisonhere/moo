use std::path::Path;

#[cfg(not(windows))]
pub(crate) fn create_private_state_file(path: &Path) -> std::io::Result<std::fs::File> {
    super::create_remote_ssh_config_file(path)
}

#[cfg(windows)]
pub(crate) fn create_private_state_file(path: &Path) -> std::io::Result<std::fs::File> {
    super::windows::create_remote_ssh_config_file(path)
}

#[cfg(not(windows))]
pub(crate) fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
pub(crate) fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    super::windows::replace_file(source, destination)
}

#[cfg(not(windows))]
pub(crate) fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
pub(crate) fn sync_parent_directory(_path: &Path) -> std::io::Result<()> {
    // replace_file uses MOVEFILE_WRITE_THROUGH on Windows.
    Ok(())
}

/// Credential access fails closed on platforms whose ACL validation is not implemented.
pub(crate) fn private_credentials_supported() -> bool {
    cfg!(unix)
}

#[cfg(unix)]
pub(crate) fn validate_private_file(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "credential must be a private regular file owned by the current user",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn validate_private_file(_file: &std::fs::File) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Teams credential ACL validation is not available on this platform",
    ))
}

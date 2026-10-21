//! Product identity for this downstream fork.

pub const NAME: &str = "Herdr Forge";
pub const BINARY_NAME: &str = "herdr-forge";
pub const APP_DIR_NAME: &str = "herdr-forge";
pub const DEV_APP_DIR_NAME: &str = "herdr-forge-dev";

pub const fn supports_upstream_self_update() -> bool {
    false
}

pub const fn app_dir_name() -> &'static str {
    if cfg!(debug_assertions) {
        DEV_APP_DIR_NAME
    } else {
        APP_DIR_NAME
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fork_identity_cannot_collide_with_upstream_install() {
        assert_ne!(super::BINARY_NAME, "herdr");
        assert_ne!(super::APP_DIR_NAME, "herdr");
        assert_ne!(super::DEV_APP_DIR_NAME, "herdr-dev");
        assert!(!super::supports_upstream_self_update());
    }
}

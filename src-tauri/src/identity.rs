//! Fork-specific persistent identity. Do not fall back to or migrate Deep data.
use std::path::{Path, PathBuf};

pub const PRODUCT_NAME: &str = "Deep Lite";
pub const DATA_DIR_NAME: &str = "DeepLite";
// Match the NSIS product name so uninstall removes only our opted-in Run value.
pub const RUN_VALUE_NAME: &str = PRODUCT_NAME;
// Intentionally shared with upstream: either application blocks a second
// overlay before settings, logging or autostart reconciliation can run.
pub const SHARED_MUTEX_NAME: &str = r"Local\DeepSingleInstance";

pub fn data_dir_at(config_root: &Path) -> PathBuf {
    config_root.join(DATA_DIR_NAME)
}

pub fn data_dir() -> PathBuf {
    data_dir_at(&dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")))
}

// A normal first launch has no registry side effect. Explicit settings changes
// can still remove this fork's own Run value when the user turns autostart off.
pub fn reconcile_autostart_on_launch(enabled: bool) -> bool {
    enabled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_identity_is_separate_and_matches_visible_product() {
        let config: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["productName"], PRODUCT_NAME);
        assert_eq!(config["identifier"], "io.github.zufallupon.deeplite");
        assert_ne!(config["identifier"], "work.brycelewis.deep");
        assert_eq!(config["app"]["windows"][0]["title"], PRODUCT_NAME);
        assert_ne!(RUN_VALUE_NAME, "Deep");
        assert_eq!(RUN_VALUE_NAME, config["productName"].as_str().unwrap());
        assert_ne!(DATA_DIR_NAME, "Deep");
    }

    #[test]
    fn default_launch_does_not_request_registry_reconciliation() {
        assert!(!reconcile_autostart_on_launch(crate::settings::AppSettings::default().start_on_login));
        assert!(reconcile_autostart_on_launch(true));
    }

    #[test]
    fn logs_and_settings_share_only_the_fork_directory() {
        let root = Path::new("test-config-root");
        let fork = data_dir_at(root);
        assert_eq!(fork, root.join("DeepLite"));
        assert_ne!(fork, root.join("Deep"));
        assert_eq!(crate::logging::log_path_at(root).parent(), Some(fork.as_path()));
    }
}

use serde::{Deserialize, Serialize};

fn default_fade_duration_secs() -> f64 { 0.75 }
fn default_gpu_blur_intensity() -> f64 { 0.3 }
fn default_effect_renderer() -> String { "mask".into() }
fn default_mask_pattern() -> String { "solid".into() }
fn default_blur_mode() -> String { "deep_focus".into() }
fn default_toggle_shortcut() -> String { "Ctrl+Alt+Win+F".into() }
fn default_mode_shortcut() -> String { "Ctrl+Alt+Win+M".into() }
fn default_settings_shortcut() -> String { "Ctrl+Alt+Win+C".into() }

/// One entry in the ignored-apps list. `exe` is the lowercased executable
/// filename (e.g. "calc.exe") used to match an app's windows; `name` is the
/// friendly label captured when the app was added, so the list still reads
/// well even when the app isn't currently running.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IgnoredApp {
    pub exe: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    /// Missing/unknown values select the capture-free renderer.
    #[serde(default = "default_effect_renderer")]
    pub effect_renderer: String,
    #[serde(default = "default_mask_pattern")]
    pub mask_pattern: String,
    // GPU Gaussian blur strength (0..1 -> stddev 0..STDDEV_MAX). The
    // capture-based blur (gpu_blur) — the only blur now; the old acrylic
    // `blur_enabled`/`blur_intensity` settings have been retired.
    #[serde(default = "default_gpu_blur_intensity")]
    pub gpu_blur_intensity: f64,
    // "deep_focus": uniform full-screen blur. "ambient": progressive blur,
    // sharp at the top of the screen ramping to full blur at the bottom.
    #[serde(default = "default_blur_mode")]
    pub blur_mode: String,
    pub grain_amount: f64,
    #[serde(default)]
    pub desaturate_enabled: bool,
    pub tint_color: String,
    pub tint_opacity: f64,
    pub shake_sensitivity: f64,
    #[serde(default = "default_fade_duration_secs")]
    pub fade_duration_secs: f64,
    pub per_monitor_focus: bool,
    // When true, focusing a window keeps every window of that same
    // application sharp (not just the active one). Interacts with
    // per_monitor_focus: see `focused_group` in overlay.rs.
    #[serde(default)]
    pub app_wide_focus: bool,
    pub blur_taskbar: bool,
    // Hide the desktop icons while Deep is active (restored on deactivate).
    // Driven through the shell's "Show desktop icons" toggle, not a registry
    // edit, so it takes effect instantly and reverses cleanly.
    #[serde(default)]
    pub hide_desktop_icons: bool,
    // Global hotkeys, as "+"-delimited specs (e.g. "Ctrl+Alt+Win+F"). Parsed
    // in lib.rs; "Win"/"Super"/"Meta" all map to the Windows key. An empty
    // string disables that hotkey. toggle = on/off, mode = switch blur mode,
    // settings = show/hide the settings window.
    #[serde(default = "default_toggle_shortcut")]
    pub toggle_shortcut: String,
    #[serde(default = "default_mode_shortcut")]
    pub mode_shortcut: String,
    #[serde(default = "default_settings_shortcut")]
    pub settings_shortcut: String,
    // Apps whose windows always stay sharp (above the blur), regardless of
    // focus or any other setting. Matched by executable filename; see
    // IGNORED_EXES / ignored_hwnds in overlay.rs.
    #[serde(default)]
    pub ignored_apps: Vec<IgnoredApp>,
    pub start_on_login: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            effect_renderer: default_effect_renderer(),
            mask_pattern: default_mask_pattern(),
            gpu_blur_intensity: default_gpu_blur_intensity(),
            blur_mode: default_blur_mode(),
            grain_amount: 0.5,
            desaturate_enabled: false,
            tint_color: "#0f172a".into(),
            tint_opacity: 0.4,
            shake_sensitivity: 0.5,
            fade_duration_secs: default_fade_duration_secs(),
            per_monitor_focus: true,
            app_wide_focus: false,
            blur_taskbar: false,
            hide_desktop_icons: false,
            toggle_shortcut: default_toggle_shortcut(),
            mode_shortcut: default_mode_shortcut(),
            settings_shortcut: default_settings_shortcut(),
            ignored_apps: Vec::new(),
            start_on_login: false,
        }
    }
}

impl AppSettings {
    fn load_from_dir(dir: &std::path::Path) -> Self {
        // Only the provided fork directory is read. No legacy fallback.
        std::fs::read_to_string(dir.join("settings.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save_to_dir(&self, dir: &std::path::Path) {
        std::fs::create_dir_all(dir).ok();
        if let Ok(json) = serde_json::to_string_pretty(self) {
            std::fs::write(dir.join("settings.json"), json).ok();
        }
    }

    pub fn load() -> Self {
        Self::load_from_dir(&crate::identity::data_dir())
    }

    pub fn save(&self) {
        self.save_to_dir(&crate::identity::data_dir());
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_migration_defaults_to_static_and_preserves_blur_preferences() {
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        value.as_object_mut().unwrap().remove("effect_renderer");
        value.as_object_mut().unwrap().remove("mask_pattern");
        value["gpu_blur_intensity"] = serde_json::json!(0.83);
        let migrated: AppSettings = serde_json::from_value(value).unwrap();
        assert_eq!(migrated.effect_renderer, "mask");
        assert_eq!(migrated.mask_pattern, "solid");
        assert_eq!(migrated.gpu_blur_intensity, 0.83);
        let mut selected = migrated;
        selected.effect_renderer = "blur".into();
        selected.mask_pattern = "grid".into();
        let restored: AppSettings = serde_json::from_str(&serde_json::to_string(&selected).unwrap()).unwrap();
        assert_eq!(restored.effect_renderer, "blur");
        assert_eq!(restored.mask_pattern, "grid");
    }

    #[test]
    fn legacy_settings_are_neither_imported_nor_modified() {
        let root = std::env::temp_dir().join(format!("deep-lite-isolation-test-{}", std::process::id()));
        assert!(!root.exists(), "test fixture path must be new");
        let legacy = root.join("Deep");
        std::fs::create_dir_all(&legacy).unwrap();
        let mut upstream = AppSettings::default();
        upstream.start_on_login = true;
        upstream.gpu_blur_intensity = 0.91;
        let original = serde_json::to_string(&upstream).unwrap();
        std::fs::write(legacy.join("settings.json"), &original).unwrap();
        let fork = crate::identity::data_dir_at(&root);
        let mut fresh = AppSettings::load_from_dir(&fork);
        assert!(!fresh.start_on_login);
        assert_eq!(fresh.gpu_blur_intensity, default_gpu_blur_intensity());
        assert!(!fork.exists(), "loading defaults must not migrate/create data");
        fresh.gpu_blur_intensity = 0.42;
        fresh.save_to_dir(&fork);
        assert_eq!(AppSettings::load_from_dir(&fork).gpu_blur_intensity, 0.42);
        assert_eq!(std::fs::read_to_string(legacy.join("settings.json")).unwrap(), original);
        std::fs::write(fork.join("settings.json"), "invalid json").unwrap();
        assert!(!AppSettings::load_from_dir(&fork).start_on_login);
        assert_eq!(std::fs::read_to_string(legacy.join("settings.json")).unwrap(), original);
        std::fs::remove_dir_all(root).unwrap();
    }
}

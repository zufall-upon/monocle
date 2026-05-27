use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub blur_enabled: bool,
    pub blur_intensity: f64,
    pub grain_amount: f64,
    pub tint_color: String,
    pub tint_opacity: f64,
    pub shake_sensitivity: f64,
    pub per_monitor_focus: bool,
    pub blur_taskbar: bool,
    pub start_on_login: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            blur_enabled: true,
            blur_intensity: 0.6,
            grain_amount: 0.3,
            tint_color: "#000000".into(),
            tint_opacity: 0.4,
            shake_sensitivity: 0.5,
            per_monitor_focus: true,
            blur_taskbar: false,
            start_on_login: false,
        }
    }
}

impl AppSettings {
    fn config_path() -> std::path::PathBuf {
        let dir = dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("Monocle");
        std::fs::create_dir_all(&dir).ok();
        dir.join("settings.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Ok(json) = serde_json::to_string_pretty(self) {
            std::fs::write(path, json).ok();
        }
    }
}

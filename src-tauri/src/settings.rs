use serde::{Deserialize, Serialize};

fn default_fade_duration_secs() -> f64 { 0.75 }
fn default_gpu_blur_intensity() -> f64 { 0.3 }
fn default_blur_mode() -> String { "deep_focus".into() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
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
    pub blur_taskbar: bool,
    pub start_on_login: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            gpu_blur_intensity: default_gpu_blur_intensity(),
            blur_mode: default_blur_mode(),
            grain_amount: 0.5,
            desaturate_enabled: false,
            tint_color: "#000000".into(),
            tint_opacity: 0.1,
            shake_sensitivity: 0.5,
            fade_duration_secs: default_fade_duration_secs(),
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

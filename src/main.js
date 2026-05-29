const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

let settings = {};

async function init() {
  settings = await invoke("get_settings");
  applySettingsToUI(settings);

  listen("monocle-toggled", (event) => {
    updateStatusUI(event.payload);
  });
}

function applySettingsToUI(s) {
  document.getElementById("tint-opacity").value = s.tint_opacity * 100;
  document.getElementById("val-tint-opacity").textContent = Math.round(s.tint_opacity * 100) + "%";

  document.getElementById("blur-intensity").value = s.gpu_blur_intensity * 100;
  document.getElementById("val-blur").textContent = Math.round(s.gpu_blur_intensity * 100) + "%";
  setBlurModeUI(s.blur_mode || "deep_focus");

  document.getElementById("grain-amount").value = s.grain_amount * 100;
  document.getElementById("val-grain").textContent = Math.round(s.grain_amount * 100) + "%";

  document.getElementById("shake-sensitivity").value = s.shake_sensitivity * 100;
  document.getElementById("val-shake").textContent = Math.round(s.shake_sensitivity * 100) + "%";

  document.getElementById("fade-duration").value = Math.round(s.fade_duration_secs * 100);
  document.getElementById("val-fade").textContent = s.fade_duration_secs.toFixed(2) + "s";

  document.getElementById("desaturate-enabled").checked = s.desaturate_enabled;
  document.getElementById("per-monitor-focus").checked = s.per_monitor_focus;
  document.getElementById("blur-taskbar").checked = s.blur_taskbar;
  document.getElementById("start-on-login").checked = s.start_on_login;

  // Set active color swatch
  const swatches = document.querySelectorAll(".swatch");
  swatches.forEach((sw) => sw.classList.remove("active"));
  const match = document.querySelector(`.swatch[data-color="${s.tint_color}"]`);
  if (match) {
    match.classList.add("active");
  }
  document.getElementById("custom-color").value = s.tint_color;
}

function updateStatusUI(active) {
  const dot = document.getElementById("status-dot");
  const text = document.getElementById("status-text");
  const btn = document.getElementById("btn-toggle");
  const label = document.getElementById("toggle-label");

  if (active) {
    dot.classList.add("active");
    text.textContent = "Active";
    btn.classList.add("active");
    label.textContent = "Deactivate";
  } else {
    dot.classList.remove("active");
    text.textContent = "Inactive";
    btn.classList.remove("active");
    label.textContent = "Activate";
  }
}

async function saveSettings() {
  await invoke("update_settings", { newSettings: settings });
}

// Slider bindings
function bindSlider(id, key, displayId) {
  const el = document.getElementById(id);
  el.addEventListener("input", () => {
    const val = parseInt(el.value);
    document.getElementById(displayId).textContent = val + "%";
    settings[key] = val / 100;
    saveSettings();
  });
}

bindSlider("tint-opacity", "tint_opacity", "val-tint-opacity");
bindSlider("blur-intensity", "gpu_blur_intensity", "val-blur");
bindSlider("grain-amount", "grain_amount", "val-grain");

// Blur mode segmented toggle: deep_focus (uniform) vs ambient (progressive).
function setBlurModeUI(mode) {
  document.querySelectorAll("#blur-mode .seg-btn").forEach((btn) => {
    btn.classList.toggle("active", btn.dataset.mode === mode);
  });
}

document.querySelectorAll("#blur-mode .seg-btn").forEach((btn) => {
  btn.addEventListener("click", () => {
    settings.blur_mode = btn.dataset.mode;
    setBlurModeUI(btn.dataset.mode);
    saveSettings();
  });
});
bindSlider("shake-sensitivity", "shake_sensitivity", "val-shake");

// Crossfade duration: slider 0-150 represents 0.00-1.50 seconds.
document.getElementById("fade-duration").addEventListener("input", (e) => {
  const val = parseInt(e.target.value);
  const secs = val / 100;
  document.getElementById("val-fade").textContent = secs.toFixed(2) + "s";
  settings.fade_duration_secs = secs;
  saveSettings();
});

// Toggle button
document.getElementById("btn-toggle").addEventListener("click", async () => {
  const active = await invoke("toggle_active");
  updateStatusUI(active);
});

// Close button - hide window instead of quitting
document.getElementById("btn-close").addEventListener("click", () => {
  getCurrentWindow().hide();
});

// Color swatches
document.querySelectorAll(".swatch").forEach((swatch) => {
  swatch.addEventListener("click", () => {
    document.querySelectorAll(".swatch").forEach((s) => s.classList.remove("active"));
    swatch.classList.add("active");
    settings.tint_color = swatch.dataset.color;
    document.getElementById("custom-color").value = swatch.dataset.color;
    saveSettings();
  });
});

document.getElementById("custom-color").addEventListener("input", (e) => {
  document.querySelectorAll(".swatch").forEach((s) => s.classList.remove("active"));
  settings.tint_color = e.target.value;
  saveSettings();
});

// Desaturate background
document.getElementById("desaturate-enabled").addEventListener("change", (e) => {
  settings.desaturate_enabled = e.target.checked;
  saveSettings();
});

// Per-monitor focus
document.getElementById("per-monitor-focus").addEventListener("change", (e) => {
  settings.per_monitor_focus = e.target.checked;
  saveSettings();
});

// Blur taskbar
document.getElementById("blur-taskbar").addEventListener("change", (e) => {
  settings.blur_taskbar = e.target.checked;
  saveSettings();
});

// Start on login
document.getElementById("start-on-login").addEventListener("change", (e) => {
  settings.start_on_login = e.target.checked;
  saveSettings();
});

init();

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

  document.getElementById("blur-enabled").checked = s.blur_enabled;
  document.getElementById("blur-intensity").value = s.blur_intensity * 100;
  document.getElementById("val-blur").textContent = Math.round(s.blur_intensity * 100) + "%";
  updateBlurUI(s.blur_enabled);

  document.getElementById("grain-amount").value = s.grain_amount * 100;
  document.getElementById("val-grain").textContent = Math.round(s.grain_amount * 100) + "%";

  document.getElementById("shake-sensitivity").value = s.shake_sensitivity * 100;
  document.getElementById("val-shake").textContent = Math.round(s.shake_sensitivity * 100) + "%";

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
bindSlider("blur-intensity", "blur_intensity", "val-blur");

function updateBlurUI(enabled) {
  const group = document.getElementById("blur-intensity").closest(".control-group");
  if (enabled) {
    group.classList.remove("blur-disabled");
  } else {
    group.classList.add("blur-disabled");
  }
}

document.getElementById("blur-enabled").addEventListener("change", (e) => {
  settings.blur_enabled = e.target.checked;
  updateBlurUI(e.target.checked);
  saveSettings();
});
bindSlider("grain-amount", "grain_amount", "val-grain");
bindSlider("shake-sensitivity", "shake_sensitivity", "val-shake");

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

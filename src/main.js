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

  // The mode-switch hotkey changes settings from outside this window;
  // re-apply so the blur-mode toggle stays in sync.
  listen("settings-updated", (event) => {
    settings = event.payload;
    applySettingsToUI(settings);
  });
}

function applySettingsToUI(s) {
  document.getElementById("tint-opacity").value = s.tint_opacity * 100;

  document.getElementById("blur-intensity").value = s.gpu_blur_intensity * 100;
  setBlurModeUI(s.blur_mode || "deep_focus");

  document.getElementById("grain-amount").value = s.grain_amount * 100;

  document.getElementById("shake-sensitivity").value = s.shake_sensitivity * 100;

  document.getElementById("fade-duration").value = Math.round(s.fade_duration_secs * 100);
  document.getElementById("val-fade").textContent = s.fade_duration_secs.toFixed(2) + "s";

  document.getElementById("desaturate-enabled").checked = s.desaturate_enabled;
  document.getElementById("per-monitor-focus").checked = s.per_monitor_focus;
  document.getElementById("app-wide-focus").checked = s.app_wide_focus;
  document.getElementById("blur-taskbar").checked = s.blur_taskbar;
  document.getElementById("hide-desktop-icons").checked = s.hide_desktop_icons;
  document.getElementById("start-on-login").checked = s.start_on_login;

  // Document the active global hotkeys in the Shortcuts section.
  const fmt = (spec) => (spec || "").replace(/\+/g, " + ");
  document.getElementById("sc-toggle").textContent = fmt(s.toggle_shortcut);
  document.getElementById("sc-mode").textContent = fmt(s.mode_shortcut);
  document.getElementById("sc-settings").textContent = fmt(s.settings_shortcut);

  // Reflect the current tint color in the single circle + the palette.
  document.getElementById("swatch-current").style.background = s.tint_color;
  document.getElementById("custom-color").value = s.tint_color;
  document.querySelectorAll(".swatch").forEach((sw) => sw.classList.remove("active"));
  const match = document.querySelector(`.swatch[data-color="${s.tint_color}"]`);
  if (match) {
    match.classList.add("active");
  }
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

// Slider bindings. displayId is optional — sliders without a value label
// (everything but Crossfade) pass none.
function bindSlider(id, key, displayId) {
  const el = document.getElementById(id);
  const display = displayId ? document.getElementById(displayId) : null;
  el.addEventListener("input", () => {
    const val = parseInt(el.value);
    if (display) display.textContent = val + "%";
    settings[key] = val / 100;
    saveSettings();
  });
}

bindSlider("tint-opacity", "tint_opacity");
bindSlider("blur-intensity", "gpu_blur_intensity");
bindSlider("grain-amount", "grain_amount");

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
bindSlider("shake-sensitivity", "shake_sensitivity");

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

// Tint color: a single circle that expands into the palette and collapses
// once a color is picked.
const tintControl = document.getElementById("tint-control");
const swatchCurrent = document.getElementById("swatch-current");
const pickerName = document.getElementById("picker-name");
const pickerBack = document.getElementById("picker-back");

// Map a hex color to its palette name, falling back to "Custom".
function colorName(color) {
  const match = document.querySelector(`.swatch[data-color="${color}"]`);
  return match ? match.dataset.name : "Custom";
}

function setTintControlExpanded(expanded) {
  if (expanded) pickerName.textContent = colorName(settings.tint_color);
  tintControl.classList.toggle("expanded", expanded);
}

function selectTintColor(color) {
  settings.tint_color = color;
  swatchCurrent.style.background = color;
  document.getElementById("custom-color").value = color;
  document.querySelectorAll(".swatch").forEach((s) => s.classList.remove("active"));
  const match = document.querySelector(`.swatch[data-color="${color}"]`);
  if (match) match.classList.add("active");
  pickerName.textContent = colorName(color);
  saveSettings();
}

swatchCurrent.addEventListener("click", () => {
  setTintControlExpanded(!tintControl.classList.contains("expanded"));
});

pickerBack.addEventListener("click", () => setTintControlExpanded(false));

// Stay in the picker while choosing so colors can be compared freely; the
// back button (or a click outside) returns to the main row.
document.querySelectorAll(".swatch").forEach((swatch) => {
  swatch.addEventListener("click", () => selectTintColor(swatch.dataset.color));
});

// Custom picker: reflect the color live as it changes.
const customColor = document.getElementById("custom-color");
customColor.addEventListener("input", (e) => selectTintColor(e.target.value));

// Collapse the palette when clicking anywhere outside the tint control.
document.addEventListener("click", (e) => {
  if (!tintControl.contains(e.target)) {
    setTintControlExpanded(false);
  }
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

// App-wide focus
document.getElementById("app-wide-focus").addEventListener("change", (e) => {
  settings.app_wide_focus = e.target.checked;
  saveSettings();
});

// Blur taskbar
document.getElementById("blur-taskbar").addEventListener("change", (e) => {
  settings.blur_taskbar = e.target.checked;
  saveSettings();
});

// Hide desktop icons
document.getElementById("hide-desktop-icons").addEventListener("change", (e) => {
  settings.hide_desktop_icons = e.target.checked;
  saveSettings();
});

// Start on login
document.getElementById("start-on-login").addEventListener("change", (e) => {
  settings.start_on_login = e.target.checked;
  saveSettings();
});

init();

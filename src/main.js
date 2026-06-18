const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

let settings = {};

async function init() {
  settings = await invoke("get_settings");
  applySettingsToUI(settings);

  listen("deep-toggled", (event) => {
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

  renderIgnoredApps();
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

// --- Ignored apps -------------------------------------------------------

// Render the current ignored-apps list (one row each, with a remove button)
// and toggle the empty-state hint. Reads from the live `settings` object.
function renderIgnoredApps() {
  const list = document.getElementById("ignored-list");
  const empty = document.getElementById("ignored-empty");
  const apps = settings.ignored_apps || [];
  list.innerHTML = "";
  empty.hidden = apps.length > 0;
  for (const app of apps) {
    const row = document.createElement("div");
    row.className = "ignored-app-row";
    const name = document.createElement("span");
    name.className = "ignored-app-name";
    name.textContent = app.name || app.exe;
    const remove = document.createElement("button");
    remove.className = "icon-btn";
    remove.setAttribute("aria-label", `Stop ignoring ${app.name || app.exe}`);
    remove.textContent = "−"; // minus sign
    remove.addEventListener("click", () => removeIgnoredApp(app.exe));
    row.append(name, remove);
    list.append(row);
  }
}

// Append an app to the ignore list (deduped by exe, case-insensitive),
// persist, and re-render. No-op if already ignored.
function addIgnoredApp(app) {
  if (!app || !app.exe) return;
  const exe = app.exe.toLowerCase();
  const apps = settings.ignored_apps || (settings.ignored_apps = []);
  if (apps.some((a) => a.exe.toLowerCase() === exe)) return;
  apps.push({ exe, name: app.name || app.exe });
  renderIgnoredApps();
  saveSettings();
}

function removeIgnoredApp(exe) {
  const target = (exe || "").toLowerCase();
  settings.ignored_apps = (settings.ignored_apps || []).filter(
    (a) => a.exe.toLowerCase() !== target,
  );
  renderIgnoredApps();
  saveSettings();
}

// Quick-add card: show the frontmost real app (the one behind this settings
// window) so a single click ignores it. Hidden only when nothing's eligible;
// when the app is already ignored the card stays (visual continuity) but
// greys out and the click is a no-op.
let foregroundApp = null;
async function refreshForegroundApp() {
  foregroundApp = await invoke("get_foreground_app");
  const card = document.getElementById("ignore-current");
  if (!foregroundApp) {
    card.hidden = true;
    return;
  }
  const already = (settings.ignored_apps || []).some(
    (a) => a.exe.toLowerCase() === foregroundApp.exe.toLowerCase(),
  );
  document.getElementById("ignore-current-name").textContent = foregroundApp.name;
  card.classList.toggle("is-ignored", already);
  card.hidden = false;
}

document.getElementById("ignore-current").addEventListener("click", (e) => {
  if (e.currentTarget.classList.contains("is-ignored")) return;
  addIgnoredApp(foregroundApp);
  e.currentTarget.classList.add("is-ignored");
});

// Section "+" picker: list running apps and let the user ignore one.
const ignoredPicker = document.getElementById("ignored-picker");
async function toggleIgnoredPicker() {
  if (!ignoredPicker.hidden) {
    ignoredPicker.hidden = true;
    return;
  }
  const apps = await invoke("list_running_apps");
  const ignored = new Set(
    (settings.ignored_apps || []).map((a) => a.exe.toLowerCase()),
  );
  const available = apps.filter((a) => !ignored.has(a.exe.toLowerCase()));
  ignoredPicker.innerHTML = "";
  if (available.length === 0) {
    const none = document.createElement("div");
    none.className = "ignored-picker-empty";
    none.textContent = "No other apps running";
    ignoredPicker.append(none);
  } else {
    for (const app of available) {
      const item = document.createElement("button");
      item.className = "ignored-picker-item";
      item.textContent = app.name || app.exe;
      item.addEventListener("click", () => {
        addIgnoredApp(app);
        ignoredPicker.hidden = true;
        refreshForegroundApp();
      });
      ignoredPicker.append(item);
    }
  }
  ignoredPicker.hidden = false;
}

document.getElementById("ignored-add").addEventListener("click", (e) => {
  e.stopPropagation();
  toggleIgnoredPicker();
});

// Close the picker when clicking outside it.
document.addEventListener("click", (e) => {
  if (!ignoredPicker.hidden && !e.target.closest(".ignored-add-wrap")) {
    ignoredPicker.hidden = true;
  }
});

// Keep the quick-add card pointed at the last-interacted-with window. The
// backend resolves the topmost real window behind this settings window — which
// is exactly the app the user last used, on any monitor. Poll so it tracks the
// user moving between windows live; the focus event refreshes instantly on
// return to settings.
getCurrentWindow().onFocusChanged(({ payload: focused }) => {
  if (focused) refreshForegroundApp();
});
setInterval(refreshForegroundApp, 250);

init();
refreshForegroundApp();

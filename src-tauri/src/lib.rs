#[cfg(windows)]
mod input_policy;
mod renderer_policy;
mod focus_policy;
#[cfg(any(windows, test))]
mod blur_policy;
mod gpu_blur;
mod identity;
mod logging;
mod overlay;
mod settings;
mod shake;
mod windows_api;

use settings::AppSettings;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};
use tauri_plugin_global_shortcut::{
    Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState,
};

#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<Mutex<AppSettings>>,
    pub active: Arc<Mutex<bool>>,
    // The global hotkeys currently registered. Held so we can unregister them
    // before rebinding when any shortcut setting changes.
    pub shortcuts: Arc<Mutex<Vec<Shortcut>>>,
}

#[tauri::command]
fn get_focus_diagnostics() -> String { overlay::diagnostic_snapshot() }

#[tauri::command]
fn get_settings(state: tauri::State<AppState>) -> AppSettings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn update_settings(app: tauri::AppHandle, state: tauri::State<AppState>, new_settings: AppSettings) {
    let (shortcuts_changed, start_changed) = {
        let mut s = state.settings.lock().unwrap();
        let shortcuts_changed = s.toggle_shortcut != new_settings.toggle_shortcut
            || s.mode_shortcut != new_settings.mode_shortcut
            || s.settings_shortcut != new_settings.settings_shortcut;
        let start_changed = s.start_on_login != new_settings.start_on_login;
        if s.ignored_exes()!=new_settings.ignored_exes() {
            logging::log(&format!("ignored_apps changed via update_settings: {:?} -> {:?}",s.ignored_exes(),new_settings.ignored_exes()));
        }
        *s = new_settings.clone();
        s.save();
        (shortcuts_changed, start_changed)
    };
    overlay::update_overlay(&new_settings, *state.active.lock().unwrap());
    // Keep shake detection's distance threshold in sync with the slider.
    shake::set_sensitivity(new_settings.shake_sensitivity);
    // Rebind the global hotkeys only when a spec actually changed, so a
    // routine settings save doesn't churn the OS registration.
    if shortcuts_changed {
        register_shortcuts(&app, &new_settings);
    }
    // Register/unregister the login entry only when the toggle actually flips.
    if start_changed {
        set_start_on_login(new_settings.start_on_login);
    }
}

/// Flip the blur mode between deep-focus and ambient, persist it, apply it
/// live, and notify the settings UI so its segmented toggle stays in sync.
fn toggle_blur_mode<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let state = app.state::<AppState>();
    let new_settings = {
        let mut s = state.settings.lock().unwrap();
        s.blur_mode = if s.blur_mode == "ambient" {
            "deep_focus".into()
        } else {
            "ambient".into()
        };
        s.save();
        s.clone()
    };
    let active = *state.active.lock().unwrap();
    overlay::update_overlay(&new_settings, active);
    let _ = app.emit("settings-updated", new_settings);
}

/// Show/hide the settings window. When it's already visible this hides it —
/// identical to clicking its X button (which also just hides, not quits).
fn toggle_settings_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(window) = app.get_webview_window("settings") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

/// Map a single key token (already lowercased) to a keyboard_types `Code`.
/// Handles bare letters/digits ("f" -> KeyF, "5" -> Digit5) and function
/// keys ("f5" -> F5); anything else is tried verbatim against `Code`'s
/// W3C-code parser (e.g. "space", "enter" won't match — pass "Space").
fn key_to_code(token: &str) -> Option<Code> {
    let t = token.trim();
    if t.is_empty() {
        return None;
    }
    let normalized = if t.len() == 1 && t.as_bytes()[0].is_ascii_alphabetic() {
        format!("Key{}", t.to_ascii_uppercase())
    } else if t.len() == 1 && t.as_bytes()[0].is_ascii_digit() {
        format!("Digit{t}")
    } else if t.starts_with('f') && t[1..].chars().all(|c| c.is_ascii_digit()) && t.len() > 1 {
        format!("F{}", &t[1..])
    } else {
        // Title-case so "space"/"enter"/"tab" match W3C "Space"/"Enter"/"Tab".
        let mut c = t.chars();
        c.next()
            .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
            .unwrap_or_default()
    };
    Code::from_str(&normalized).ok()
}

/// Parse a "+"-delimited hotkey spec (e.g. "Ctrl+Alt+Win+F") into a
/// `Shortcut`. Modifier aliases are case-insensitive; "Win"/"Super"/"Meta"/
/// "Cmd" all map to the Windows/Super key. Returns None if no key is present
/// or the key token is unrecognized (e.g. an empty/disabled spec).
fn parse_shortcut(spec: &str) -> Option<Shortcut> {
    let mut mods = Modifiers::empty();
    let mut code: Option<Code> = None;
    for token in spec.split('+') {
        let t = token.trim();
        if t.is_empty() {
            continue;
        }
        match t.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= Modifiers::CONTROL,
            "alt" | "option" | "opt" => mods |= Modifiers::ALT,
            "shift" => mods |= Modifiers::SHIFT,
            "win" | "windows" | "super" | "meta" | "cmd" | "command" => mods |= Modifiers::SUPER,
            key => code = key_to_code(key),
        }
    }
    code.map(|c| Shortcut::new(if mods.is_empty() { None } else { Some(mods) }, c))
}

/// (Re)bind every global hotkey from the current settings. Unregisters
/// whatever was bound before, then registers each spec that parses. A blank or
/// unparseable spec is simply skipped (that action gets no hotkey) rather than
/// erroring. Each shortcut fires its action on key-down only — the released
/// edge would otherwise trigger twice per press.
fn register_shortcuts<R: tauri::Runtime>(app: &tauri::AppHandle<R>, settings: &AppSettings) {
    let gs = app.global_shortcut();
    let state = app.state::<AppState>();
    for prev in state.shortcuts.lock().unwrap().drain(..) {
        let _ = gs.unregister(prev);
    }

    let mut registered = Vec::new();
    let mut bind = |spec: &str, action: fn(&tauri::AppHandle<R>)| {
        let Some(shortcut) = parse_shortcut(spec) else {
            return;
        };
        let handle = app.clone();
        let ok = gs
            .on_shortcut(shortcut, move |_app, _sc, event| {
                if event.state == ShortcutState::Pressed {
                    action(&handle);
                }
            })
            .is_ok();
        if ok {
            registered.push(shortcut);
        }
    };

    bind(&settings.toggle_shortcut, toggle_active_with_handle_action);
    bind(&settings.mode_shortcut, toggle_blur_mode);
    bind(&settings.settings_shortcut, toggle_settings_window);

    *state.shortcuts.lock().unwrap() = registered;
}

/// Thin wrapper so the toggle action has the same `fn(&AppHandle)` shape as the
/// other hotkey actions (`toggle_active_with_handle` returns the new state).
fn toggle_active_with_handle_action<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    toggle_active_with_handle(app, "hotkey");
}

/// Single chokepoint for changing the active state. Every toggle/set
/// entry point (UI button, tray menu, tray icon, shake) goes through
/// this so the active mutex, the overlay's internal state, and the UI
/// (via the deep-toggled event) can never disagree. `source` is logged
/// so an unexpected activation can be traced back to what triggered it.
fn apply_active<R: tauri::Runtime>(app: &tauri::AppHandle<R>, value: bool, source: &str) {
    let state = app.state::<AppState>();
    let changed = {
        let mut active = state.active.lock().unwrap();
        let prev = *active;
        *active = value;
        prev != value
    };
    if !changed {
        logging::log(&format!("active: no-op {} (source: {})", value, source));
        return;
    }
    logging::log(&format!("active -> {} (source: {})", value, source));
    let settings = state.settings.lock().unwrap().clone();
    overlay::update_overlay(&settings, value);
    let _ = app.emit("deep-toggled", value);
}

fn toggle_active_with_handle<R: tauri::Runtime>(app: &tauri::AppHandle<R>, source: &str) -> bool {
    let current = {
        let state = app.state::<AppState>();
        let active = state.active.lock().unwrap();
        *active
    };
    apply_active(app, !current, source);
    !current
}

#[tauri::command]
fn toggle_active(app: tauri::AppHandle) -> bool {
    toggle_active_with_handle(&app, "ui-button")
}

#[tauri::command]
fn set_active(app: tauri::AppHandle, value: bool) {
    apply_active(&app, value, "ui-set_active");
}

/// Every currently-running app the user could ignore, deduped by executable
/// and sorted by name. Backs the "add ignored app" picker in settings.
#[tauri::command]
fn list_running_apps() -> Vec<settings::IgnoredApp> {
    windows_api::list_app_windows()
}

/// The frontmost real app behind the settings window, for the quick-add card.
/// None when there's nothing eligible in front.
#[tauri::command]
fn get_foreground_app() -> Option<settings::IgnoredApp> {
    windows_api::foreground_app()
}

/// Acquire a named mutex so only one Deep process can run at a time.
/// Returns true if we got the lock (first instance), false if another
/// instance already holds it. The HANDLE is intentionally leaked — the
/// kernel releases it on process exit, which is exactly when we want
/// the next launch to be permitted. `Local\\` (not `Global\\`) scopes
/// the mutex to the user's session, so two different users on the same
/// machine can each run their own instance.
#[cfg(windows)]
fn acquire_single_instance_lock() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    let name: Vec<u16> = identity::SHARED_MUTEX_NAME.encode_utf16().chain(Some(0)).collect();
    unsafe {
        match CreateMutexW(None, true, PCWSTR(name.as_ptr())) {
            Ok(_handle) => GetLastError() != ERROR_ALREADY_EXISTS,
            // Do not risk two overlays when exclusivity cannot be established.
            // This returns before reading settings or touching the Run key.
            Err(_) => false,
        }
    }
}

#[cfg(not(windows))]
fn acquire_single_instance_lock() -> bool {
    true
}

/// Register or unregister Deep to launch at user login by writing (or
/// deleting) a value under HKCU\...\CurrentVersion\Run. Per-user, so it
/// needs no elevation; the value points at the current executable, so it
/// self-corrects if the binary moves and the setting is re-applied.
#[cfg(windows)]
fn set_start_on_login(enabled: bool) {
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };
    // Quote the path so a space in it (e.g. "Program Files") still parses as
    // a single argument when the shell launches it at login.
    let value: Vec<u16> = format!("\"{}\"", exe.display())
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let run_name: Vec<u16> = identity::RUN_VALUE_NAME.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let mut hkey = HKEY::default();
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        ) != ERROR_SUCCESS
        {
            return;
        }

        if enabled {
            let bytes = std::slice::from_raw_parts(
                value.as_ptr() as *const u8,
                value.len() * std::mem::size_of::<u16>(),
            );
            let _ = RegSetValueExW(hkey, PCWSTR(run_name.as_ptr()), None, REG_SZ, Some(bytes));
        } else {
            // Deleting a missing value returns an error we intentionally ignore.
            let _ = RegDeleteValueW(hkey, PCWSTR(run_name.as_ptr()));
        }
        let _ = RegCloseKey(hkey);
    }
}

#[cfg(not(windows))]
fn set_start_on_login(_enabled: bool) {}

pub fn run() {
    if !acquire_single_instance_lock() {
        return;
    }

    logging::init();
    let settings = AppSettings::load();
    // Only a previously opted-in Deep Lite setting may reconcile its own Run
    // value. Fresh/default launches do not create, delete or modify Run values.
    if identity::reconcile_autostart_on_launch(settings.start_on_login) {
        set_start_on_login(true);
    }
    let state = AppState {
        settings: Arc::new(Mutex::new(settings)),
        active: Arc::new(Mutex::new(false)),
        shortcuts: Arc::new(Mutex::new(Vec::new())),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(state.clone())
        .setup(move |app| {
            // Build system tray
            let toggle_i = MenuItem::with_id(app, "toggle", "Toggle Deep Lite", true, None::<&str>)?;
            let settings_i =
                MenuItem::with_id(app, "settings", "Settings...", true, None::<&str>)?;
            let logs_i =
                MenuItem::with_id(app, "open_logs", "Open logs", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit Deep Lite", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&toggle_i, &settings_i, &logs_i, &quit_i])?;

            let mut tray = TrayIconBuilder::new()
                .tooltip(identity::PRODUCT_NAME)
                .menu(&menu);
            // Show the app icon in the tray. Falls back gracefully if no
            // default window icon is configured.
            if let Some(icon) = app.default_window_icon().cloned() {
                tray = tray.icon(icon);
            }
            let _tray = tray
                .show_menu_on_left_click(false)
                .on_menu_event({
                    let app_handle = app.handle().clone();
                    move |_tray, event| match event.id.as_ref() {
                        "toggle" => {
                            toggle_active_with_handle(&app_handle, "tray-menu");
                        }
                        "settings" => {
                            if let Some(window) = app_handle.get_webview_window("settings") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                        "open_logs" => {
                            // Reveal the log file in Explorer so the user can
                            // grab it when reporting a bug.
                            let path = logging::log_path();
                            let _ = std::process::Command::new("explorer")
                                .arg("/select,")
                                .arg(&path)
                                .spawn();
                        }
                        "quit" => {
                            // Undo any shell state we changed (taskbar
                            // auto-hide, desktop icons) before exiting, since
                            // exit(0) skips the deactivate fade that normally
                            // reverts them.
                            overlay::restore_shell_state();
                            app_handle.exit(0);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event({
                    let app_handle = app.handle().clone();
                    move |_tray, event| {
                        if let TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } = event
                        {
                            toggle_active_with_handle(&app_handle, "tray-click");
                        }
                    }
                })
                .build(app)?;

            // Visual windows never intercept input; Windows handles native clicks.
            overlay::init();

            // The retired Magnification path is not initialized. Desaturation
            // is handled by the selected renderer, not an unused capture host.

            // GPU blur layer: per-monitor DirectComposition windows that
            // capture the screen, run a Saturation -> Gaussian Direct2D
            // chain, and present the result into the overlay stack between
            // the (now-disabled) acrylic blur placeholders and the tint
            // layer. Replaces the old acrylic blur + white-offset trick.
            gpu_blur::init();

            // Exclude our own settings window from the GPU blur's screen
            // capture so it isn't blurred/haloed when shown over the overlay.
            // Maps to SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) on Windows.
            if let Some(settings_win) = app.get_webview_window("settings") {
                let protected = settings_win.set_content_protected(true);
                logging::log(&format!("settings capture exclusion requested: {protected:?}"));
                // Register its HWND so the overlay keeps it above the blur and
                // excludes it from all focus/z-order mechanics.
                #[cfg(windows)]
                if let Ok(hwnd) = settings_win.hwnd() {
                    overlay::register_settings_window(hwnd.0 as isize);
                }
            }

            // Start mouse shake detection in a background thread, seeded with
            // the saved sensitivity so the slider's value applies immediately.
            shake::set_sensitivity(
                app.state::<AppState>().settings.lock().unwrap().shake_sensitivity,
            );
            let shake_handle = app.handle().clone();
            std::thread::spawn(move || {
                shake::start_detection(move || {
                    toggle_active_with_handle(&shake_handle, "shake");
                });
            });

            // Bind the global hotkeys from the loaded settings.
            let settings_snapshot = app.state::<AppState>().settings.lock().unwrap().clone();
            register_shortcuts(app.handle(), &settings_snapshot);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_focus_diagnostics,
            get_settings,
            update_settings,
            toggle_active,
            set_active,
            list_running_apps,
            get_foreground_app,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Deep Lite");
}

#[cfg(all(test, windows))]
mod isolation_tests {
    #[test]
    fn shared_overlay_mutex_rejects_a_second_instance() {
        // Runs in the CI test process only, before any app/UI startup.
        assert!(super::acquire_single_instance_lock());
        assert!(!super::acquire_single_instance_lock());
    }
}

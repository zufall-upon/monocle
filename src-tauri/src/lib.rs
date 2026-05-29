mod blur_proto;
mod gpu_blur;
mod magnifier;
mod overlay;
mod settings;
mod shake;
mod windows_api;

use settings::AppSettings;
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};

#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<Mutex<AppSettings>>,
    pub active: Arc<Mutex<bool>>,
}

#[tauri::command]
fn get_settings(state: tauri::State<AppState>) -> AppSettings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn update_settings(state: tauri::State<AppState>, new_settings: AppSettings) {
    let mut s = state.settings.lock().unwrap();
    *s = new_settings.clone();
    s.save();
    overlay::update_overlay(&new_settings, *state.active.lock().unwrap());
}

/// Single chokepoint for changing the active state. Every toggle/set
/// entry point (UI button, tray menu, tray icon, shake) goes through
/// this so the active mutex, the overlay's internal state, and the UI
/// (via the monocle-toggled event) can never disagree.
fn apply_active<R: tauri::Runtime>(app: &tauri::AppHandle<R>, value: bool) {
    let state = app.state::<AppState>();
    let changed = {
        let mut active = state.active.lock().unwrap();
        let prev = *active;
        *active = value;
        prev != value
    };
    if !changed {
        return;
    }
    let settings = state.settings.lock().unwrap().clone();
    overlay::update_overlay(&settings, value);
    let _ = app.emit("monocle-toggled", value);
}

fn toggle_active_with_handle<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    let current = {
        let state = app.state::<AppState>();
        let active = state.active.lock().unwrap();
        *active
    };
    apply_active(app, !current);
    !current
}

#[tauri::command]
fn toggle_active(app: tauri::AppHandle) -> bool {
    toggle_active_with_handle(&app)
}

#[tauri::command]
fn set_active(app: tauri::AppHandle, value: bool) {
    apply_active(&app, value);
}

/// Acquire a named mutex so only one Monocle process can run at a time.
/// Returns true if we got the lock (first instance), false if another
/// instance already holds it. The HANDLE is intentionally leaked — the
/// kernel releases it on process exit, which is exactly when we want
/// the next launch to be permitted. `Local\\` (not `Global\\`) scopes
/// the mutex to the user's session, so two different users on the same
/// machine can each run their own instance.
#[cfg(windows)]
fn acquire_single_instance_lock() -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    unsafe {
        match CreateMutexW(None, true, w!("Local\\MonocleSingleInstance")) {
            Ok(_handle) => GetLastError() != ERROR_ALREADY_EXISTS,
            // If the kernel can't even create a mutex, fall through and
            // allow the launch — failing closed here would lock the user
            // out for a transient OS hiccup.
            Err(_) => true,
        }
    }
}

#[cfg(not(windows))]
fn acquire_single_instance_lock() -> bool {
    true
}

pub fn run() {
    if !acquire_single_instance_lock() {
        return;
    }

    let settings = AppSettings::load();
    let state = AppState {
        settings: Arc::new(Mutex::new(settings)),
        active: Arc::new(Mutex::new(false)),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(state.clone())
        .setup(move |app| {
            // Build system tray
            let toggle_i = MenuItem::with_id(app, "toggle", "Toggle Monocle", true, None::<&str>)?;
            let settings_i =
                MenuItem::with_id(app, "settings", "Settings...", true, None::<&str>)?;
            let blur_test_i =
                MenuItem::with_id(app, "blur_test", "Blur Test", true, None::<&str>)?;
            let blur_progressive_i = MenuItem::with_id(
                app,
                "blur_progressive",
                "Progressive Blur Test",
                true,
                None::<&str>,
            )?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit Monocle", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &toggle_i,
                    &settings_i,
                    &blur_test_i,
                    &blur_progressive_i,
                    &quit_i,
                ],
            )?;

            let _tray = TrayIconBuilder::new()
                .tooltip("Monocle")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event({
                    let app_handle = app.handle().clone();
                    move |_tray, event| match event.id.as_ref() {
                        "toggle" => {
                            toggle_active_with_handle(&app_handle);
                        }
                        "settings" => {
                            if let Some(window) = app_handle.get_webview_window("settings") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                        "blur_test" => {
                            blur_proto::open();
                        }
                        "blur_progressive" => {
                            blur_proto::open_progressive();
                        }
                        "quit" => {
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
                            toggle_active_with_handle(&app_handle);
                        }
                    }
                })
                .build(app)?;

            // Initialize overlay system. The grain layer doubles as an
            // input catcher: it absorbs the first "raise" click on
            // blurred background windows and forces an arrow cursor over
            // them, so accidental UI hits behind the blur don't fire.
            overlay::init();

            // Magnification-API-based desaturation backdrop. Retained but
            // no longer driven — desaturation now happens natively inside
            // the GPU blur pass (gpu_blur). Kept compiled so we can fall
            // back while the new path is proven; slated for removal.
            magnifier::init();

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
                let _ = settings_win.set_content_protected(true);
            }

            // Blur prototype test panels remain available from the tray
            // ("Blur Test" / "Progressive Blur Test"); no longer auto-opened
            // now that the blur is integrated into the production overlay.

            // Start mouse shake detection in a background thread
            let shake_handle = app.handle().clone();
            std::thread::spawn(move || {
                shake::start_detection(move || {
                    toggle_active_with_handle(&shake_handle);
                });
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            update_settings,
            toggle_active,
            set_active,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Monocle");
}

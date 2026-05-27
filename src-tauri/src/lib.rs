mod click_guard;
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

#[tauri::command]
fn toggle_active(state: tauri::State<AppState>) -> bool {
    let mut active = state.active.lock().unwrap();
    *active = !*active;
    let is_active = *active;
    let settings = state.settings.lock().unwrap().clone();
    overlay::update_overlay(&settings, is_active);
    is_active
}

#[tauri::command]
fn set_active(state: tauri::State<AppState>, value: bool) {
    let mut active = state.active.lock().unwrap();
    *active = value;
    let settings = state.settings.lock().unwrap().clone();
    overlay::update_overlay(&settings, value);
}

pub fn run() {
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
            let quit_i = MenuItem::with_id(app, "quit", "Quit Monocle", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&toggle_i, &settings_i, &quit_i])?;

            let _tray = TrayIconBuilder::new()
                .tooltip("Monocle")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event({
                    let app_handle = app.handle().clone();
                    move |_tray, event| match event.id.as_ref() {
                        "toggle" => {
                            let state = app_handle.state::<AppState>();
                            let mut active = state.active.lock().unwrap();
                            *active = !*active;
                            let is_active = *active;
                            let settings = state.settings.lock().unwrap().clone();
                            overlay::update_overlay(&settings, is_active);
                        }
                        "settings" => {
                            if let Some(window) = app_handle.get_webview_window("settings") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
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
                            let state = app_handle.state::<AppState>();
                            let mut active = state.active.lock().unwrap();
                            *active = !*active;
                            let is_active = *active;
                            let settings = state.settings.lock().unwrap().clone();
                            overlay::update_overlay(&settings, is_active);
                        }
                    }
                })
                .build(app)?;

            // Initialize overlay system
            overlay::init();

            // Absorb the first "raise" click on background windows so
            // accidental UI hits behind the blur don't fire.
            #[cfg(windows)]
            click_guard::init();

            // Start mouse shake detection in a background thread
            let shake_state = state.clone();
            let shake_handle = app.handle().clone();
            std::thread::spawn(move || {
                shake::start_detection(move || {
                    let mut active = shake_state.active.lock().unwrap();
                    *active = !*active;
                    let is_active = *active;
                    let settings = shake_state.settings.lock().unwrap().clone();
                    overlay::update_overlay(&settings, is_active);
                    let _ = shake_handle.emit("monocle-toggled", is_active);
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

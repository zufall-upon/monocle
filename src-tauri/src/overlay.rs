use crate::settings::AppSettings;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

#[cfg(windows)]
use windows::core::BOOL;
#[cfg(windows)]
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
#[cfg(windows)]
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
#[cfg(windows)]
use windows::Win32::Graphics::Gdi::*;
#[cfg(windows)]
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::*;

const BLUR_LAYERS: usize = 10;
// Ownership chain (bottom to top):
// [0..9]=blur layers, [10]=tint, [11]=grain
// Each window owns the next, so owned windows are always above owner.
// Pushing behind [0] = behind the entire group.
const TOTAL_WINDOWS: usize = BLUR_LAYERS + 2;
const TINT_IDX: usize = BLUR_LAYERS;     // 10
const GRAIN_IDX: usize = BLUR_LAYERS + 1; // 11
const DIM_CAP: f64 = 0.65;
const GRAIN_MAX_ALPHA: f64 = 0.20;

static ALL_HWNDS: Mutex<Vec<isize>> = Mutex::new(Vec::new());
static OVERLAY_ACTIVE: AtomicBool = AtomicBool::new(false);
static OVERLAY_PER_MONITOR: AtomicBool = AtomicBool::new(true);
static NEEDS_INITIAL_SETUP: AtomicBool = AtomicBool::new(false);
static BLUR_ON: AtomicBool = AtomicBool::new(false);
static BLUR_TASKBAR: AtomicBool = AtomicBool::new(false);

struct FadeState {
    progress: f64,
    target: f64,
    animating: bool,
}

static FADE: Mutex<FadeState> = Mutex::new(FadeState {
    progress: 0.0,
    target: 0.0,
    animating: false,
});

struct OverlayVisuals {
    tint_r: u8,
    tint_g: u8,
    tint_b: u8,
    tint_opacity: f64,
    blur_intensity: f64,
    grain_amount: f64,
}

static VISUALS: Mutex<OverlayVisuals> = Mutex::new(OverlayVisuals {
    tint_r: 0,
    tint_g: 0,
    tint_b: 0,
    tint_opacity: 0.4,
    blur_intensity: 0.6,
    grain_amount: 0.3,
});

static OVERLAY_COLOR: Mutex<u32> = Mutex::new(0);

// Noise bitmap DC for grain rendering
static NOISE_DC: Mutex<Option<isize>> = Mutex::new(None);
const NOISE_TILE: i32 = 1024;

// --- SetWindowCompositionAttribute ---

#[cfg(windows)]
#[repr(C)]
struct AccentPolicy {
    accent_state: i32,
    accent_flags: u32,
    gradient_color: u32,
    animation_id: i32,
}

#[cfg(windows)]
#[repr(C)]
struct WindowCompositionAttribData {
    attribute: i32,
    data: *mut std::ffi::c_void,
    data_size: u32,
}

#[cfg(windows)]
const WCA_ACCENT_POLICY: i32 = 19;
#[cfg(windows)]
const ACCENT_DISABLED: i32 = 0;
#[cfg(windows)]
const ACCENT_ENABLE_ACRYLICBLURBEHIND: i32 = 4;

#[cfg(windows)]
type SwcaFn = unsafe extern "system" fn(HWND, *mut WindowCompositionAttribData) -> i32;
#[cfg(windows)]
static SWCA_FN: Mutex<Option<SwcaFn>> = Mutex::new(None);

#[cfg(windows)]
fn load_composition_api() {
    unsafe {
        if let Ok(handle) = windows::Win32::System::LibraryLoader::GetModuleHandleW(
            windows::core::w!("user32.dll"),
        ) {
            if let Some(p) = windows::Win32::System::LibraryLoader::GetProcAddress(
                handle, windows::core::s!("SetWindowCompositionAttribute"),
            ) {
                *SWCA_FN.lock().unwrap() = Some(std::mem::transmute(p));
            }
        }
    }
}

#[cfg(windows)]
fn set_accent(hwnd: HWND, accent_state: i32, gradient_color: u32) {
    let swca = SWCA_FN.lock().unwrap();
    let Some(f) = *swca else { return };
    let mut policy = AccentPolicy {
        accent_state,
        accent_flags: 0,
        gradient_color,
        animation_id: 0,
    };
    let mut data = WindowCompositionAttribData {
        attribute: WCA_ACCENT_POLICY,
        data: &mut policy as *mut _ as *mut _,
        data_size: std::mem::size_of::<AccentPolicy>() as u32,
    };
    unsafe { f(hwnd, &mut data); }
}

// --- Noise bitmap generation ---

#[cfg(windows)]
unsafe fn create_noise_bitmap() {
    let screen_dc = GetDC(None);
    let mem_dc = CreateCompatibleDC(Some(screen_dc));

    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: NOISE_TILE,
            biHeight: -NOISE_TILE, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0 as u32,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let Ok(hbitmap) = CreateDIBSection(
        Some(screen_dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0,
    ) else {
        DeleteDC(mem_dc);
        ReleaseDC(None, screen_dc);
        return;
    };

    SelectObject(mem_dc, hbitmap.into());

    // Gaussian noise centered on 128 (neutral gray) using Box-Muller transform.
    // Most pixels cluster near 128 (invisible at low alpha), with occasional
    // bright/dark spots that create organic, film-like grain.
    let pixels = bits as *mut u32;
    let count = (NOISE_TILE * NOISE_TILE) as usize;
    let mut state: u32 = 0xDEADBEEF;
    let std_dev: f64 = 50.0;

    let mut i = 0;
    while i < count {
        // Generate two uniform randoms via xorshift
        state ^= state << 13; state ^= state >> 17; state ^= state << 5;
        let u1 = (state as f64) / (u32::MAX as f64);
        state ^= state << 13; state ^= state >> 17; state ^= state << 5;
        let u2 = (state as f64) / (u32::MAX as f64);

        // Box-Muller: two Gaussian samples from two uniform samples
        let u1_safe = if u1 < 1e-10 { 1e-10 } else { u1 };
        let mag = std_dev * (-2.0 * u1_safe.ln()).sqrt();
        let z0 = mag * (2.0 * std::f64::consts::PI * u2).cos();
        let z1 = mag * (2.0 * std::f64::consts::PI * u2).sin();

        let v0 = (128.0 + z0).clamp(0.0, 255.0) as u32;
        let v1 = (128.0 + z1).clamp(0.0, 255.0) as u32;

        *pixels.add(i) = 0xFF000000 | (v0 << 16) | (v0 << 8) | v0;
        i += 1;
        if i < count {
            *pixels.add(i) = 0xFF000000 | (v1 << 16) | (v1 << 8) | v1;
            i += 1;
        }
    }

    ReleaseDC(None, screen_dc);
    *NOISE_DC.lock().unwrap() = Some(mem_dc.0 as isize);
}

// --- Window helpers ---

#[cfg(windows)]
fn is_overlay(hwnd_val: isize) -> bool {
    ALL_HWNDS.lock().unwrap().contains(&hwnd_val)
}

#[cfg(windows)]
pub fn is_active() -> bool {
    OVERLAY_ACTIVE.load(Ordering::Relaxed)
}

#[cfg(windows)]
pub fn is_skip_target(hwnd: HWND) -> bool {
    should_skip_window(hwnd)
}

#[cfg(windows)]
fn should_skip_window(hwnd: HWND) -> bool {
    unsafe {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex_style & WS_EX_TOPMOST.0 != 0 { return true; }
        let mut class_buf = [0u16; 256];
        let len = GetClassNameW(hwnd, &mut class_buf);
        if len == 0 { return false; }
        let class = String::from_utf16_lossy(&class_buf[..len as usize]);
        matches!(class.as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
    }
}

#[cfg(windows)]
fn is_eligible_window(hwnd: HWND, all_hwnds: &[isize]) -> bool {
    unsafe {
        let hwnd_val = hwnd.0 as isize;
        if hwnd_val == 0 || all_hwnds.contains(&hwnd_val) { return false; }
        if !IsWindowVisible(hwnd).as_bool() { return false; }
        if should_skip_window(hwnd) { return false; }
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex_style & WS_EX_TOOLWINDOW.0 != 0 { return false; }
        let mut cloaked: u32 = 0;
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4);
        if cloaked != 0 { return false; }
        let mut rect = windows::Win32::Foundation::RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_ok() {
            let w = rect.right - rect.left;
            let h = rect.bottom - rect.top;
            if w > 50 && h > 50 { return true; }
        }
        false
    }
}

#[cfg(windows)]
fn find_top_window_per_monitor(all_hwnds: &[isize]) -> HashMap<isize, isize> {
    struct EnumState { all_hwnds: Vec<isize>, result: HashMap<isize, isize> }
    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = &mut *(lparam.0 as *mut EnumState);
        if !is_eligible_window(hwnd, &state.all_hwnds) { return BOOL(1); }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        state.result.entry(monitor.0 as isize).or_insert(hwnd.0 as isize);
        BOOL(1)
    }
    let mut state = EnumState { all_hwnds: all_hwnds.to_vec(), result: HashMap::new() };
    unsafe { let _ = EnumWindows(Some(callback), LPARAM(&mut state as *mut EnumState as isize)); }
    state.result
}

/// Find the top eligible window on a specific monitor (by z-order).
#[cfg(windows)]
fn find_top_window_on_monitor(target_mon: isize, all_hwnds: &[isize]) -> Option<isize> {
    struct EnumState { all_hwnds: Vec<isize>, target_mon: isize, result: Option<isize> }
    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = &mut *(lparam.0 as *mut EnumState);
        if state.result.is_some() { return BOOL(0); } // stop after first match
        if !is_eligible_window(hwnd, &state.all_hwnds) { return BOOL(1); }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if monitor.0 as isize == state.target_mon {
            state.result = Some(hwnd.0 as isize);
            return BOOL(0);
        }
        BOOL(1)
    }
    let mut state = EnumState { all_hwnds: all_hwnds.to_vec(), target_mon, result: None };
    unsafe { let _ = EnumWindows(Some(callback), LPARAM(&mut state as *mut EnumState as isize)); }
    state.result
}

// --- Layer management ---

const BLUR_LAYER_GRADIENT: u32 = 0x01FFFFFF;

/// Layout: [0..9]=blur, [10]=tint, [11]=grain (ownership chain, bottom to top)
#[cfg(windows)]
unsafe fn apply_all(all_hwnds: &[isize], vis: &OverlayVisuals, blur_on: bool, fade: f64) {
    let tint_hwnd = HWND(all_hwnds[TINT_IDX] as *mut _);
    let grain_hwnd = HWND(all_hwnds[GRAIN_IDX] as *mut _);

    // --- Tint window [10]: dim capped at DIM_CAP ---
    let tint_alpha = (vis.tint_opacity * DIM_CAP * fade * 255.0).clamp(0.0, 255.0) as u8;
    SetLayeredWindowAttributes(
        tint_hwnd, windows::Win32::Foundation::COLORREF(0), tint_alpha, LWA_ALPHA,
    ).ok();
    set_accent(tint_hwnd, ACCENT_DISABLED, 0);

    // --- Grain window [11] ---
    let grain_alpha = (vis.grain_amount * GRAIN_MAX_ALPHA * fade * 255.0).clamp(0.0, 255.0) as u8;
    SetLayeredWindowAttributes(
        grain_hwnd, windows::Win32::Foundation::COLORREF(0), grain_alpha, LWA_ALPHA,
    ).ok();

    // --- Blur layers [0..9] ---
    if !blur_on {
        for i in 0..BLUR_LAYERS {
            let hwnd = HWND(all_hwnds[i] as *mut _);
            SetLayeredWindowAttributes(
                hwnd, windows::Win32::Foundation::COLORREF(0), 0, LWA_ALPHA,
            ).ok();
            set_accent(hwnd, ACCENT_DISABLED, 0);
        }
        return;
    }

    let active_count = vis.blur_intensity * BLUR_LAYERS as f64;
    let full = active_count.floor() as usize;
    let frac = active_count - full as f64;
    let base_alpha = (fade * 255.0).clamp(0.0, 255.0) as u8;

    for i in 0..BLUR_LAYERS {
        let hwnd = HWND(all_hwnds[i] as *mut _);
        if i < full {
            SetLayeredWindowAttributes(
                hwnd, windows::Win32::Foundation::COLORREF(0), base_alpha, LWA_ALPHA,
            ).ok();
            set_accent(hwnd, ACCENT_ENABLE_ACRYLICBLURBEHIND, BLUR_LAYER_GRADIENT);
        } else if i == full && frac > 0.01 {
            let partial = (frac * fade * 255.0).clamp(0.0, 255.0) as u8;
            SetLayeredWindowAttributes(
                hwnd, windows::Win32::Foundation::COLORREF(0), partial, LWA_ALPHA,
            ).ok();
            set_accent(hwnd, ACCENT_ENABLE_ACRYLICBLURBEHIND, BLUR_LAYER_GRADIENT);
        } else {
            SetLayeredWindowAttributes(
                hwnd, windows::Win32::Foundation::COLORREF(0), 0, LWA_ALPHA,
            ).ok();
            set_accent(hwnd, ACCENT_DISABLED, 0);
        }
    }
}

// --- Overlay ---

#[cfg(windows)]
pub fn init() {
    load_composition_api();

    std::thread::spawn(|| {
        unsafe {
            create_noise_bitmap();

            let hinstance = GetModuleHandleW(None).unwrap();
            let class_name = windows::core::w!("MonocleOverlay");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(overlay_wnd_proc),
                hInstance: hinstance.into(),
                lpszClassName: class_name,
                ..Default::default()
            };
            RegisterClassExW(&wc);

            let sw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
            let sh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
            let sx = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let sy = GetSystemMetrics(SM_YVIRTUALSCREEN);

            let mut hwnds = Vec::with_capacity(TOTAL_WINDOWS);
            for i in 0..TOTAL_WINDOWS {
                // Each window is owned by the previous one, creating a z-order chain.
                // Windows guarantees: owned window is always above its owner.
                let owner = if i == 0 {
                    None
                } else {
                    Some(HWND(hwnds[i - 1] as *mut _))
                };
                let hwnd = CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    class_name, windows::core::w!("MonocleOverlay"),
                    WS_POPUP, sx, sy, sw, sh,
                    owner, None, Some(hinstance.into()), None,
                ).expect("Failed to create overlay window");
                SetLayeredWindowAttributes(
                    hwnd, windows::Win32::Foundation::COLORREF(0), 0, LWA_ALPHA,
                ).ok();
                hwnds.push(hwnd.0 as isize);
            }

            *ALL_HWNDS.lock().unwrap() = hwnds;
            std::thread::spawn(foreground_tracker);

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
}

#[cfg(windows)]
fn foreground_tracker() {
    let mut monitor_focused: HashMap<isize, isize> = HashMap::new();
    let mut last_fg: isize = 0;

    const FADE_MS: f64 = 300.0;
    const TICK_MS: f64 = 16.0;
    let tick = std::time::Duration::from_millis(TICK_MS as u64);

    loop {
        std::thread::sleep(tick);

        let hwnds: Vec<isize> = ALL_HWNDS.lock().unwrap().clone();
        if hwnds.is_empty() { continue; }

        unsafe {
            {
                let mut fade = FADE.lock().unwrap();
                if fade.animating {
                    let step = TICK_MS / FADE_MS;
                    if fade.target > fade.progress {
                        fade.progress = (fade.progress + step).min(fade.target);
                    } else {
                        fade.progress = (fade.progress - step).max(fade.target);
                    }

                    let vis = VISUALS.lock().unwrap();
                    let blur_on = BLUR_ON.load(Ordering::Relaxed);
                    apply_all(&hwnds, &vis, blur_on, fade.progress);

                    if (fade.progress - fade.target).abs() < 0.01 {
                        fade.progress = fade.target;
                        fade.animating = false;
                        if fade.target == 0.0 {
                            for &hwnd_val in &hwnds {
                                let hwnd = HWND(hwnd_val as *mut _);
                                set_accent(hwnd, ACCENT_DISABLED, 0);
                                let _ = ShowWindow(hwnd, SW_HIDE);
                            }
                        }
                    }
                }
            }

            if !OVERLAY_ACTIVE.load(Ordering::Relaxed) {
                last_fg = 0;
                monitor_focused.clear();
                continue;
            }

            if NEEDS_INITIAL_SETUP.swap(false, Ordering::Relaxed) {
                monitor_focused.clear();
                let top_windows = find_top_window_per_monitor(&hwnds);
                let fg = GetForegroundWindow();
                let fg_val = fg.0 as isize;
                for (&mon, &hw) in &top_windows {
                    monitor_focused.insert(mon, hw);
                    if hw != fg_val {
                        let _ = SetWindowPos(HWND(hw as *mut _), Some(fg), 0,0,0,0, SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE);
                    }
                }
                // Keep taskbar above overlay on initial setup
                if let Ok(tb) = FindWindowW(windows::core::w!("Shell_TrayWnd"), None) {
                    let _ = SetWindowPos(tb, Some(fg), 0,0,0,0, SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE);
                }
                if let Ok(tb2) = FindWindowW(windows::core::w!("Shell_SecondaryTrayWnd"), None) {
                    let _ = SetWindowPos(tb2, Some(fg), 0,0,0,0, SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE);
                }
                last_fg = fg_val;
                continue;
            }

            let fg = GetForegroundWindow();
            let fg_val = fg.0 as isize;
            if fg_val == 0 || is_overlay(fg_val) { continue; }
            if should_skip_window(fg) { continue; }

            let per_monitor = OVERLAY_PER_MONITOR.load(Ordering::Relaxed);
            let root_overlay = HWND(hwnds[0] as *mut _);

            // --- Detect tracked windows that moved to a different monitor ---
            // Runs every tick, even when fg hasn't changed (e.g. during drag)
            if per_monitor {
                let snapshot: Vec<(isize, isize)> = monitor_focused.iter()
                    .map(|(&m, &h)| (m, h)).collect();
                for (old_mon, hw) in &snapshot {
                    if !IsWindow(Some(HWND(*hw as *mut _))).as_bool() { continue; }
                    let current_mon = MonitorFromWindow(
                        HWND(*hw as *mut _), MONITOR_DEFAULTTONEAREST,
                    ).0 as isize;

                    if current_mon != *old_mon {
                        // Push down old focused window on destination monitor
                        if let Some(&dest_old) = monitor_focused.get(&current_mon) {
                            if dest_old != *hw
                                && IsWindow(Some(HWND(dest_old as *mut _))).as_bool()
                            {
                                let _ = SetWindowPos(
                                    HWND(dest_old as *mut _), Some(root_overlay),
                                    0, 0, 0, 0,
                                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                                );
                            }
                        }

                        // Move this window to new monitor's slot
                        monitor_focused.remove(old_mon);
                        monitor_focused.insert(current_mon, *hw);

                        // Find next window on vacated monitor and promote it
                        if let Some(next_hw) = find_top_window_on_monitor(*old_mon, &hwnds) {
                            if next_hw != *hw {
                                let _ = SetWindowPos(
                                    HWND(next_hw as *mut _), Some(fg),
                                    0, 0, 0, 0,
                                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                                );
                                monitor_focused.insert(*old_mon, next_hw);
                            }
                        }

                        break; // one move per tick
                    }
                }
            }

            if fg_val == last_fg { continue; }

            let monitor = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
            let monitor_key = monitor.0 as isize;

            // Only push the old window on the
            // SAME monitor behind the bottom overlay. The new fg is
            // already above the overlay (Windows brought it to top).
            if per_monitor {
                if let Some(&old_val) = monitor_focused.get(&monitor_key) {
                    if old_val != fg_val
                        && IsWindow(Some(HWND(old_val as *mut _))).as_bool()
                    {
                        let _ = SetWindowPos(
                            HWND(old_val as *mut _), Some(root_overlay),
                            0, 0, 0, 0,
                            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                        );
                    }
                }
                monitor_focused.insert(monitor_key, fg_val);
            } else {
                if last_fg != 0 && last_fg != fg_val
                    && IsWindow(Some(HWND(last_fg as *mut _))).as_bool()
                {
                    let _ = SetWindowPos(
                        HWND(last_fg as *mut _), Some(root_overlay),
                        0, 0, 0, 0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
                monitor_focused.clear();
                monitor_focused.insert(monitor_key, fg_val);
            }

            monitor_focused.retain(|_, v| IsWindow(Some(HWND(*v as *mut _))).as_bool());
            last_fg = fg_val;
        }
    }
}

/// [0]=grain (tiles noise), [1]=tint (solid color), [2..]=blur (no paint)
#[cfg(windows)]
unsafe extern "system" fn overlay_wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let hwnds = ALL_HWNDS.lock().unwrap();
            let hwnd_val = hwnd.0 as isize;

            if hwnds.get(GRAIN_IDX) == Some(&hwnd_val) {
                // Grain [11]: tile noise bitmap across the window
                let noise_dc = NOISE_DC.lock().unwrap();
                if let Some(ndc_val) = *noise_dc {
                    let ndc = HDC(ndc_val as *mut _);
                    let r = &ps.rcPaint;
                    let mut y = r.top;
                    while y < r.bottom {
                        let mut x = r.left;
                        while x < r.right {
                            let w = (r.right - x).min(NOISE_TILE);
                            let h = (r.bottom - y).min(NOISE_TILE);
                            let src_x = ((x % NOISE_TILE) + NOISE_TILE) % NOISE_TILE;
                            let src_y = ((y % NOISE_TILE) + NOISE_TILE) % NOISE_TILE;
                            let bw = w.min(NOISE_TILE - src_x);
                            let bh = h.min(NOISE_TILE - src_y);
                            let _ = BitBlt(hdc, x, y, bw, bh, Some(ndc), src_x, src_y, SRCCOPY);
                            x += bw;
                        }
                        y += NOISE_TILE;
                    }
                }
            } else if hwnds.get(TINT_IDX) == Some(&hwnd_val) {
                // Tint [10]: solid color
                let color = *OVERLAY_COLOR.lock().unwrap();
                let brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(color));
                FillRect(hdc, &ps.rcPaint, brush);
                let _ = DeleteObject(brush.into());
            }

            drop(hwnds);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_DESTROY => { PostQuitMessage(0); LRESULT(0) }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(windows)]
pub fn update_overlay(settings: &AppSettings, active: bool) {
    let was_active = OVERLAY_ACTIVE.swap(active, Ordering::Relaxed);
    OVERLAY_PER_MONITOR.store(settings.per_monitor_focus, Ordering::Relaxed);
    BLUR_ON.store(settings.blur_enabled, Ordering::Relaxed);
    BLUR_TASKBAR.store(settings.blur_taskbar, Ordering::Relaxed);

    let (r, g, b) = parse_hex_color(&settings.tint_color);
    {
        let mut vis = VISUALS.lock().unwrap();
        vis.tint_r = r; vis.tint_g = g; vis.tint_b = b;
        vis.tint_opacity = settings.tint_opacity;
        vis.blur_intensity = settings.blur_intensity;
        vis.grain_amount = settings.grain_amount;
    }
    { *OVERLAY_COLOR.lock().unwrap() = (b as u32) << 16 | (g as u32) << 8 | (r as u32); }

    let hwnds: Vec<isize> = ALL_HWNDS.lock().unwrap().clone();
    if hwnds.is_empty() { return; }

    unsafe {
        if !active {
            let mut fade = FADE.lock().unwrap();
            fade.target = 0.0;
            fade.animating = true;
            return;
        }

        if was_active {
            let vis = VISUALS.lock().unwrap();
            let fade = FADE.lock().unwrap();
            let progress = if fade.animating { fade.progress } else { 1.0 };
            drop(fade);
            apply_all(&hwnds, &vis, settings.blur_enabled, progress);
            let _ = InvalidateRect(Some(HWND(hwnds[TINT_IDX] as *mut _)), None, true);
            let _ = InvalidateRect(Some(HWND(hwnds[GRAIN_IDX] as *mut _)), None, true);
            return;
        }

        let fg = GetForegroundWindow();
        let sw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let sh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        let sx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let sy = GetSystemMetrics(SM_YVIRTUALSCREEN);

        let insert_after = if fg.0 as isize != 0 && !is_overlay(fg.0 as isize) {
            Some(fg)
        } else { Some(HWND_TOP) };

        // Position root (hwnds[0]) behind fg. Ownership chain guarantees
        // all other overlay windows stay above the root.
        for &hwnd_val in &hwnds {
            let hwnd = HWND(hwnd_val as *mut _);
            SetLayeredWindowAttributes(
                hwnd, windows::Win32::Foundation::COLORREF(0), 0, LWA_ALPHA,
            ).ok();
        }
        // Position and show root behind fg
        let root = HWND(hwnds[0] as *mut _);
        let _ = SetWindowPos(root, insert_after, sx, sy, sw, sh, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        // Show all owned windows (they position above root automatically)
        for &hwnd_val in &hwnds[1..] {
            let hwnd = HWND(hwnd_val as *mut _);
            let _ = SetWindowPos(hwnd, None, sx, sy, sw, sh, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }

        {
            let mut fade = FADE.lock().unwrap();
            fade.progress = 0.0;
            fade.target = 1.0;
            fade.animating = true;
        }
        NEEDS_INITIAL_SETUP.store(true, Ordering::Relaxed);
    }
}

fn parse_hex_color(hex: &str) -> (u8, u8, u8) {
    let hex = hex.trim_start_matches('#');
    if hex.len() < 6 { return (0, 0, 0); }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
    (r, g, b)
}

#[cfg(not(windows))]
pub fn init() {}

#[cfg(not(windows))]
pub fn update_overlay(_settings: &AppSettings, _active: bool) {}

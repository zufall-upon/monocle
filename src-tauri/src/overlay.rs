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
// (Desaturation is handled separately by the Magnification API in the
// `magnifier` module — see there.)
const TOTAL_WINDOWS: usize = BLUR_LAYERS + 2;
const TINT_IDX: usize = BLUR_LAYERS;     // 10
const GRAIN_IDX: usize = BLUR_LAYERS + 1; // 11
const DIM_CAP: f64 = 0.65;
const GRAIN_MAX_ALPHA: f64 = 0.20;

static ALL_HWNDS: Mutex<Vec<isize>> = Mutex::new(Vec::new());
static OVERLAY_ACTIVE: AtomicBool = AtomicBool::new(false);

// Every HWND we've SetWindowPos'd below root_overlay during a session.
// Bounded — drops oldest on overflow. On full deactivate we BringWindowToTop
// each one, undoing the z-order debt the tracker accumulated. Without this,
// long sessions leave a graveyard of windows stuck at the bottom of z-order
// that the user can't easily raise after Monocle is turned off.
static PUSHED_DOWN: Mutex<Vec<isize>> = Mutex::new(Vec::new());
const MAX_PUSHED: usize = 256;

static OVERLAY_PER_MONITOR: AtomicBool = AtomicBool::new(true);
static NEEDS_INITIAL_SETUP: AtomicBool = AtomicBool::new(false);
static BLUR_TASKBAR: AtomicBool = AtomicBool::new(false);
// Crossfade duration in milliseconds. Live-updated from settings via
// `update_overlay`. Clamped to >= 1 tick when read so a 0-duration
// setting still converges instead of dividing by zero.
static FADE_MS: Mutex<f64> = Mutex::new(750.0);

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

// Deep-focus <-> ambient blend animator. `progress`/`target` are 0.0 (deep
// focus) .. 1.0 (ambient). Driven each tracker tick (same cadence and
// duration as FADE) so switching modes dissolves between the two blur
// treatments instead of cutting hard. Snapped instantly on (de)activation;
// only an in-place mode toggle animates.
struct ModeState {
    progress: f64,
    target: f64,
    animating: bool,
}

static MODE: Mutex<ModeState> = Mutex::new(ModeState {
    progress: 0.0,
    target: 0.0,
    animating: false,
});

struct OverlayVisuals {
    tint_r: u8,
    tint_g: u8,
    tint_b: u8,
    tint_opacity: f64,
    grain_amount: f64,
}

static VISUALS: Mutex<OverlayVisuals> = Mutex::new(OverlayVisuals {
    tint_r: 0,
    tint_g: 0,
    tint_b: 0,
    tint_opacity: 0.4,
    grain_amount: 0.3,
});

static OVERLAY_COLOR: Mutex<u32> = Mutex::new(0);

// Grain dynamically becomes click-through when the cursor is over a
// background window's non-client area (title bar, caption buttons,
// borders) so drag-to-move and close/min/max work in one continuous
// gesture. The polling timer re-arms grain once the cursor returns to
// a client area — once WS_EX_TRANSPARENT is set, grain stops receiving
// WM_MOUSEMOVE and only WM_TIMER can detect the move-back.
static GRAIN_PASSTHROUGH: AtomicBool = AtomicBool::new(false);
const GRAIN_POLL_TIMER: usize = 1;
const GRAIN_POLL_MS: u32 = 16;

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
const ACCENT_ENABLE_BLURBEHIND: i32 = 3;
#[cfg(windows)]
const ACCENT_ENABLE_ACRYLICBLURBEHIND: i32 = 4;

// Below this intensity, the bottom layer switches from acrylic to the
// softer Aero-era BlurBehind. A single full-opacity acrylic layer (the
// effect at intensity = 0.1) is the minimum strong-blur step we can
// take; anything weaker on the acrylic path required a translucent
// acrylic layer, which lets the sharp original bleed through. BlurBehind
// at full opacity gives a true (lighter) blur with no double-vision.
const BLUR_BEHIND_THRESHOLD: f64 = 0.1;

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

/// Feed the GPU blur the focused app group's window rectangles (virtual-
/// screen coords) so it can neutralize each window's footprint in the
/// captured frame before blurring. Cross-process capture exclusion is
/// OS-blocked (SetWindowDisplayAffinity returns ACCESS_DENIED on foreign
/// windows), so instead the blur paints over the window region with its
/// surrounding background — the crisp window never enters the blur input and
/// can't smear into a halo around the raised foreground app. Dead and
/// zero-area windows are skipped.
/// Per-hwnd smoothed rect: (hwnd, left, top, right, bottom) in f32.
/// Vec (not HashMap) because `Mutex::new` needs a const initializer.
#[cfg(windows)]
static FOCUS_SMOOTH: Mutex<Vec<(isize, f32, f32, f32, f32)>> = Mutex::new(Vec::new());

#[cfg(windows)]
unsafe fn update_focus_rects(focused: &HashMap<isize, Vec<isize>>) {
    // Glide + expand: each window's emitted rect lags toward its true position
    // (lerp by ALPHA per tick) and covers the union of the lagged and current
    // rects plus a small margin. The union sweeps the gap between the trailing
    // (gliding) edge and the leading (true) edge during a drag, so the leading
    // edge never flashes a halo and the trailing edge recedes smoothly. When
    // the window is stationary the smoothed rect converges to the true one, so
    // the union collapses to the true rect (set_focus_rects then change-detects
    // and skips redundant re-syncs).
    const ALPHA: f32 = 0.35;
    const MARGIN: f32 = 2.0;

    let mut smooth = FOCUS_SMOOTH.lock().unwrap();
    let mut live: Vec<isize> = Vec::new();
    let mut rects: Vec<(i32, i32, i32, i32)> = Vec::new();

    for group in focused.values() {
        for &h in group {
            let hwnd = HWND(h as *mut _);
            if !IsWindow(Some(hwnd)).as_bool() {
                continue;
            }
            let mut r = windows::Win32::Foundation::RECT::default();
            if GetWindowRect(hwnd, &mut r).is_ok() && r.right > r.left && r.bottom > r.top {
                let (tl, tt, tr, tb) = (r.left as f32, r.top as f32, r.right as f32, r.bottom as f32);
                let (sl, st, sr, sb) = match smooth.iter_mut().find(|e| e.0 == h) {
                    Some(e) => {
                        e.1 += (tl - e.1) * ALPHA;
                        e.2 += (tt - e.2) * ALPHA;
                        e.3 += (tr - e.3) * ALPHA;
                        e.4 += (tb - e.4) * ALPHA;
                        (e.1, e.2, e.3, e.4)
                    }
                    None => {
                        smooth.push((h, tl, tt, tr, tb));
                        (tl, tt, tr, tb)
                    }
                };
                let left = tl.min(sl) - MARGIN;
                let top = tt.min(st) - MARGIN;
                let right = tr.max(sr) + MARGIN;
                let bottom = tb.max(sb) + MARGIN;
                rects.push((
                    left.floor() as i32,
                    top.floor() as i32,
                    right.ceil() as i32,
                    bottom.ceil() as i32,
                ));
                live.push(h);
            }
        }
    }

    smooth.retain(|e| live.contains(&e.0));
    drop(smooth);

    crate::gpu_blur::set_focus_rects(&rects);
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
        matches!(
            class.as_str(),
            "Shell_TrayWnd"
                | "Shell_SecondaryTrayWnd"
                // Win11 tray-overflow flyout panel
                | "TopLevelWindowForOverflowXamlIsland"
                // XAML popups (used by various Win10/11 system surfaces)
                | "Xaml_WindowedPopupClass"
                // Standard Win32 popup menu — used by TrackPopupMenu,
                // which is what every tray icon's right-click menu sits
                // on top of.
                | "#32768"
        )
    }
}

/// Reject foreground windows that don't look like real app windows the
/// user could be working in. Tray icon helpers — the hidden owner
/// window that arbitrary apps SetForegroundWindow on before showing
/// their right-click menu — fail this gate because they're invisible,
/// cloaked, or zero-sized. Catches the case the same-PID and class
/// filters miss: a foreign-process helper with an arbitrary class name.
#[cfg(windows)]
fn looks_like_real_app_window(hwnd: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() { return false; }
        let mut cloaked: u32 = 0;
        let _ = DwmGetWindowAttribute(
            hwnd, DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut _, 4,
        );
        if cloaked != 0 { return false; }
        let mut rect = windows::Win32::Foundation::RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() { return false; }
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        w >= 50 && h >= 50
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

/// Push a window below root_overlay in z-order AND remember we did so,
/// so the deactivate path can free it. The PUSHED_DOWN list is
/// move-to-back: a re-push of an existing entry moves it to the newest
/// slot. On overflow the oldest entries are dropped (their HWNDs may be
/// dead by then anyway).
#[cfg(windows)]
unsafe fn push_below_overlay(target: HWND, root_overlay: HWND) {
    let _ = SetWindowPos(
        target, Some(root_overlay), 0, 0, 0, 0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
    );
    let val = target.0 as isize;
    let mut pushed = PUSHED_DOWN.lock().unwrap();
    pushed.retain(|&h| h != val);
    pushed.push(val);
    if pushed.len() > MAX_PUSHED {
        let drop_count = pushed.len() - MAX_PUSHED;
        pushed.drain(..drop_count);
    }
}

/// Iterate every still-living window we pushed below root and bring
/// each to the top. Oldest-pushed first so the most-recently-deprioritized
/// window ends up nearest the top (mimics LRU recency).
#[cfg(windows)]
unsafe fn restore_pushed_windows() {
    let mut pushed = PUSHED_DOWN.lock().unwrap();
    for &hwnd_val in pushed.iter() {
        let hwnd = HWND(hwnd_val as *mut _);
        if IsWindow(Some(hwnd)).as_bool() {
            let _ = BringWindowToTop(hwnd);
        }
    }
    pushed.clear();
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

/// Walk the GW_OWNER chain to the top (the "root owner" of a window).
/// For an unowned top-level window this returns the window itself; for
/// an owned popup/dialog it returns its top-level owner.
#[cfg(windows)]
unsafe fn root_owner(hwnd: HWND) -> HWND {
    let mut cur = hwnd;
    loop {
        match GetWindow(cur, GW_OWNER) {
            Ok(o) if !o.0.is_null() => cur = o,
            _ => return cur,
        }
    }
}

/// All visible top-level windows that share a root owner with `fg`.
/// This is the "foreground app group" — the FG window plus any popups,
/// dialogs, or sibling dialogs the same root spawned. Keeping the whole
/// group above the overlay together means a bookmark popup, file-open
/// dialog, etc. don't get blurred along with their owner.
#[cfg(windows)]
fn fg_app_group(fg: HWND, all_hwnds: &[isize]) -> Vec<isize> {
    let fg_root_val = unsafe { root_owner(fg).0 as isize };

    struct EnumState {
        all_hwnds: Vec<isize>,
        fg_root: isize,
        result: Vec<isize>,
    }
    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = &mut *(lparam.0 as *mut EnumState);
        let val = hwnd.0 as isize;
        if state.all_hwnds.contains(&val) { return BOOL(1); }
        if !IsWindowVisible(hwnd).as_bool() { return BOOL(1); }
        let mut cloaked: u32 = 0;
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4);
        if cloaked != 0 { return BOOL(1); }
        if root_owner(hwnd).0 as isize == state.fg_root {
            state.result.push(val);
        }
        BOOL(1)
    }

    let mut state = EnumState {
        all_hwnds: all_hwnds.to_vec(),
        fg_root: fg_root_val,
        result: Vec::new(),
    };
    unsafe { let _ = EnumWindows(Some(callback), LPARAM(&mut state as *mut EnumState as isize)); }
    // Guarantee fg itself is present even if EnumWindows somehow missed it.
    let fg_val = fg.0 as isize;
    if !state.result.contains(&fg_val) {
        state.result.push(fg_val);
    }
    state.result
}

/// Find the topmost eligible window underneath the overlay at a screen
/// point. Used when grain catches a click — figures out which background
/// app the user meant to raise.
#[cfg(windows)]
fn find_window_at_point(pt: windows::Win32::Foundation::POINT, all_hwnds: &[isize]) -> Option<HWND> {
    struct EnumState {
        all_hwnds: Vec<isize>,
        pt: windows::Win32::Foundation::POINT,
        result: Option<HWND>,
    }
    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = &mut *(lparam.0 as *mut EnumState);
        if state.result.is_some() { return BOOL(0); }
        if !is_eligible_window(hwnd, &state.all_hwnds) { return BOOL(1); }
        let mut rect = windows::Win32::Foundation::RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_ok()
            && state.pt.x >= rect.left && state.pt.x < rect.right
            && state.pt.y >= rect.top && state.pt.y < rect.bottom
        {
            state.result = Some(hwnd);
            return BOOL(0);
        }
        BOOL(1)
    }
    let mut state = EnumState { all_hwnds: all_hwnds.to_vec(), pt, result: None };
    unsafe { let _ = EnumWindows(Some(callback), LPARAM(&mut state as *mut EnumState as isize)); }
    state.result
}

/// Toggle WS_EX_TRANSPARENT on the grain layer. When on, hit-testing
/// passes the cursor (and clicks) through to the window underneath.
#[cfg(windows)]
fn set_grain_passthrough_style(grain_hwnd: HWND, on: bool) {
    unsafe {
        let cur = GetWindowLongW(grain_hwnd, GWL_EXSTYLE) as u32;
        let new = if on {
            cur | WS_EX_TRANSPARENT.0
        } else {
            cur & !WS_EX_TRANSPARENT.0
        };
        if new != cur {
            SetWindowLongW(grain_hwnd, GWL_EXSTYLE, new as i32);
        }
    }
    GRAIN_PASSTHROUGH.store(on, Ordering::Relaxed);
}

/// Does the cursor want grain to be click-through? True if the window
/// underneath the overlay reports a non-client hit (title bar, caption
/// buttons, resize edges) — those should respond to one continuous
/// gesture, not be absorbed.
#[cfg(windows)]
fn cursor_wants_passthrough(pt: windows::Win32::Foundation::POINT, all_hwnds: &[isize]) -> bool {
    let target = match find_window_at_point(pt, all_hwnds) {
        Some(h) => h,
        None => return false,
    };
    let lo = (pt.x as u32) & 0xFFFF;
    let hi = (pt.y as u32) & 0xFFFF;
    let lparam_xy = ((hi << 16) | lo) as isize;
    let mut hit: usize = 0;
    unsafe {
        SendMessageTimeoutW(
            target,
            WM_NCHITTEST,
            WPARAM(0),
            LPARAM(lparam_xy),
            SMTO_ABORTIFHUNG,
            50,
            Some(&mut hit as *mut _),
        );
    }
    matches!(
        hit as u32,
        HTCAPTION | HTSYSMENU | HTMINBUTTON | HTMAXBUTTON | HTCLOSE | HTHELP
            | HTLEFT | HTRIGHT | HTTOP | HTBOTTOM
            | HTTOPLEFT | HTTOPRIGHT | HTBOTTOMLEFT | HTBOTTOMRIGHT
            | HTBORDER
    )
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
unsafe fn apply_all(all_hwnds: &[isize], vis: &OverlayVisuals, fade: f64) {
    let tint_hwnd = HWND(all_hwnds[TINT_IDX] as *mut _);
    let grain_hwnd = HWND(all_hwnds[GRAIN_IDX] as *mut _);

    // --- Tint window [10]: dim capped at DIM_CAP ---
    let tint_alpha = (vis.tint_opacity * DIM_CAP * fade * 255.0).clamp(0.0, 255.0) as u8;
    SetLayeredWindowAttributes(
        tint_hwnd, windows::Win32::Foundation::COLORREF(0), tint_alpha, LWA_ALPHA,
    ).ok();
    set_accent(tint_hwnd, ACCENT_DISABLED, 0);

    // --- Grain window [11] ---
    // Grain doubles as the input catcher (no WS_EX_TRANSPARENT). Per
    // MSDN, hit-testing on a layered window uses the LWA_ALPHA value;
    // alpha=0 lets clicks pass through. Force a 1/255 floor while the
    // overlay is visible so we always intercept clicks, even when the
    // user has dialed grain down to 0.
    let raw_grain = (vis.grain_amount * GRAIN_MAX_ALPHA * fade * 255.0).clamp(0.0, 255.0) as u8;
    let grain_alpha = if fade > 0.0 { raw_grain.max(1) } else { 0 };
    SetLayeredWindowAttributes(
        grain_hwnd, windows::Win32::Foundation::COLORREF(0), grain_alpha, LWA_ALPHA,
    ).ok();

    // --- Blur layers [0..9] ---
    // The old acrylic-blur path (SetWindowCompositionAttribute +
    // ACCENT_ENABLE_ACRYLICBLURBEHIND with a white gradient offset) is
    // retired: the GPU capture blur (gpu_blur module) now does the blur.
    // These windows are kept purely as z-order anchors for the chain, so
    // force every one of them fully transparent and accent-disabled. Blur
    // strength/mode is driven separately through gpu_blur::set_params.
    for i in 0..BLUR_LAYERS {
        let hwnd = HWND(all_hwnds[i] as *mut _);
        SetLayeredWindowAttributes(
            hwnd, windows::Win32::Foundation::COLORREF(0), 0, LWA_ALPHA,
        ).ok();
        set_accent(hwnd, ACCENT_DISABLED, 0);
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
                // The topmost layer (grain) catches mouse input so we can
                // force the cursor to an arrow and absorb stray clicks on
                // blurred background apps. All other layers stay
                // click-through.
                let ex_style = if i == GRAIN_IDX {
                    WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
                } else {
                    WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
                };
                let hwnd = CreateWindowExW(
                    ex_style,
                    class_name, windows::core::w!("MonocleOverlay"),
                    WS_POPUP, sx, sy, sw, sh,
                    owner, None, Some(hinstance.into()), None,
                ).expect("Failed to create overlay window");
                SetLayeredWindowAttributes(
                    hwnd, windows::Win32::Foundation::COLORREF(0), 0, LWA_ALPHA,
                ).ok();
                // Exclude every overlay window from screen capture so the
                // GPU blur's capture pipeline never ingests our own
                // dim/grain/blur output — it must only ever see real apps.
                let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
                hwnds.push(hwnd.0 as isize);
            }

            let hwnds_snapshot = hwnds.clone();
            *ALL_HWNDS.lock().unwrap() = hwnds;
            // Tell the magnifier which windows to exclude from its
            // capture (otherwise it would capture our own paint).
            crate::magnifier::set_filter_list(&hwnds_snapshot);
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
    // Per monitor, the set of windows currently kept above the overlay
    // — the "foreground app group" on that monitor. A group is the
    // foreground window plus any visible siblings sharing its root
    // owner (dialogs, popups, etc.). Storing the whole group means
    // focus moves within the group are no-ops, and focus moves across
    // apps push the entire previous group down at once.
    let mut monitor_focused: HashMap<isize, Vec<isize>> = HashMap::new();
    let mut last_fg: isize = 0;

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
                    let fade_ms = (*FADE_MS.lock().unwrap()).max(TICK_MS);
                    let step = TICK_MS / fade_ms;
                    if fade.target > fade.progress {
                        fade.progress = (fade.progress + step).min(fade.target);
                    } else {
                        fade.progress = (fade.progress - step).max(fade.target);
                    }

                    let vis = VISUALS.lock().unwrap();
                    // fade.progress is the linear time variable;
                    // ease_for_target picks an easing curve based on
                    // direction so fade-in and fade-out feel right.
                    let eased = ease_for_target(fade.progress, fade.target);
                    apply_all(&hwnds, &vis, eased);
                    crate::gpu_blur::set_fade(eased);

                    if (fade.progress - fade.target).abs() < 0.01 {
                        fade.progress = fade.target;
                        fade.animating = false;
                        let final_eased = ease_for_target(fade.progress, fade.target);
                        crate::gpu_blur::set_fade(final_eased);
                        if fade.target == 0.0 {
                            for &hwnd_val in &hwnds {
                                let hwnd = HWND(hwnd_val as *mut _);
                                set_accent(hwnd, ACCENT_DISABLED, 0);
                                let _ = ShowWindow(hwnd, SW_HIDE);
                            }
                            // Release every window we pushed below root during
                            // this active session. Hiding the overlay alone
                            // leaves them stuck at the bottom of z-order.
                            restore_pushed_windows();
                            // Clear the focus rects so the GPU blur stops
                            // neutralizing any window region once we're off.
                            crate::gpu_blur::set_focus_rects(&[]);
                            FOCUS_SMOOTH.lock().unwrap().clear();
                        }
                    }
                }
            }

            // Advance the deep-focus <-> ambient dissolve on the same cadence
            // and duration as the fade. Independent of the fade so a mode
            // toggle mid-fade still animates.
            {
                let mut mode = MODE.lock().unwrap();
                if mode.animating {
                    // Fixed, snappy duration — independent of the (slower)
                    // activation fade. A mode switch should feel immediate.
                    const MODE_MS: f64 = 280.0;
                    let step = TICK_MS / MODE_MS;
                    if mode.target > mode.progress {
                        mode.progress = (mode.progress + step).min(mode.target);
                    } else {
                        mode.progress = (mode.progress - step).max(mode.target);
                    }
                    // Symmetric smootherstep — a dissolve reads best easing in
                    // and out at both ends regardless of direction.
                    crate::gpu_blur::set_mode_mix(smootherstep(mode.progress));
                    if (mode.progress - mode.target).abs() < 0.01 {
                        mode.progress = mode.target;
                        mode.animating = false;
                        crate::gpu_blur::set_mode_mix(smootherstep(mode.progress));
                    }
                }
            }

            if !OVERLAY_ACTIVE.load(Ordering::Relaxed) {
                last_fg = 0;
                monitor_focused.clear();
                continue;
            }

            // Keep every GPU blur window pinned directly above the top blur
            // placeholder (hwnds[9]) — and therefore below tint/grain. The
            // OS reshuffles z-order on each click/focus change, so this
            // re-establishes the invariant every tick while active.
            crate::gpu_blur::reassert_z(hwnds[BLUR_LAYERS - 1]);

            // Feed the focused app group(s) to the GPU blur so it neutralizes
            // each window's footprint in the captured frame (no halo). Reflects
            // last tick's groups — a 16ms lag that's invisible against the
            // fade. Covers every path (initial setup, monitor transfer, focus
            // change) uniformly.
            update_focus_rects(&monitor_focused);

            if NEEDS_INITIAL_SETUP.swap(false, Ordering::Relaxed) {
                monitor_focused.clear();
                let top_windows = find_top_window_per_monitor(&hwnds);
                let fg = GetForegroundWindow();
                let fg_val = fg.0 as isize;
                // Never anchor against a topmost/shell foreground (e.g. the
                // tray flyout we activate from): inserting after it makes the
                // moved window topmost. Fall back to HWND_TOP — top of the
                // non-topmost band — which is where these belong anyway.
                let anchor = if fg_val != 0
                    && !is_overlay(fg_val)
                    && !should_skip_window(fg)
                {
                    fg
                } else { HWND_TOP };
                for (&mon, &hw) in &top_windows {
                    let group = fg_app_group(HWND(hw as *mut _), &hwnds);
                    monitor_focused.insert(mon, group);
                    if hw != fg_val {
                        let _ = SetWindowPos(HWND(hw as *mut _), Some(anchor), 0,0,0,0, SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE);
                    }
                }
                // Keep taskbar above overlay on initial setup
                if let Ok(tb) = FindWindowW(windows::core::w!("Shell_TrayWnd"), None) {
                    let _ = SetWindowPos(tb, Some(anchor), 0,0,0,0, SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE);
                }
                if let Ok(tb2) = FindWindowW(windows::core::w!("Shell_SecondaryTrayWnd"), None) {
                    let _ = SetWindowPos(tb2, Some(anchor), 0,0,0,0, SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE);
                }
                last_fg = fg_val;
                continue;
            }

            let fg = GetForegroundWindow();
            let fg_val = fg.0 as isize;
            if fg_val == 0 || is_overlay(fg_val) { continue; }
            if should_skip_window(fg) { continue; }

            // Ignore foreground changes to transient/helper windows.
            // Tray icons (ours and any other app's) work by calling
            // SetForegroundWindow on a hidden helper window before
            // showing their right-click menu — required Win32 practice
            // so the popup dismisses correctly. Treating that helper as
            // a real FG would push the user's actual app behind the
            // overlay; when the menu dismisses, the user's app is left
            // blurred. These helpers are invisible / cloaked / zero-
            // sized, so the real-app gate catches them regardless of
            // which process owns them — including our own tray helper.
            // Our visible settings window passes the gate and IS a real
            // FG the user wants promoted above the overlay.
            if !looks_like_real_app_window(fg) {
                continue;
            }

            let per_monitor = OVERLAY_PER_MONITOR.load(Ordering::Relaxed);
            let root_overlay = HWND(hwnds[0] as *mut _);

            // --- Detect tracked windows that moved to a different monitor ---
            // Runs every tick, even when fg hasn't changed (e.g. during drag).
            // Tracks via the group's primary (first) hwnd; the whole group
            // transfers together.
            if per_monitor {
                let snapshot: Vec<(isize, isize)> = monitor_focused.iter()
                    .filter_map(|(&m, v)| v.first().map(|&h| (m, h)))
                    .collect();
                for (old_mon, hw) in &snapshot {
                    if !IsWindow(Some(HWND(*hw as *mut _))).as_bool() { continue; }
                    let current_mon = MonitorFromWindow(
                        HWND(*hw as *mut _), MONITOR_DEFAULTTONEAREST,
                    ).0 as isize;

                    if current_mon != *old_mon {
                        // Push down the destination monitor's existing group
                        if let Some(dest_group) = monitor_focused.get(&current_mon).cloned() {
                            for &dest_w in &dest_group {
                                if dest_w != *hw
                                    && IsWindow(Some(HWND(dest_w as *mut _))).as_bool()
                                {
                                    push_below_overlay(
                                        HWND(dest_w as *mut _), root_overlay,
                                    );
                                }
                            }
                        }

                        // Transfer the moving group to the new monitor
                        if let Some(my_group) = monitor_focused.remove(old_mon) {
                            monitor_focused.insert(current_mon, my_group);
                        }

                        // Promote a replacement group on the vacated monitor
                        if let Some(next_hw) = find_top_window_on_monitor(*old_mon, &hwnds) {
                            if next_hw != *hw {
                                let _ = SetWindowPos(
                                    HWND(next_hw as *mut _), Some(fg),
                                    0, 0, 0, 0,
                                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                                );
                                let next_group = fg_app_group(HWND(next_hw as *mut _), &hwnds);
                                monitor_focused.insert(*old_mon, next_group);
                            }
                        }

                        break; // one move per tick
                    }
                }
            }

            if fg_val == last_fg { continue; }

            let monitor = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
            let monitor_key = monitor.0 as isize;
            let new_group = fg_app_group(fg, &hwnds);

            // Push down only what *left* the un-blurred set — windows
            // that were in the previous group but aren't part of the
            // new FG's group. A focus move within the same app (main →
            // its bookmark popup → back) is a no-op here.
            if per_monitor {
                if let Some(old_group) = monitor_focused.get(&monitor_key).cloned() {
                    for &old in &old_group {
                        if !new_group.contains(&old)
                            && IsWindow(Some(HWND(old as *mut _))).as_bool()
                        {
                            push_below_overlay(HWND(old as *mut _), root_overlay);
                        }
                    }
                }
                monitor_focused.insert(monitor_key, new_group);
            } else {
                // Single global group — push down everything tracked that
                // isn't in the new group.
                let prev: Vec<isize> = monitor_focused
                    .values().flatten().copied().collect();
                for &old in &prev {
                    if !new_group.contains(&old)
                        && IsWindow(Some(HWND(old as *mut _))).as_bool()
                    {
                        push_below_overlay(HWND(old as *mut _), root_overlay);
                    }
                }
                monitor_focused.clear();
                monitor_focused.insert(monitor_key, new_group);
            }

            monitor_focused.retain(|_, v| {
                v.retain(|h| IsWindow(Some(HWND(*h as *mut _))).as_bool());
                !v.is_empty()
            });

            // Re-assert the cross-monitor invariant. A single overlay spans
            // every monitor, but per-monitor focus needs one window *per
            // monitor* sitting above that overlay simultaneously. Pushing
            // the departed window down only fixes the monitor we just
            // touched; the OTHER monitor's focused window routinely gets
            // stranded behind the overlay when the OS reshuffles z-order on
            // a click. So after every focus change, lift every tracked
            // focused group back above the overlay. The current
            // foreground's monitor is raised last so it ends up genuinely
            // on top.
            if per_monitor {
                for (&mon, group) in monitor_focused.iter() {
                    if mon == monitor_key {
                        continue;
                    }
                    for &w in group {
                        let hw = HWND(w as *mut _);
                        if IsWindow(Some(hw)).as_bool() {
                            let _ = SetWindowPos(
                                hw, Some(HWND_TOP), 0, 0, 0, 0,
                                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                            );
                        }
                    }
                }
                if let Some(group) = monitor_focused.get(&monitor_key) {
                    for &w in group {
                        let hw = HWND(w as *mut _);
                        if IsWindow(Some(hw)).as_bool() {
                            let _ = SetWindowPos(
                                hw, Some(HWND_TOP), 0, 0, 0, 0,
                                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                            );
                        }
                    }
                }
            }

            last_fg = fg_val;
        }
    }
}

/// [0]=grain (tiles noise), [1]=tint (solid color), [2..]=blur (no paint)
#[cfg(windows)]
unsafe extern "system" fn overlay_wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Grain doubles as the input catcher for the entire overlay group.
    // Force the arrow cursor, refuse activation, and translate clicks
    // into focus-only "raise window" actions on the app underneath.
    let is_grain = {
        let hwnds = ALL_HWNDS.lock().unwrap();
        hwnds.get(GRAIN_IDX) == Some(&(hwnd.0 as isize))
    };
    if is_grain {
        match msg {
            WM_SETCURSOR => {
                if let Ok(arrow) = LoadCursorW(None, IDC_ARROW) {
                    SetCursor(Some(arrow));
                }
                return LRESULT(1);
            }
            WM_MOUSEACTIVATE => {
                return LRESULT(MA_NOACTIVATE as isize);
            }
            WM_MOUSEMOVE => {
                let mut pt = windows::Win32::Foundation::POINT::default();
                let _ = GetCursorPos(&mut pt);
                let snapshot: Vec<isize> = ALL_HWNDS.lock().unwrap().clone();
                if cursor_wants_passthrough(pt, &snapshot)
                    && !GRAIN_PASSTHROUGH.load(Ordering::Relaxed)
                {
                    set_grain_passthrough_style(hwnd, true);
                    SetTimer(Some(hwnd), GRAIN_POLL_TIMER, GRAIN_POLL_MS, None);
                }
                return LRESULT(0);
            }
            WM_TIMER => {
                if wparam.0 == GRAIN_POLL_TIMER {
                    if !IsWindowVisible(hwnd).as_bool() {
                        let _ = KillTimer(Some(hwnd), GRAIN_POLL_TIMER);
                        set_grain_passthrough_style(hwnd, false);
                        return LRESULT(0);
                    }
                    let mut pt = windows::Win32::Foundation::POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let snapshot: Vec<isize> = ALL_HWNDS.lock().unwrap().clone();
                    if !cursor_wants_passthrough(pt, &snapshot) {
                        let _ = KillTimer(Some(hwnd), GRAIN_POLL_TIMER);
                        set_grain_passthrough_style(hwnd, false);
                    }
                }
                return LRESULT(0);
            }
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN
            | WM_XBUTTONDOWN => {
                let mut pt = windows::Win32::Foundation::POINT::default();
                let _ = GetCursorPos(&mut pt);
                let snapshot: Vec<isize> = ALL_HWNDS.lock().unwrap().clone();
                if let Some(target) = find_window_at_point(pt, &snapshot) {
                    // BringWindowToTop raises within the z-order;
                    // SetForegroundWindow activates. We deliberately do NOT
                    // call SwitchToThisWindow here — its alt-tab semantics
                    // shove unrelated windows (including the other monitor's
                    // focused app) backward, which is the exact cross-
                    // monitor stranding the tracker then has to undo.
                    let _ = BringWindowToTop(target);
                    let _ = SetForegroundWindow(target);
                }
                return LRESULT(0);
            }
            WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP
            | WM_LBUTTONDBLCLK | WM_RBUTTONDBLCLK | WM_MBUTTONDBLCLK | WM_XBUTTONDBLCLK => {
                return LRESULT(0);
            }
            _ => {}
        }
    }

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
    BLUR_TASKBAR.store(settings.blur_taskbar, Ordering::Relaxed);
    *FADE_MS.lock().unwrap() = settings.fade_duration_secs * 1000.0;

    let (r, g, b) = parse_hex_color(&settings.tint_color);
    {
        let mut vis = VISUALS.lock().unwrap();
        vis.tint_r = r; vis.tint_g = g; vis.tint_b = b;
        vis.tint_opacity = settings.tint_opacity;
        vis.grain_amount = settings.grain_amount;
    }
    { *OVERLAY_COLOR.lock().unwrap() = (b as u32) << 16 | (g as u32) << 8 | (r as u32); }

    let hwnds: Vec<isize> = ALL_HWNDS.lock().unwrap().clone();
    if hwnds.is_empty() { return; }

    // Blur + desaturation are handled by the GPU blur layer: it captures
    // the screen and runs a Saturation -> Gaussian Direct2D chain. Map the
    // 0..1 slider to a Gaussian standard deviation and fold the desaturate
    // toggle into the same pass (the Magnification-API magnifier is no
    // longer driven).
    let gpu_stddev = (settings.gpu_blur_intensity as f32) * crate::gpu_blur::STDDEV_MAX;
    crate::gpu_blur::set_params(gpu_stddev, settings.desaturate_enabled);
    crate::gpu_blur::set_active(active);

    // Deep-focus <-> ambient target. Snap instantly when (de)activating so the
    // appearance is governed solely by the fade; only an in-place toggle while
    // already active dissolves between the two treatments.
    let mode_target = if settings.blur_mode == "ambient" { 1.0 } else { 0.0 };
    {
        let mut mode = MODE.lock().unwrap();
        if !active || !was_active {
            mode.progress = mode_target;
            mode.target = mode_target;
            mode.animating = false;
            crate::gpu_blur::set_mode_mix(mode_target);
        } else if mode.target != mode_target {
            mode.target = mode_target;
            mode.animating = true;
        }
    }

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
            let target = fade.target;
            drop(fade);
            apply_all(&hwnds, &vis, ease_for_target(progress, target));
            let _ = InvalidateRect(Some(HWND(hwnds[TINT_IDX] as *mut _)), None, true);
            let _ = InvalidateRect(Some(HWND(hwnds[GRAIN_IDX] as *mut _)), None, true);
            return;
        }

        let fg = GetForegroundWindow();
        let sw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let sh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        let sx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let sy = GetSystemMetrics(SM_YVIRTUALSCREEN);

        // Anchor root just behind the foreground app — but NEVER behind a
        // topmost window. The foreground at activation is often a topmost
        // shell surface (e.g. the system-tray overflow flyout, since we
        // activate from a tray click). Inserting root after a topmost window
        // makes root topmost, and the owner-owned chain then drags the whole
        // overlay + GPU-blur stack into the topmost band — so it floats above
        // every real app. should_skip_window() rejects topmost/shell/menu
        // windows; fall back to HWND_TOP (top of the *non*-topmost band) for
        // those.
        let insert_after = if fg.0 as isize != 0
            && !is_overlay(fg.0 as isize)
            && !should_skip_window(fg)
        {
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

/// Perlin's smootherstep — `6t⁵ − 15t⁴ + 10t³`. First and second
/// derivatives are zero at both endpoints, so a fade eases gently
/// into and out of the animation. Used for fade-in: gradual onset,
/// gradual settle.
fn smootherstep(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
}

/// Direction-aware easing applied to the linear `fade.progress` so the
/// visual response is asymmetric: fade-in eases at both ends
/// (smootherstep) while fade-out releases immediately then settles into
/// transparent (cubic ease-out). Symmetric easing on fade-out makes the
/// overlay feel like it lingers before letting go.
fn ease_for_target(progress: f64, target: f64) -> f64 {
    if target > 0.5 {
        smootherstep(progress)
    } else {
        // Cubic ease-out expressed in terms of the *progress* value
        // (which decreases from 1→0 during fade-out): alpha = p³.
        // At p=0.9 → 0.729 (a fast initial drop), at p=0.1 → 0.001.
        let p = progress.clamp(0.0, 1.0);
        p * p * p
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

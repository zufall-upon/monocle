use crate::settings::AppSettings;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, Ordering};
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

const BLUR_LAYERS: usize = 1;
// Ownership chain (bottom to top):
// [0]=transparent anchor, [1]=tint, [2]=grain
// Each window owns the next, so owned windows are always above owner.
// Pushing behind [0] = behind the entire group.
// Live desaturation is handled by the optional GPU renderer.
const TOTAL_WINDOWS: usize = BLUR_LAYERS + 2;
const TINT_IDX: usize = BLUR_LAYERS;
const GRAIN_IDX: usize = BLUR_LAYERS + 1;
// Grain is a sparse, signed (bipolar) speckle pushed onto the grain window
// with per-pixel premultiplied alpha (UpdateLayeredWindow). Most pixels are
// fully transparent. This caps the layer alpha at slider = 100%: 0.27 was
// chosen so full slider matches what the old 0.90 cap produced at the 30%
// slider position (0.3 x 0.90), which read as the right maximum intensity.
const GRAIN_MAX_ALPHA: f64 = 0.27;
// |z| below this -> no speck (transparent). Keeps grain sparse: a thin layer
// of sand on paper, not a full gray wash.
const GRAIN_THRESHOLD: f64 = 0.55;
// Maps |z| above the threshold to per-speck alpha (0..1).
const GRAIN_GAIN: f64 = 0.60;
// Each speck is given a random hue at this saturation (HSV), emulating color
// film's three R/G/B emulsion layers -> random colored speckles. Saturation is
// decoupled from brightness, so this controls "how colorful" independently of
// how bright/dark the speck is. ~0.5 reads as clearly tinted film grain;
// raise toward 0.8 for punchier FilmConvert-style color, drop toward 0.2 for
// near-monochrome. Jittered per speck so not every grain is equally saturated.
const GRAIN_SATURATION: f64 = 0.55;
// HSV "value" of a "brighten" speck (positive z) and a "darken" speck (negative
// z), 0..255. Bright and dark roughly balance, so grain perturbs rather than
// washes the image.
const GRAIN_LIGHT: f64 = 200.0;
const GRAIN_DARK: f64 = 35.0;
// Max HSL colorize fraction applied to the GPU pipeline at slider 100%.
const TINT_STRENGTH_MAX: f32 = 0.50;

static ALL_HWNDS: Mutex<Vec<isize>> = Mutex::new(Vec::new());
static DISPLAY_DIRTY: AtomicBool = AtomicBool::new(false);
static STATIC_MASK: AtomicBool = AtomicBool::new(true);
static MASK_PATTERN: AtomicI32 = AtomicI32::new(0);
static OVERLAY_ACTIVE: AtomicBool = AtomicBool::new(false);

// Every HWND we've SetWindowPos'd below root_overlay during a session.
// Bounded — drops oldest on overflow. On full deactivate we BringWindowToTop
// each one, undoing the z-order debt the tracker accumulated. Without this,
// long sessions leave a graveyard of windows stuck at the bottom of z-order
// that the user can't easily raise after Deep is turned off.
static PUSHED_DOWN: Mutex<Vec<isize>> = Mutex::new(Vec::new());
const MAX_PUSHED: usize = 256;

static OVERLAY_PER_MONITOR: AtomicBool = AtomicBool::new(true);
// When true, a focus anchor sharpens its whole application (every window of
// the same process), not just the active window + its owner-chain popups.
static APP_WIDE_FOCUS: AtomicBool = AtomicBool::new(false);
static NEEDS_INITIAL_SETUP: AtomicBool = AtomicBool::new(false);
// HWND of our own settings window. It's excluded from all focus/z-order
// mechanics (never an anchor, group member, or demote target) and instead
// kept above the overlay every tick, so it stays usable for tuning no matter
// what else is focused. 0 until registered at startup.
static SETTINGS_HWND: AtomicIsize = AtomicIsize::new(0);
// Lowercased executable filenames of ignored apps (e.g. "calc.exe"). Every
// window of a matching process is kept sharp (above the overlay, footprint
// neutralized in the blur), regardless of focus or any other setting. Set from
// settings via `update_overlay`. Empty is the common case and short-circuits
// all per-tick work.
static IGNORED_CONFIG: Mutex<crate::focus_policy::IgnoredConfig> = Mutex::new(crate::focus_policy::IgnoredConfig::new());
static LAST_REAL_FOREGROUND: AtomicIsize = AtomicIsize::new(0);
#[derive(Clone)]
struct FocusTrace { tracked:isize, anchors:Vec<(isize,isize)>, sharp:Vec<isize>, events:Vec<String> }
static FOCUS_TRACE: Mutex<FocusTrace> = Mutex::new(FocusTrace { tracked:0,anchors:Vec::new(),sharp:Vec::new(),events:Vec::new() });
fn publish_focus(groups:&HashMap<isize,Vec<isize>>,tracked:isize,event:&str) {
    let mut anchors:Vec<_>=groups.iter().filter_map(|(&mon,g)|g.first().map(|&id|(mon,id))).collect();
    anchors.sort_unstable();
    let mut sharp:Vec<_>=groups.values().flatten().copied().collect();sharp.sort_unstable();sharp.dedup();
    let mut trace=FOCUS_TRACE.lock().unwrap();
    if trace.tracked==tracked && trace.anchors==anchors && trace.sharp==sharp { return; }
    let millis=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
    trace.events.push(format!("unix_ms={millis} event={event} tracked={tracked:#x} anchors={anchors:x?} sharp={sharp:x?}"));
    if trace.events.len()>12 { trace.events.remove(0); }
    trace.tracked=tracked;trace.anchors=anchors;trace.sharp=sharp;
}
// Sentinel key for the ignored-app group in the focus-rect map. Real keys are
// HMONITOR handles (always positive), so -1 never collides.
const IGNORED_FOCUS_KEY: isize = -1;
// User setting: while Deep is active, switch the Windows taskbar into real
// OS auto-hide (it slides off-screen and reappears on edge-hover), rather than
// blurring it in place. Named `blur_taskbar` for settings back-compat.
static BLUR_TASKBAR: AtomicBool = AtomicBool::new(false);
// True while WE have forced the taskbar into auto-hide. Tracks our own action
// so deactivation, toggling the setting off, or quitting can put the taskbar
// back to exactly the state the user had before.
static TASKBAR_AUTOHIDDEN: AtomicBool = AtomicBool::new(false);
// The taskbar's appbar state (ABS_* bitmask) captured right before we forced
// auto-hide, so we can restore it precisely instead of guessing the default.
static SAVED_TASKBAR_STATE: AtomicI32 = AtomicI32::new(0);
// User setting: hide the desktop icons while Deep is active.
static HIDE_DESKTOP_ICONS: AtomicBool = AtomicBool::new(false);
// True while WE have hidden the desktop icons. Tracks our own action so
// deactivation (or toggling the setting off mid-session) only re-shows them
// if we were the ones that hid them — we never force-show icons the user
// had already hidden themselves.
static ICONS_HIDDEN: AtomicBool = AtomicBool::new(false);
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

// The input-catcher window (tint [10]) dynamically becomes click-through when
// the cursor is over a background window's non-client area (title bar, caption
// buttons, borders) so drag-to-move and close/min/max work in one continuous
// gesture. The polling timer re-arms the catcher once the cursor returns to a
// client area — once WS_EX_TRANSPARENT is set, the catcher stops receiving
// WM_MOUSEMOVE and only WM_TIMER can detect the move-back.


// Premultiplied-BGRA noise tile DC: the source pattern that's tiled across the
// full-screen grain surface. Built once at init.
static NOISE_DC: Mutex<Option<isize>> = Mutex::new(None);
// Full-virtual-screen premultiplied grain surface (tiled from NOISE_DC) and its
// size. Pushed onto the grain window via UpdateLayeredWindow; the slider only
// scales it (SourceConstantAlpha), so it never needs rebuilding.
static GRAIN_DC: Mutex<Option<isize>> = Mutex::new(None);
static GRAIN_SIZE: Mutex<(i32, i32)> = Mutex::new((0, 0));
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

// --- Grain surface generation ---

/// Marsaglia xorshift32 -> uniform (0,1]. Advances `state` in place.
#[cfg(windows)]
fn xorshift(state: &mut u32) -> f64 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state as f64) / (u32::MAX as f64)
}

/// HSV -> RGB. `h` in degrees [0,360), `s`/`v` in [0,1]. Returns each channel
/// in [0,255]. Used to give each grain speck a random hue at a controlled
/// saturation, so color intensity is independent of the speck's brightness.
#[cfg(windows)]
fn hsv_to_rgb(h: f64, s: f64, v: f64) -> (f64, f64, f64) {
    let h = (h.rem_euclid(360.0)) / 60.0;
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = match h as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    ((r1 + m) * 255.0, (g1 + m) * 255.0, (b1 + m) * 255.0)
}

/// Build the premultiplied-BGRA bipolar colored noise tile.
///
/// Pixel layout is 0xAARRGGBB with RGB already multiplied by alpha (the format
/// UpdateLayeredWindow expects with AC_SRC_ALPHA). Most pixels are 0 (fully
/// transparent) so the image behind is untouched. A Gaussian sample decides
/// each speck: |z| above a threshold becomes a speck, its sign picks bright vs
/// dark, and per-channel chroma jitter tints it. Bright and dark specks roughly
/// balance, so grain perturbs the image rather than washing it toward gray.
#[cfg(windows)]
unsafe fn create_noise_bitmap() {
    let screen_dc = GetDC(None);
    let mem_dc = CreateCompatibleDC(Some(screen_dc));

    let bmi = BITMAPINFO {
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
        let _ = DeleteDC(mem_dc);
        ReleaseDC(None, screen_dc);
        return;
    };

    SelectObject(mem_dc, hbitmap.into());

    let pixels = bits as *mut u32;
    let count = (NOISE_TILE * NOISE_TILE) as usize;
    let mut state: u32 = 0xDEADBEEF;

    for i in 0..count {
        // Box-Muller: one Gaussian sample (z, std-dev 1) from two uniforms.
        let u1 = xorshift(&mut state).max(1e-10);
        let u2 = xorshift(&mut state);
        let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();

        let alpha_f = ((z.abs() - GRAIN_THRESHOLD) * GRAIN_GAIN).clamp(0.0, 1.0);
        if alpha_f <= 0.0 {
            *pixels.add(i) = 0; // transparent, image untouched
            continue;
        }

        let hue = xorshift(&mut state) * 360.0;
        // Jitter saturation ±0.2 around the base so specks vary in colorfulness.
        let sat = (GRAIN_SATURATION + (xorshift(&mut state) - 0.5) * 0.4).clamp(0.0, 1.0);
        let val = if z >= 0.0 { GRAIN_LIGHT } else { GRAIN_DARK } / 255.0;
        let (r, g, b) = hsv_to_rgb(hue, sat, val);

        let a = (alpha_f * 255.0).round() as u32;
        let pr = (r * alpha_f).round() as u32; // premultiply
        let pg = (g * alpha_f).round() as u32;
        let pb = (b * alpha_f).round() as u32;
        *pixels.add(i) = (a << 24) | (pr << 16) | (pg << 8) | pb;
    }

    ReleaseDC(None, screen_dc);
    *NOISE_DC.lock().unwrap() = Some(mem_dc.0 as isize);
}

/// Tile the noise pattern across a full-virtual-screen premultiplied surface.
/// SRCCOPY copies the raw premultiplied bytes (alpha included) verbatim, so the
/// tiled surface stays a valid AC_SRC_ALPHA source. Built once at init.
#[cfg(windows)]
unsafe fn create_grain_surface(width: i32, height: i32) {
    let noise = NOISE_DC.lock().unwrap();
    let Some(ndc_val) = *noise else { return };
    let ndc = HDC(ndc_val as *mut _);
    drop(noise);

    let screen_dc = GetDC(None);
    let mem_dc = CreateCompatibleDC(Some(screen_dc));

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
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
        let _ = DeleteDC(mem_dc);
        ReleaseDC(None, screen_dc);
        return;
    };

    SelectObject(mem_dc, hbitmap.into());

    let mut y = 0;
    while y < height {
        let mut x = 0;
        while x < width {
            let w = (width - x).min(NOISE_TILE);
            let h = (height - y).min(NOISE_TILE);
            let _ = BitBlt(mem_dc, x, y, w, h, Some(ndc), 0, 0, SRCCOPY);
            x += NOISE_TILE;
        }
        y += NOISE_TILE;
    }

    ReleaseDC(None, screen_dc);
    // Replace atomically with respect to update_grain_layer. Display changes
    // can rebuild this surface repeatedly, so release the old DC and bitmap.
    let mut grain = GRAIN_DC.lock().unwrap();
    if let Some(old) = grain.replace(mem_dc.0 as isize) {
        let old_dc = HDC(old as *mut _);
        let bitmap = GetCurrentObject(old_dc, OBJ_BITMAP);
        let _ = DeleteDC(old_dc);
        let _ = DeleteObject(bitmap);
    }
    *GRAIN_SIZE.lock().unwrap() = (width, height);
}

/// Push the grain surface onto the grain window with per-pixel alpha. `strength`
/// (0..255) drives SourceConstantAlpha, scaling the whole layer in one call so
/// the baked surface never has to be regenerated as the slider moves.
#[cfg(windows)]
unsafe fn update_grain_layer(grain_hwnd: HWND, strength: u8) {
    let grain = GRAIN_DC.lock().unwrap();
    let Some(gdc_val) = *grain else { return };
    let gdc = HDC(gdc_val as *mut _);
    let (w, h) = *GRAIN_SIZE.lock().unwrap();
    if w == 0 || h == 0 { return; }

    let size = windows::Win32::Foundation::SIZE { cx: w, cy: h };
    let src = windows::Win32::Foundation::POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: 0,    // AC_SRC_OVER
        BlendFlags: 0,
        SourceConstantAlpha: strength,
        AlphaFormat: 1, // AC_SRC_ALPHA (source is premultiplied)
    };
    let _ = UpdateLayeredWindow(
        grain_hwnd,
        None,
        None,
        Some(&size),
        Some(gdc),
        Some(&src),
        windows::Win32::Foundation::COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    );
}

// --- Window helpers ---

#[cfg(windows)]
fn is_overlay(hwnd_val: isize) -> bool {
    ALL_HWNDS.lock().unwrap().contains(&hwnd_val)
}

/// Legacy focus-bound diagnostics; these rectangles do not cut holes in the GPU image.
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
    let traits = window_traits(hwnd);
    traits.shell || traits.topmost || traits.own_process
}

#[cfg(windows)]
fn window_traits(hwnd: HWND) -> crate::focus_policy::Traits {
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd,Some(&mut pid));
        let style = GetWindowLongW(hwnd,GWL_EXSTYLE) as u32;
        let mut buf=[0u16;256];
        let len=GetClassNameW(hwnd,&mut buf).max(0) as usize;
        let class=String::from_utf16_lossy(&buf[..len]);
        let mut cloaked=0u32;
        let _=DwmGetWindowAttribute(hwnd,DWMWA_CLOAKED,&mut cloaked as *mut _ as *mut _,4);
        let mut rect=windows::Win32::Foundation::RECT::default();
        let _=GetWindowRect(hwnd,&mut rect);
        crate::focus_policy::Traits {
            own_process: pid == std::process::id(),
            shell: matches!(class.as_ref(),"Shell_TrayWnd"|"Shell_SecondaryTrayWnd"|"TopLevelWindowForOverflowXamlIsland"|"Xaml_WindowedPopupClass"|"#32768"|"Progman"|"WorkerW"),
            visible:IsWindowVisible(hwnd).as_bool(), minimized:IsIconic(hwnd).as_bool(),
            cloaked:cloaked!=0, topmost:style & WS_EX_TOPMOST.0 != 0,
            no_activate:style & WS_EX_NOACTIVATE.0 != 0, tool:style & WS_EX_TOOLWINDOW.0 != 0,
            width:rect.right-rect.left, height:rect.bottom-rect.top,
        }
    }
}

#[cfg(windows)]
fn looks_like_real_app_window(hwnd: HWND) -> bool {
    crate::focus_policy::can_anchor(window_traits(hwnd))
}

#[cfg(windows)]
fn is_eligible_window(hwnd: HWND, all_hwnds: &[isize]) -> bool {
    !hwnd.0.is_null() && !all_hwnds.contains(&(hwnd.0 as isize))
        && crate::focus_policy::can_anchor(window_traits(hwnd))
}

#[cfg(windows)]
fn is_effect_target(hwnd: HWND) -> bool {
    !hwnd.0.is_null() && crate::focus_policy::receives_effect(window_traits(hwnd))
}

/// Whether a single window belongs to an ignored app (live exe lookup).
/// Short-circuits when no apps are ignored — the common case.
#[cfg(windows)]
fn is_ignored_hwnd(hwnd_val: isize) -> bool {
    let config = IGNORED_CONFIG.lock().unwrap();
    let ignored = &config.exes;
    if ignored.is_empty() {
        return false;
    }
    match crate::windows_api::exe_name_for_hwnd(hwnd_val) {
        Some(exe) => ignored.iter().any(|e| *e == exe),
        None => false,
    }
}

/// Every currently-visible window belonging to an ignored app. Empty (and
/// free) when nothing is ignored. Resolves each window's exe, so callers throttle
/// how often they invoke it rather than running it every 16ms tick.
#[cfg(windows)]
fn ignored_hwnds(ignored: Vec<String>) -> Vec<isize> {
    if ignored.is_empty() {
        return Vec::new();
    }
    struct St {
        ignored: Vec<String>,
        result: Vec<isize>,
    }
    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let st = &mut *(lparam.0 as *mut St);
        if IsWindowVisible(hwnd).as_bool() {
            if let Some(exe) = crate::windows_api::exe_name_for_hwnd(hwnd.0 as isize) {
                if st.ignored.iter().any(|e| *e == exe) {
                    st.result.push(hwnd.0 as isize);
                }
            }
        }
        BOOL(1)
    }
    let mut st = St {
        ignored,
        result: Vec::new(),
    };
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut st as *mut St as isize));
    }
    st.result
}

/// Push a window below root_overlay in z-order AND remember we did so,
/// so the deactivate path can free it. The PUSHED_DOWN list is
/// move-to-back: a re-push of an existing entry moves it to the newest
/// slot. On overflow the oldest entries are dropped (their HWNDs may be
/// dead by then anyway).
#[cfg(windows)]
unsafe fn push_below_overlay(target: HWND, root_overlay: HWND) {
    // Ignored apps are always sharp — never demote them, no matter which path
    // (focus change, monitor transfer) asked to.
    if is_ignored_hwnd(target.0 as isize) || !is_effect_target(target) {
        return;
    }
    if crate::placement_trace::lower(target,root_overlay,"focus-demotion").is_err() { return; }
    let val = target.0 as isize;
    let mut pushed = PUSHED_DOWN.lock().unwrap();
    pushed.retain(|&h| h != val);
    pushed.push(val);
    if pushed.len() > MAX_PUSHED {
        let drop_count = pushed.len() - MAX_PUSHED;
        pushed.drain(..drop_count);
    }
}

/// Every eligible top-level window, in z-order top-to-bottom (the order
/// EnumWindows yields). This is the activation snapshot: the initial-setup
/// pass splits it into a sharp block and a blurred block, re-stacking each
/// without changing any window's position relative to the others.
#[cfg(windows)]
fn enumerate_eligible_in_z_order(all_hwnds: &[isize]) -> Vec<isize> {
    enumerate_windows(all_hwnds,false)
}
#[cfg(windows)]
fn enumerate_effect_targets(all_hwnds: &[isize]) -> Vec<isize> {
    enumerate_windows(all_hwnds,true)
}
#[cfg(windows)]
fn enumerate_windows(all_hwnds: &[isize], effects: bool) -> Vec<isize> {
    struct EnumState { all_hwnds: Vec<isize>, result: Vec<isize>, effects: bool }
    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = &mut *(lparam.0 as *mut EnumState);
        if if state.effects { is_effect_target(hwnd) } else { is_eligible_window(hwnd, &state.all_hwnds) } {
            state.result.push(hwnd.0 as isize);
        }
        BOOL(1)
    }
    let mut state = EnumState { all_hwnds: all_hwnds.to_vec(), result: Vec::new(), effects };
    unsafe { let _ = EnumWindows(Some(callback), LPARAM(&mut state as *mut EnumState as isize)); }
    state.result
}

/// Every taskbar window across all monitors: the primary `Shell_TrayWnd` plus
/// one `Shell_SecondaryTrayWnd` per additional monitor. `FindWindowW` only ever
/// returns a single window of a class, so on multi-monitor setups it misses the
/// secondary taskbars — we must enumerate to catch them all.
#[cfg(windows)]
fn all_taskbar_hwnds() -> Vec<isize> {
    struct EnumState {
        result: Vec<isize>,
    }
    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = &mut *(lparam.0 as *mut EnumState);
        let mut buf = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut buf);
        if len > 0 {
            let class = String::from_utf16_lossy(&buf[..len as usize]);
            if class == "Shell_TrayWnd" || class == "Shell_SecondaryTrayWnd" {
                state.result.push(hwnd.0 as isize);
            }
        }
        BOOL(1)
    }
    let mut state = EnumState { result: Vec::new() };
    unsafe {
        let _ = EnumWindows(Some(callback), LPARAM(&mut state as *mut EnumState as isize));
    }
    state.result
}

/// Force the system taskbar into (or out of) real OS auto-hide via the shell's
/// appbar interface. Auto-hide is a single global taskbar state (covers the
/// primary and every secondary taskbar), so one message does them all. On
/// enable we snapshot the prior state first; on disable we write that snapshot
/// back, so the user's original setting (normal or always-on-top) is preserved.
/// Idempotent through TASKBAR_AUTOHIDDEN — safe to call every tick. The slide
/// itself is Windows' own auto-hide animation: trying to drive it ourselves
/// double-animated the primary taskbar (the shell re-docks it from its
/// registered position when auto-hide engages), so we let the OS own it.
#[cfg(windows)]
unsafe fn set_taskbar_autohide(enable: bool) {
    use windows::Win32::UI::Shell::{
        SHAppBarMessage, ABM_GETSTATE, ABM_SETSTATE, ABS_AUTOHIDE, APPBARDATA,
    };

    let taskbar = FindWindowW(windows::core::w!("Shell_TrayWnd"), None).unwrap_or_default();
    let mut abd = APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        hWnd: taskbar,
        ..Default::default()
    };

    if enable {
        if TASKBAR_AUTOHIDDEN.swap(true, Ordering::Relaxed) {
            return; // already auto-hidden — nothing to do
        }
        let prev = SHAppBarMessage(ABM_GETSTATE, &mut abd) as i32;
        SAVED_TASKBAR_STATE.store(prev, Ordering::Relaxed);
        abd.lParam = LPARAM(ABS_AUTOHIDE as isize);
        SHAppBarMessage(ABM_SETSTATE, &mut abd);
    } else {
        if !TASKBAR_AUTOHIDDEN.swap(false, Ordering::Relaxed) {
            return; // we never auto-hid it — leave the user's state alone
        }
        abd.lParam = LPARAM(SAVED_TASKBAR_STATE.load(Ordering::Relaxed) as isize);
        SHAppBarMessage(ABM_SETSTATE, &mut abd);
    }
}

/// Restore any shell state Deep changed (taskbar auto-hide, desktop icons).
/// Called on quit, where the deactivate fade path that normally reverts these
/// never runs.
#[cfg(windows)]
pub fn restore_shell_state() {
    unsafe {
        set_taskbar_autohide(false);
        if ICONS_HIDDEN.swap(false, Ordering::Relaxed) {
            set_desktop_icons(true);
        }
    }
}

#[cfg(not(windows))]
pub fn restore_shell_state() {}

/// Locate the desktop's `SHELLDLL_DefView` — the shell view that hosts the
/// desktop icons and owns the "Show desktop icons" toggle command. Normally a
/// direct child of `Progman`, but when a wallpaper slideshow / Spotlight is
/// running the shell reparents it under a `WorkerW`, so fall back to scanning
/// top-level windows for one with a `SHELLDLL_DefView` child.
#[cfg(windows)]
unsafe fn find_desktop_defview() -> Option<HWND> {
    if let Ok(progman) = FindWindowW(windows::core::w!("Progman"), None) {
        if let Ok(dv) = FindWindowExW(Some(progman), None, windows::core::w!("SHELLDLL_DefView"), None) {
            if !dv.0.is_null() { return Some(dv); }
        }
    }
    struct EnumState { result: isize }
    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = &mut *(lparam.0 as *mut EnumState);
        if let Ok(dv) = FindWindowExW(Some(hwnd), None, windows::core::w!("SHELLDLL_DefView"), None) {
            if !dv.0.is_null() { state.result = dv.0 as isize; return BOOL(0); }
        }
        BOOL(1)
    }
    let mut state = EnumState { result: 0 };
    let _ = EnumWindows(Some(callback), LPARAM(&mut state as *mut EnumState as isize));
    if state.result != 0 { Some(HWND(state.result as *mut _)) } else { None }
}

/// Show or hide the desktop icons by toggling the icon list view directly with
/// `ShowWindow`. This is deliberately NOT the shell's 0x7402 "Show desktop
/// icons" command: that command flips a *persistent* user setting and plays
/// Explorer's ~1s fade-in animation on show. ShowWindow is instant in both
/// directions and leaves the persistent setting alone, so if Deep ever dies
/// mid-session Explorer just repaints the icons rather than leaving them stuck
/// hidden. The per-tick reassert in the tracker re-hides them if Explorer
/// repaints while we want them gone. Idempotent: only acts when the list view's
/// current visibility differs from `show`.
#[cfg(windows)]
unsafe fn set_desktop_icons(show: bool) {
    let Some(defview) = find_desktop_defview() else { return };
    // The SysListView32 child is the actual icon grid; its visibility is the
    // ground truth for whether icons are currently shown.
    let listview = match FindWindowExW(Some(defview), None, windows::core::w!("SysListView32"), None) {
        Ok(lv) if !lv.0.is_null() => lv,
        _ => return,
    };
    if IsWindowVisible(listview).as_bool() == show { return; }
    let _ = ShowWindow(listview, if show { SW_SHOW } else { SW_HIDE });
}

/// Walk the GW_OWNER chain to the top (the "root owner" of a window).
/// For an unowned top-level window this returns the window itself; for
/// an owned popup/dialog it returns its top-level owner.
#[cfg(windows)]
unsafe fn root_owner(hwnd: HWND) -> HWND {
    let mut chain=Vec::new();
    let mut current=hwnd;
    for _ in 0..64 {
        let owner=GetWindow(current,GW_OWNER).unwrap_or_default();
        if owner.0.is_null() { break; }
        let t=window_traits(owner);
        let usable=t.visible && !t.minimized && !t.cloaked && !t.shell && !t.own_process && t.width>0 && t.height>0;
        chain.push((current.0 as isize,owner.0 as isize,usable));
        if !usable || chain.iter().any(|(id,_,_)| *id==owner.0 as isize) { break; }
        current=owner;
    }
    HWND(crate::focus_policy::visible_owner_root(hwnd.0 as isize,&chain) as *mut _)
}

#[cfg(windows)]
fn focus_diagnostics() -> String {
    unsafe {
        let hwnds=ALL_HWNDS.lock().unwrap().clone();
        let root=hwnds.first().copied().unwrap_or(0);
        let mut out=format!("per_monitor={} app_wide={} static={} foreground={:#x} root={root:#x}\n",
            OVERLAY_PER_MONITOR.load(Ordering::Relaxed),APP_WIDE_FOCUS.load(Ordering::Relaxed),
            STATIC_MASK.load(Ordering::Relaxed),GetForegroundWindow().0 as isize);
        out.push_str(&crate::placement_trace::report());
        let trace=FOCUS_TRACE.lock().unwrap().clone();
        let ignored=IGNORED_CONFIG.lock().unwrap().clone();
        out.push_str(&format!("last_real_foreground={:#x} tracked_foreground={:#x} retained_anchors={:x?} retained_sharp={:x?}\nignored_source=settings.ignored_apps ignored_revision={} normalized_ignored_exes={:?}\n",
            LAST_REAL_FOREGROUND.load(Ordering::Relaxed),trace.tracked,trace.anchors,trace.sharp,ignored.revision,ignored.exes));
        for event in &trace.events { out.push_str(event);out.push('\n'); }
        unsafe extern "system" fn collect(hwnd: HWND,param: LPARAM)->BOOL {
            if IsWindowVisible(hwnd).as_bool() {
                (*(param.0 as *mut Vec<isize>)).push(hwnd.0 as isize);
            }
            BOOL(1)
        }
        let mut windows:Vec<isize>=Vec::new();
        let _=EnumWindows(Some(collect),LPARAM(&mut windows as *mut _ as isize));
        for (rank,id) in windows.into_iter().enumerate() {
            let hwnd=HWND(id as *mut _);
            let t=window_traits(hwnd);
            let mut cls=[0u16;256];let n=GetClassNameW(hwnd,&mut cls).max(0) as usize;
            let mut pid=0;GetWindowThreadProcessId(hwnd,Some(&mut pid));
            let owner=GetWindow(hwnd,GW_OWNER).unwrap_or_default().0 as isize;
            let exe=crate::windows_api::exe_name_for_hwnd(id).unwrap_or_default();
            let matched_ignored=ignored.exes.iter().any(|entry| *entry==exe);
            out.push_str(&format!("sharp_reason={} ",crate::focus_policy::sharp_reason(id,&trace.sharp,matched_ignored,crate::focus_policy::exclusion(t))));
            out.push_str(&format!("rank={rank} hwnd={id:#x} pid={pid} exe={} class={} owner={owner:#x} family={:#x} tool={} noactivate={} topmost={} anchor={} ignored={} exclusion={:?} above_root={} gpu={}\n",
                exe,String::from_utf16_lossy(&cls[..n]),root_owner(hwnd).0 as isize,
                t.tool,t.no_activate,t.topmost,crate::focus_policy::can_anchor(t),matched_ignored,
                crate::focus_policy::exclusion(t),root!=0 && is_above_overlay(hwnd,HWND(root as *mut _)),
                crate::gpu_blur::diagnostic_order(id)));
        }
        out
    }
}
#[cfg(windows)]
pub fn diagnostic_snapshot() -> String { focus_diagnostics() }
#[cfg(not(windows))]
pub fn diagnostic_snapshot() -> String { "Windows-only diagnostics".into() }

/// Short window label for the diagnostic log: `0x1234 "Title" [exe]`.
#[cfg(windows)]
fn win_label(hwnd_val: isize) -> String {
    let exe = crate::windows_api::exe_name_for_hwnd(hwnd_val).unwrap_or_default();
    format!("{:#x} [{}]", hwnd_val, exe)
}

/// True if `w` sits above `root_overlay` in global z-order (i.e. it's sharp,
/// not behind the blur). Walks downward from `w`; reaching the overlay root
/// before the end of the chain means `w` is above it. Used to skip re-raising
/// windows that are already correctly placed — gratuitous SetWindowPos calls
/// on the unfocused monitor cause visible flicker.
#[cfg(windows)]
unsafe fn is_above_overlay(w: HWND, root_overlay: HWND) -> bool {
    let mut cur = w;
    loop {
        match GetWindow(cur, GW_HWNDNEXT) {
            Ok(next) if !next.0.is_null() => {
                if next.0 == root_overlay.0 {
                    return true;
                }
                cur = next;
            }
            _ => return false,
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
        if state.all_hwnds.contains(&val) || window_traits(hwnd).own_process { return BOOL(1); }
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() { return BOOL(1); }
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

/// Every eligible top-level window belonging to `anchor`'s process. When
/// `monitor_scope` is `Some(m)`, restrict to windows on monitor `m`; when
/// `None`, span all monitors. This is the app-wide expansion: instead of
/// just the active window, keep the whole application sharp.
#[cfg(windows)]
fn app_process_group(
    anchor: HWND,
    all_hwnds: &[isize],
    monitor_scope: Option<isize>,
) -> Vec<isize> {
    let mut anchor_pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(anchor, Some(&mut anchor_pid)); }
    if anchor_pid == 0 {
        // No process id — fall back to the owner-chain group so we never
        // strand the anchor unfocused.
        return fg_app_group(anchor, all_hwnds);
    }

    struct EnumState {
        all_hwnds: Vec<isize>,
        pid: u32,
        monitor_scope: Option<isize>,
        result: Vec<isize>,
    }
    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = &mut *(lparam.0 as *mut EnumState);
        if !is_eligible_window(hwnd, &state.all_hwnds) { return BOOL(1); }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != state.pid { return BOOL(1); }
        if let Some(m) = state.monitor_scope {
            let win_mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST).0 as isize;
            if win_mon != m { return BOOL(1); }
        }
        state.result.push(hwnd.0 as isize);
        BOOL(1)
    }

    let mut state = EnumState {
        all_hwnds: all_hwnds.to_vec(),
        pid: anchor_pid,
        monitor_scope,
        result: Vec::new(),
    };
    unsafe { let _ = EnumWindows(Some(callback), LPARAM(&mut state as *mut EnumState as isize)); }
    // Guarantee the anchor itself is present even if it momentarily failed an
    // eligibility check (e.g. mid-resize, transiently zero-sized).
    let anchor_val = anchor.0 as isize;
    if !state.result.contains(&anchor_val) {
        state.result.push(anchor_val);
    }
    state.result
}

/// The set of windows to keep sharp for a focus `anchor` on `monitor`.
/// Without app-wide focus this is the anchor's owner-chain group (the active
/// window plus its popups/dialogs) — identical to the prior behavior. With
/// app-wide focus on, it expands to the anchor's whole process, scoped to
/// `monitor` when per-monitor focus is on (so each monitor keeps only its own
/// copy of the app sharp — e.g. Figma sharp on the left while a Chrome window
/// on the same monitor stays blurred) or spanning all monitors when off.
#[cfg(windows)]
fn focused_group(anchor: HWND, all_hwnds: &[isize], monitor: isize) -> Vec<isize> {
    let group = if !APP_WIDE_FOCUS.load(Ordering::Relaxed) {
        fg_app_group(anchor,all_hwnds)
    } else {
        let scope = if OVERLAY_PER_MONITOR.load(Ordering::Relaxed) { Some(monitor) } else { None };
        app_process_group(anchor,all_hwnds,scope)
    };
    // Explicit invariant: group[0] is the real anchor, not whichever popup
    // EnumWindows returned first. Monitor transfer relies on this identity.
    crate::focus_policy::anchor_first(anchor.0 as isize,group)
}

#[cfg(windows)]
unsafe fn reconcile_focus(groups: &mut HashMap<isize,Vec<isize>>, hwnds: &[isize], ignored: &[isize], foreground: HWND) {
    let ordered = enumerate_eligible_in_z_order(hwnds);
    let windows: Vec<_> = ordered.iter().map(|&id| crate::focus_policy::Window {
        id, monitor: MonitorFromWindow(HWND(id as *mut _),MONITOR_DEFAULTTONEAREST).0 as isize,
    }).collect();
    let previous: Vec<_> = groups.iter().filter_map(|(&monitor,group)| group.first().map(|&id|(monitor,id))).collect();
    let anchors = crate::focus_policy::anchors(&previous,&windows,Some(foreground.0 as isize),OVERLAY_PER_MONITOR.load(Ordering::Relaxed));
    let refreshed: HashMap<_,_> = anchors.into_iter().map(|(monitor,id)| (monitor,focused_group(HWND(id as *mut _),hwnds,monitor))).collect();
    if *groups != refreshed { crate::logging::log("focus reconcile: lifecycle/group membership changed"); }
    *groups = refreshed;
    let sharp: Vec<_> = groups.values().flatten().copied().collect();
    let root = HWND(hwnds[0] as *mut _);
    // Reassert only actual violations, not the entire desktop every tracker tick.
    for id in enumerate_effect_targets(hwnds) {
        let hwnd = HWND(id as *mut _);
        let above = is_above_overlay(hwnd,root);
        if crate::focus_policy::must_lower(id,&sharp,ignored,above) {
            push_below_overlay(hwnd,root);
        } else if sharp.contains(&id) && !above {
            let insert_after = if is_eligible_window(foreground,hwnds) && foreground != hwnd { foreground } else { HWND_TOP };
            let _ = SetWindowPos(hwnd,Some(insert_after),0,0,0,0,SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE);
        }
    }
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

/// Layout: anchor, tint, grain (ownership chain, bottom to top)
#[cfg(windows)]
unsafe fn apply_all(all_hwnds: &[isize], vis: &OverlayVisuals, fade: f64) {
    let tint_hwnd = HWND(all_hwnds[TINT_IDX] as *mut _);
    let grain_hwnd = HWND(all_hwnds[GRAIN_IDX] as *mut _);

    // Visual opacity is independent of hit testing. Live tint is in the GPU
    // image; the GDI tint is transparent in Live mode and never catches input.
    let mask = STATIC_MASK.load(Ordering::Relaxed);
    let catcher_alpha = if fade <= 0.0 { 0 } else if mask {
        (vis.tint_opacity * fade * 255.0).clamp(1.0,255.0) as u8
    } else { 0 };
    SetLayeredWindowAttributes(
        tint_hwnd, windows::Win32::Foundation::COLORREF(0), catcher_alpha, LWA_ALPHA,
    ).ok();
    set_accent(tint_hwnd, ACCENT_DISABLED, 0);

    // --- Grain window ---
    // Per-pixel premultiplied alpha via UpdateLayeredWindow: sparse signed
    // colored specks. The slider scales the whole layer through
    // SourceConstantAlpha. Grain is click-through (WS_EX_TRANSPARENT) and never
    // gets SetLayeredWindowAttributes — that would knock it out of ULW mode.
    let grain_strength = (vis.grain_amount * GRAIN_MAX_ALPHA * fade * 255.0)
        .clamp(0.0, 255.0) as u8;
    if mask {
        let _ = ShowWindow(grain_hwnd, SW_HIDE);
        let mut grain = GRAIN_DC.lock().unwrap();
        if let Some(old) = grain.take() {
            let old_dc = HDC(old as *mut _);
            let bitmap = GetCurrentObject(old_dc, OBJ_BITMAP);
            let _ = DeleteDC(old_dc);
            let _ = DeleteObject(bitmap);
        }
    } else {
        let missing = GRAIN_DC.lock().unwrap().is_none();
        if missing {
            if NOISE_DC.lock().unwrap().is_none() { create_noise_bitmap(); }
            create_grain_surface(GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
        }
        update_grain_layer(grain_hwnd, grain_strength);
        if fade > 0.0 { let _ = ShowWindow(grain_hwnd, SW_SHOWNOACTIVATE); }
    }

    // --- Transparent z-order anchor ---
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

/// Register our own settings window so the focus engine can keep it above the
/// overlay and out of the blur/z-order mechanics. Called once at startup.
#[cfg(windows)]
pub fn register_settings_window(hwnd: isize) {
    SETTINGS_HWND.store(hwnd, Ordering::Relaxed);
}

#[cfg(not(windows))]
pub fn register_settings_window(_hwnd: isize) {}

#[cfg(windows)]
pub fn init() {
    load_composition_api();

    std::thread::spawn(|| {
        unsafe {

            let hinstance = GetModuleHandleW(None).unwrap();
            let class_name = windows::core::w!("DeepOverlay");
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

            // Static mode allocates no noise/grain bitmap. Live blur creates it lazily.

            let mut hwnds = Vec::with_capacity(TOTAL_WINDOWS);
            let mut capture_excluded = true;
            for i in 0..TOTAL_WINDOWS {
                // Each window is owned by the previous one, creating a z-order chain.
                // Windows guarantees: owned window is always above its owner.
                let owner = if i == 0 {
                    None
                } else {
                    Some(HWND(hwnds[i - 1] as *mut _))
                };
                // All visual layers are permanently layered and input-transparent.
                let ex_style = crate::input_policy::visual_ex_style(true);
                let hwnd = CreateWindowExW(
                    ex_style,
                    class_name, windows::core::w!("DeepOverlay"),
                    crate::input_policy::visual_style(), sx, sy, sw, sh,
                    owner, None, Some(hinstance.into()), None,
                ).expect("Failed to create overlay window");
                // Grain is driven exclusively by UpdateLayeredWindow; calling
                // SetLayeredWindowAttributes on it would knock it back into
                // uniform-alpha mode, so leave it untouched here.
                if i != GRAIN_IDX {
                    SetLayeredWindowAttributes(
                        hwnd, windows::Win32::Foundation::COLORREF(0), 0, LWA_ALPHA,
                    ).ok();
                }
                // Exclude every overlay window from screen capture so the
                // GPU blur's capture pipeline never ingests our own
                // dim/grain/blur output — it must only ever see real apps.
                let mut affinity = 0u32;
                let excluded = SetWindowDisplayAffinity(hwnd,WDA_EXCLUDEFROMCAPTURE)
                    .and_then(|_| GetWindowDisplayAffinity(hwnd,&mut affinity)).is_ok()
                    && affinity == WDA_EXCLUDEFROMCAPTURE.0;
                capture_excluded &= excluded;
                crate::logging::log(&format!("overlay capture exclusion hwnd={:#x} verified={} affinity={affinity:#x}",hwnd.0 as isize,excluded));
                hwnds.push(hwnd.0 as isize);
            }

            *ALL_HWNDS.lock().unwrap() = hwnds;
            crate::gpu_blur::set_capture_allowed(capture_excluded);
            if !capture_excluded { crate::logging::log("GPU blur disabled: overlay capture exclusion could not be verified"); }
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
    let mut last_observed_raw:isize = 0;
    let mut ignored_acknowledged:Option<u64> = None;
    let mut last_reconcile = std::time::Instant::now();

    // Cached set of ignored-app windows. Resolving each window's exe is too
    // costly to do every tick, so refresh it on a slower cadence (and on any
    // foreground change, handled below). Empty whenever no apps are ignored.
    let mut ignored: Vec<isize> = Vec::new();
    let mut ignored_refresh: u32 = 0;
    const IGNORED_REFRESH_TICKS: u32 = 30; // ~0.5s at 16ms

    const TICK_MS: f64 = 16.0;
    let tick = std::time::Duration::from_millis(TICK_MS as u64);
    // Animations advance by real elapsed time, not a fixed per-tick step.
    // Windows' default timer granularity rounds thread::sleep(16ms) up to
    // ~31ms, and heavy per-tick work adds more, so a fixed step made the
    // fade run ~2x slower than its configured duration.
    let mut last_tick = std::time::Instant::now();

    loop {
        std::thread::sleep(tick);
        let now = std::time::Instant::now();
        let dt_ms = now.duration_since(last_tick).as_secs_f64() * 1000.0;
        last_tick = now;

        let hwnds: Vec<isize> = ALL_HWNDS.lock().unwrap().clone();
        if hwnds.is_empty() { continue; }

        unsafe {
            crate::placement_trace::observe_next();
            if DISPLAY_DIRTY.swap(false, Ordering::AcqRel) {
                let sw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
                let sh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
                let sx = GetSystemMetrics(SM_XVIRTUALSCREEN);
                let sy = GetSystemMetrics(SM_YVIRTUALSCREEN);
                if sw > 0 && sh > 0 {
                    if !STATIC_MASK.load(Ordering::Relaxed) { create_grain_surface(sw, sh); }
                    for &value in &hwnds {
                        let _ = SetWindowPos(HWND(value as *mut _), None,
                            sx, sy, sw, sh, SWP_NOZORDER | SWP_NOACTIVATE);
                    }
                    let progress = {
                        let fade = FADE.lock().unwrap();
                        ease_for_target(fade.progress, fade.target)
                    };
                    let vis = VISUALS.lock().unwrap();
                    apply_all(&hwnds, &vis, progress);
                    NEEDS_INITIAL_SETUP.store(true, Ordering::Relaxed);
                }
            }
            {
                let mut fade = FADE.lock().unwrap();
                if fade.animating {
                    let fade_ms = (*FADE_MS.lock().unwrap()).max(TICK_MS);
                    let step = dt_ms / fade_ms;
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
                            // Just hide the overlay — don't touch any window's
                            // z-order. Activation kept every window in its
                            // original relative order with the overlay slotted
                            // in as a divider, so hiding it closes the gap and
                            // the windows are left exactly where the user last
                            // had them. Forcibly raising the blurred windows
                            // here is what made them reshuffle on deactivate.
                            PUSHED_DOWN.lock().unwrap().clear();
                            // If we forced the taskbar into auto-hide, put it
                            // back the way the user had it now that Deep is
                            // off.
                            set_taskbar_autohide(false);
                            // Bring desktop icons back once the overlay has
                            // fully faded out, so they reappear with the
                            // background rather than popping in early.
                            if ICONS_HIDDEN.swap(false, Ordering::Relaxed) {
                                set_desktop_icons(true);
                            }
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
                    let step = dt_ms / MODE_MS;
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

            // Observe real app focus even while OFF, before opening our settings.
            let observed=GetForegroundWindow();
            if observed.0 as isize != last_observed_raw {
                last_observed_raw=observed.0 as isize;
                if is_eligible_window(observed,&hwnds) {
                    LAST_REAL_FOREGROUND.store(last_observed_raw,Ordering::Relaxed);
                }
            }
            if !OVERLAY_ACTIVE.load(Ordering::Relaxed) {
                last_fg = 0;
                monitor_focused.clear();
                ignored.clear();
                ignored_acknowledged=None;
                publish_focus(&monitor_focused,last_fg,"inactive");
                continue;
            }

            let setup=NEEDS_INITIAL_SETUP.swap(false,Ordering::AcqRel);
            // Snapshot entries and revision together. A change during enumeration
            // remains pending: acknowledge the snapshot, never the latest revision.
            let snapshot=IGNORED_CONFIG.lock().unwrap().clone();
            if snapshot.needs_refresh(ignored_acknowledged,ignored_refresh==0,setup) {
                ignored=ignored_hwnds(snapshot.exes);
                ignored_acknowledged=Some(snapshot.revision);
            }
            ignored_refresh=(ignored_refresh+1)%IGNORED_REFRESH_TICKS;

            // Keep every GPU blur window pinned directly above the top blur
            // placeholder (hwnds[9]) — and therefore below tint/grain. The
            // OS reshuffles z-order on each click/focus change, so this
            // re-establishes the invariant every tick while active.
            crate::gpu_blur::reassert_z(hwnds[BLUR_LAYERS - 1]);

            // Feed the focused app group(s) to the GPU blur so it neutralizes
            // each window's footprint in the captured frame (no halo). Reflects
            // last tick's groups — a 16ms lag that's invisible against the
            // fade. Covers every path (initial setup, monitor transfer, focus
            // change) uniformly. Ignored apps are folded in under a sentinel key
            // so their footprints are neutralized too.
            if ignored.is_empty() {
                update_focus_rects(&monitor_focused);
            } else {
                let mut combined = monitor_focused.clone();
                combined.insert(IGNORED_FOCUS_KEY, ignored.clone());
                update_focus_rects(&combined);
            }

            // Taskbar auto-hide: drive the real OS auto-hide while active, and
            // undo it the moment the setting is toggled off. Both calls are
            // idempotent (guarded by TASKBAR_AUTOHIDDEN), so running them every
            // tick is cheap and self-corrects if anything changes the state.
            set_taskbar_autohide(BLUR_TASKBAR.load(Ordering::Relaxed));

            // Desktop icons: hide them while active, or re-show them if the
            // setting was toggled off mid-session. set_desktop_icons is
            // idempotent so re-asserting each tick is cheap.
            if HIDE_DESKTOP_ICONS.load(Ordering::Relaxed) {
                set_desktop_icons(false);
                ICONS_HIDDEN.store(true, Ordering::Relaxed);
            } else if ICONS_HIDDEN.swap(false, Ordering::Relaxed) {
                set_desktop_icons(true);
            }

            // Keep our settings window above the overlay (and everything else)
            // every tick so it's always usable for tuning while Deep is
            // active, regardless of which app the user focuses. It's excluded
            // from the focus engine entirely (is_eligible_window rejects it),
            // so this is the only thing that positions it.
            let settings_val = SETTINGS_HWND.load(Ordering::Relaxed);
            if settings_val != 0 {
                let sw = HWND(settings_val as *mut _);
                if IsWindowVisible(sw).as_bool() {
                    let _ = SetWindowPos(
                        sw, Some(HWND_TOP), 0, 0, 0, 0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
            }

            // Keep ignored-app windows above the overlay. Like the focused
            // groups, only lift one that has actually fallen behind the overlay,
            // so already-sharp ignored windows don't get reshuffled (flicker).
            {
                let root_overlay = HWND(hwnds[0] as *mut _);
                for &w in &ignored {
                    let hw = HWND(w as *mut _);
                    if IsWindow(Some(hw)).as_bool()
                        && IsWindowVisible(hw).as_bool()
                        && !is_above_overlay(hw, root_overlay)
                    {
                        let _ = SetWindowPos(
                            hw, Some(HWND_TOP), 0, 0, 0, 0,
                            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                        );
                    }
                }
            }

            if setup {
                let root_overlay = HWND(hwnds[0] as *mut _);
                let previous:Vec<_>=monitor_focused.iter().filter_map(|(&mon,g)|g.first().map(|&id|(mon,id))).collect();

                // Snapshot every eligible window once, top-to-bottom in z-order.
                // This is the ground truth we re-stack against — we never invent
                // a new ordering, we only split it into "sharp" (stays above the
                // overlay) and "blurred" (drops below) while preserving each
                // window's position relative to the others.
                let z_ordered = enumerate_eligible_in_z_order(&hwnds);
                let per_monitor = OVERLAY_PER_MONITOR.load(Ordering::Relaxed);

                let windows:Vec<_>=z_ordered.iter().map(|&id| crate::focus_policy::Window {
                    id,monitor:MonitorFromWindow(HWND(id as *mut _),MONITOR_DEFAULTTONEAREST).0 as isize,
                }).collect();
                let (preferred,anchors)=crate::focus_policy::setup_anchors(&previous,&windows,
                    GetForegroundWindow().0 as isize,LAST_REAL_FOREGROUND.load(Ordering::Relaxed),per_monitor);
                monitor_focused.clear();
                let mut sharp_set=std::collections::HashSet::new();
                for (mon,id) in anchors {
                    let group=focused_group(HWND(id as *mut _),&hwnds,mon);
                    sharp_set.extend(group.iter().copied());
                    monitor_focused.insert(mon,group);
                }

                // Ignored apps always stay sharp regardless of focus, so fold
                // their windows into the sharp block: the lift below raises them
                // and the blurred-drop loop skips them.
                for &w in &ignored { sharp_set.insert(w); }

                // The sharp windows in their existing relative z-order. No window
                // ever moves relative to another — we lift this whole block above
                // the overlay as a unit, so the user never sees them reshuffle.
                let sharp_ordered: Vec<isize> = z_ordered
                    .iter().copied().filter(|h| sharp_set.contains(h)).collect();

                // Record what the activation decided to keep sharp — the
                // baseline for diagnosing an unexpectedly-sharp window.
                {
                    let labels: Vec<String> = sharp_ordered.iter().map(|&h| win_label(h)).collect();
                    crate::logging::log(&format!(
                        "activate setup: per_monitor={} app_wide={} sharp({})=[{}]",
                        per_monitor,
                        APP_WIDE_FOCUS.load(Ordering::Relaxed),
                        sharp_ordered.len(),
                        labels.join(", "),
                    ));
                }

                // 1. Chain the sharp windows at the very top, preserving order.
                let mut prev = HWND_TOP;
                for &w in &sharp_ordered {
                    let hw = HWND(w as *mut _);
                    let _ = SetWindowPos(
                        hw, Some(prev), 0, 0, 0, 0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                    prev = hw;
                }

                // 2. Slide the whole overlay directly beneath the last sharp
                //    window. The owner→owned chain keeps grain/tint/blur stacked
                //    just above root, so moving root drags the entire overlay
                //    into the slot below the sharp block. With nothing sharp,
                //    park it at the top so the blur covers everything.
                let overlay_anchor =
                    if sharp_ordered.is_empty() { HWND_TOP } else { prev };
                let _ = SetWindowPos(
                    root_overlay, Some(overlay_anchor), 0, 0, 0, 0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );

                // 3. Drop every blurred window directly below the overlay, in
                //    its existing relative order, and record it so deactivation
                //    can restore it.
                {
                    let mut pushed = PUSHED_DOWN.lock().unwrap();
                    let mut prev_below = root_overlay;
                    for w in enumerate_effect_targets(&hwnds) {
                        if sharp_set.contains(&w) { continue; }
                        let hw = HWND(w as *mut _);
                        if crate::placement_trace::lower_after(hw,prev_below,root_overlay,"setup-demotion").is_err() { continue; }
                        prev_below = hw;
                        pushed.retain(|&h| h != w);
                        pushed.push(w);
                    }
                    if pushed.len() > MAX_PUSHED {
                        let drop_count = pushed.len() - MAX_PUSHED;
                        pushed.drain(..drop_count);
                    }
                }

                // Re-pin the GPU blur windows above the top blur placeholder now
                // that the overlay stack moved.
                crate::gpu_blur::reassert_z(hwnds[BLUR_LAYERS - 1]);

                // Keep the taskbar crisp above the overlay unless the user
                // opted to auto-hide it (handled by the per-tick auto-hide).
                if !BLUR_TASKBAR.load(Ordering::Relaxed) {
                    for tb_val in all_taskbar_hwnds() {
                        let _ = SetWindowPos(
                            HWND(tb_val as *mut _), Some(HWND_TOP), 0, 0, 0, 0,
                            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                        );
                    }
                }

                last_fg=preferred.unwrap_or(0);
                publish_focus(&monitor_focused,last_fg,"setup");
                continue;
            }

            let fg = GetForegroundWindow();
            let fg_val = fg.0 as isize;
            // Run before early returns. Closing/minimizing a window on another
            // monitor and nonactivating self-raise do not change GetForegroundWindow.
            // Defer a real new foreground to the immediate transition below.
            if last_reconcile.elapsed().as_millis() >= 250
                && (fg_val == last_fg || !is_eligible_window(fg,&hwnds))
            {
                reconcile_focus(&mut monitor_focused,&hwnds,&ignored,fg);
                publish_focus(&monitor_focused,last_fg,"reconcile");
                last_reconcile = std::time::Instant::now();
            }

            if fg_val == 0 || is_overlay(fg_val) { continue; }
            if should_skip_window(fg) { continue; }
            // Our settings window is outside the focus engine: focusing it must
            // not recompute groups or push the user's real app behind the
            // overlay. It's kept above the overlay by the per-tick raise above,
            // and last_fg is left untouched so returning to the real app still
            // registers as a foreground change.
            if fg_val == SETTINGS_HWND.load(Ordering::Relaxed) { continue; }

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
            // (Our settings window is handled separately above.)
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
                        crate::logging::log(&format!(
                            "monitor transfer: {} {:#x} -> {:#x}",
                            win_label(*hw), old_mon, current_mon
                        ));
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
                                let next_group = focused_group(HWND(next_hw as *mut _), &hwnds, *old_mon);
                                monitor_focused.insert(*old_mon, next_group);
                            }
                        }

                        break; // one move per tick
                    }
                }
            }

            if fg_val == last_fg {
                publish_focus(&monitor_focused,last_fg,"monitor-lifecycle");
                continue;
            }

            // Foreground changed — re-resolve ignored windows next tick so a
            // newly-opened or newly-focused ignored app goes sharp promptly
            // instead of waiting out the slow refresh cadence.
            ignored_refresh = 0;

            let monitor = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
            let monitor_key = monitor.0 as isize;
            let new_group = focused_group(fg, &hwnds, monitor_key);

            // Log the focus change and the group it sharpens. If more than one
            // window goes sharp from a single focus, the group list shows which
            // ones and (via app-wide focus) why — the trail for "two windows
            // got focus when only one should".
            {
                let members: Vec<String> = new_group.iter().map(|&h| win_label(h)).collect();
                crate::logging::log(&format!(
                    "focus -> {} mon={:#x} app_wide={} group({})=[{}]",
                    win_label(fg_val),
                    monitor_key,
                    APP_WIDE_FOCUS.load(Ordering::Relaxed),
                    new_group.len(),
                    members.join(", "),
                ));
            }

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
                v.retain(|h| { let hwnd=HWND(*h as *mut _); IsWindow(Some(hwnd)).as_bool() && IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool() });
                !v.is_empty()
            });

            // Re-assert which windows sit above the overlay. A single overlay
            // spans every monitor, so per-monitor focus needs one focused
            // group per monitor sharp simultaneously, and app-wide focus needs
            // every window of the focused app sharp.
            //
            // Two rules keep this correct without thrashing z-order:
            //   * Other monitors: only lift a group member that has actually
            //     fallen behind the overlay. Re-raising already-sharp windows
            //     on the unfocused monitor is what caused the visible flicker.
            //   * Current monitor: the OS click already put the fg on top, so
            //     tuck its app-wide siblings in *behind* the fg (not at
            //     HWND_TOP) — otherwise a sibling lands on top and the window
            //     the user actually clicked isn't the one in front.
            if per_monitor {
                for (&mon, group) in monitor_focused.iter() {
                    if mon == monitor_key {
                        continue;
                    }
                    for &w in group {
                        let hw = HWND(w as *mut _);
                        if IsWindow(Some(hw)).as_bool()
                            && !is_above_overlay(hw, root_overlay)
                        {
                            let _ = SetWindowPos(
                                hw, Some(HWND_TOP), 0, 0, 0, 0,
                                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                            );
                        }
                    }
                }
            }

            // Make sure the foreground window itself is above the overlay.
            // Usually the click that focused it already raised it (the grain
            // catcher lifts the clicked window), so this is a no-op. But focus
            // can also return *programmatically* with no click — a modal dialog
            // or file picker closing hands focus back to its owner — and then
            // nothing has lifted it, leaving the user's app blurred. Lift it
            // here so it sharpens whether it was clicked or not.
            if !is_above_overlay(fg, root_overlay) {
                let _ = SetWindowPos(
                    fg, Some(HWND_TOP), 0, 0, 0, 0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }

            if let Some(group) = monitor_focused.get(&monitor_key) {
                for &w in group {
                    if w == fg_val { continue; }
                    let hw = HWND(w as *mut _);
                    // Only lift a sibling that has actually fallen behind the
                    // overlay. Re-inserting an already-sharp sibling behind fg
                    // would reorder it relative to its peers — visible as the
                    // app's other windows reshuffling when one opens or gains
                    // focus. Leaving sharp windows untouched preserves the
                    // z-order they had when Deep turned on.
                    if IsWindow(Some(hw)).as_bool()
                        && !is_above_overlay(hw, root_overlay)
                    {
                        let _ = SetWindowPos(
                            hw, Some(fg), 0, 0, 0, 0,
                            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                        );
                    }
                }
            }

            last_fg = fg_val;
            publish_focus(&monitor_focused,last_fg,"foreground-change");
        }
    }
}

/// Tint (solid/hatch fill), grain (ULW noise),
/// [0..9]=blur (no paint).
#[cfg(windows)]
unsafe extern "system" fn overlay_wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as i32 as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_DISPLAYCHANGE => {
            // Never wait for animation/GDI locks in the UI thread: the tracker
            // can hold them while making synchronous Win32 window calls.
            DISPLAY_DIRTY.store(true, Ordering::Release);
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let hwnds = ALL_HWNDS.lock().unwrap();
            let hwnd_val = hwnd.0 as isize;

            // Grain is painted via UpdateLayeredWindow, not WM_PAINT, so
            // only the tint window draws here.
            if hwnds.get(TINT_IDX) == Some(&hwnd_val) {
                // Cached GDI solid or hatch fill; no screen pixels are sampled.
                let color = *OVERLAY_COLOR.lock().unwrap();
                let pattern = MASK_PATTERN.load(Ordering::Relaxed);
                let patterned = STATIC_MASK.load(Ordering::Relaxed) && pattern != 0;
                let brush = if patterned {
                    SetBkColor(hdc, windows::Win32::Foundation::COLORREF(color));
                    SetBkMode(hdc, OPAQUE);
                    CreateHatchBrush(if pattern == 2 { HS_CROSS } else { HS_BDIAGONAL },
                        windows::Win32::Foundation::COLORREF(0))
                } else { CreateSolidBrush(windows::Win32::Foundation::COLORREF(color)) };
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
    let live = crate::renderer_policy::is_live(&settings.effect_renderer);
    STATIC_MASK.store(!live, Ordering::Relaxed);
    MASK_PATTERN.store(match settings.mask_pattern.as_str() { "stripes" => 1, "grid" => 2, _ => 0 }, Ordering::Relaxed);
    crate::gpu_blur::set_renderer_enabled(live);
    let was_active = OVERLAY_ACTIVE.swap(active, Ordering::Relaxed);
    let prev_per_monitor = OVERLAY_PER_MONITOR.swap(settings.per_monitor_focus, Ordering::Relaxed);
    let prev_app_wide = APP_WIDE_FOCUS.swap(settings.app_wide_focus, Ordering::Relaxed);
    // A change to either focus-model toggle redefines which windows should be
    // sharp, but the tracker only recomputes groups on a foreground change.
    // When such a toggle flips while the overlay is already up, force a fresh
    // focus pass so the new model takes effect immediately instead of waiting
    // for the user to click another window. Gated on an actual value change so
    // ordinary updates (slider drags, color picks) don't reshuffle z-order.
    if active
        && (prev_per_monitor != settings.per_monitor_focus
            || prev_app_wide != settings.app_wide_focus)
    {
        NEEDS_INITIAL_SETUP.store(true, Ordering::Relaxed);
    }
    // Ignored apps: refresh the match set. If it changed while active, force a
    // fresh focus pass so newly-ignored apps pop sharp and un-ignored ones
    // blur immediately, instead of waiting for the next foreground change.
    {
        let mut config=IGNORED_CONFIG.lock().unwrap();
        if config.replace(settings.ignored_exes()) && active {
            NEEDS_INITIAL_SETUP.store(true,Ordering::Release);
        }
    }

    BLUR_TASKBAR.store(settings.blur_taskbar, Ordering::Relaxed);
    HIDE_DESKTOP_ICONS.store(settings.hide_desktop_icons, Ordering::Relaxed);
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
    // Tint colorizes the blurred background via an HSL "Color" blend in the
    // GPU pipeline (luminance preserved, hue + saturation from the tint
    // color). The opacity slider (0..1) is scaled by TINT_STRENGTH_MAX so the
    // top of the slider lands on a tasteful colorize amount rather than a full
    // duotone.
    // Tint dithers the chosen color over the blurred background (Dissolve
    // blend). The opacity slider (0..1) is scaled by TINT_STRENGTH_MAX so the
    // top of the slider lands on a 50% wash rather than a full one.
    crate::gpu_blur::set_tint(
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        settings.tint_opacity as f32 * TINT_STRENGTH_MAX,
    );
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
            let (progress, target) = {
                let fade = FADE.lock().unwrap();
                (if fade.animating { fade.progress } else { 1.0 }, fade.target)
            };
            let vis = VISUALS.lock().unwrap();
            // apply_all re-pushes the grain surface via UpdateLayeredWindow;
            // only the tint window needs a WM_PAINT repaint (its solid color).
            apply_all(&hwnds, &vis, ease_for_target(progress, target));
            let _ = InvalidateRect(Some(HWND(hwnds[TINT_IDX] as *mut _)), None, true);
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
        // all other overlay windows stay above the root. Skip grain: it's a
        // ULW window, and SetLayeredWindowAttributes would knock it out of
        // per-pixel mode (apply_all re-pushes its surface via the fade).
        for (i, &hwnd_val) in hwnds.iter().enumerate() {
            if i == GRAIN_IDX { continue; }
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
            if !live && hwnd_val == hwnds[GRAIN_IDX] { continue; }
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

/// Direction-aware easing applied to the linear `fade.progress`. Both
/// directions use a cubic ease-out so the response is immediate on each
/// edge and then settles gently — fade-in and fade-out feel symmetric.
/// Fade-in previously used smootherstep, whose zero initial slope made the
/// first ~150ms of a 0.75s fade nearly invisible; that flat onset read as a
/// dead pause between pressing the toggle and the overlay appearing.
fn ease_for_target(progress: f64, target: f64) -> f64 {
    let p = progress.clamp(0.0, 1.0);
    if target > 0.5 {
        // Fade-in: cubic ease-out, alpha = 1 − (1−p)³. Non-zero initial
        // slope (3) so the overlay starts rising the instant the fade does.
        let inv = 1.0 - p;
        1.0 - inv * inv * inv
    } else {
        // Fade-out: cubic ease-out on the *progress* value (which decreases
        // from 1→0 during fade-out): alpha = p³. At p=0.9 → 0.729 (a fast
        // initial drop), at p=0.1 → 0.001.
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

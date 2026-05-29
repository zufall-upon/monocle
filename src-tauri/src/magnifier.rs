//! Desaturation backdrop via the Windows Magnification API.
//!
//! A layered overlay can only paint pixels on top of underlying content —
//! it can't transform what's behind it. To actually desaturate background
//! apps we use the Magnification API: a host + magnifier-child pair that
//! captures a screen region, applies a 5×5 color matrix, and paints the
//! transformed result. Set magnification factor to 1.0× and the matrix to
//! a luma-weighted grayscale and the user sees an in-place desaturated
//! copy of whatever was captured.
//!
//! The magnifier host sits *below* the rest of the overlay stack so the
//! blur, tint, and grain layers paint on top of the desaturated content.
//! Our own windows (overlay layers + the magnifier host/child) are added
//! to the magnifier's filter list so they aren't part of the capture —
//! otherwise we'd feedback our own paint.

use crate::settings::AppSettings;

#[cfg(windows)]
mod imp {
    use crate::settings::AppSettings;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::Mutex;

    use windows::core::w;
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Magnification::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    static DESAT_ENABLED: AtomicBool = AtomicBool::new(false);
    static OVERLAY_ACTIVE: AtomicBool = AtomicBool::new(false);
    static HOST_HWND: AtomicIsize = AtomicIsize::new(0);
    static MAG_HWND: AtomicIsize = AtomicIsize::new(0);
    static FILTER_LIST: Mutex<Vec<isize>> = Mutex::new(Vec::new());
    // Mirrors overlay.rs's fade.progress (0.0..=1.0). Drives the host's
    // layered-window alpha so the desat backdrop fades in and out in
    // sync with blur/tint/grain instead of snapping on.
    static FADE_PROGRESS: Mutex<f64> = Mutex::new(0.0);
    static IS_SHOWN: AtomicBool = AtomicBool::new(false);

    const REFRESH_TIMER: usize = 1;
    // Capture rate for the desat backdrop. 8ms (~120Hz) keeps it in
    // step with high-refresh displays and minimizes the lag between
    // the actual screen state and the magnifier's painted copy —
    // mitigates ghosting during fast scroll / drag without changing
    // architecture.
    const REFRESH_MS: u32 = 8;

    // BT.709 luma weights. Stored as a flat 5×5 in row-major order;
    // each "row" is one input channel (R, G, B, A, constant) mapped
    // across the five output positions. All three RGB outputs are the
    // same luma combination → full grayscale.
    const GRAY_MATRIX: MAGCOLOREFFECT = MAGCOLOREFFECT {
        transform: [
            0.2126, 0.2126, 0.2126, 0.0, 0.0, // R input
            0.7152, 0.7152, 0.7152, 0.0, 0.0, // G input
            0.0722, 0.0722, 0.0722, 0.0, 0.0, // B input
            0.0,    0.0,    0.0,    1.0, 0.0, // A input
            0.0,    0.0,    0.0,    0.0, 1.0, // constant
        ],
    };

    pub fn init() {
        std::thread::spawn(|| unsafe { run_thread() });
    }

    unsafe fn run_thread() {
        if !MagInitialize().as_bool() {
            return;
        }

        let hinstance = GetModuleHandleW(None).unwrap();
        let class_name = w!("MonocleMagHost");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(host_wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let sx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let sy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let sw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let sh = GetSystemMetrics(SM_CYVIRTUALSCREEN);

        // Host: layered + transparent so mouse passes straight through
        // to whatever's above us in the overlay group (typically the
        // grain layer). Tool + noactivate so we don't show up in Alt+Tab
        // or steal focus.
        let host = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class_name,
            w!("MonocleMagnifier"),
            WS_POPUP,
            sx, sy, sw, sh,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
        .expect("magnifier host creation failed");
        // Start fully transparent. sync_visibility() drives the alpha
        // from FADE_PROGRESS once the overlay activates.
        SetLayeredWindowAttributes(host, COLORREF(0), 0, LWA_ALPHA).ok();

        // Magnifier child. WC_MAGNIFIER ("Magnifier") is registered by
        // MagInitialize on this thread.
        let mag = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("Magnifier"),
            w!("MagnifierCtrl"),
            WS_CHILD | WS_VISIBLE,
            0, 0, sw, sh,
            Some(host),
            None,
            Some(hinstance.into()),
            None,
        )
        .expect("magnifier child creation failed");

        // The magnifier child catches mouse input by default — make it
        // click-through so passthrough clicks from above (title-bar
        // drags etc.) keep flowing to the actual background apps.
        let cur = GetWindowLongPtrW(mag, GWL_EXSTYLE);
        SetWindowLongPtrW(mag, GWL_EXSTYLE, cur | WS_EX_TRANSPARENT.0 as isize);

        HOST_HWND.store(host.0 as isize, Ordering::Relaxed);
        MAG_HWND.store(mag.0 as isize, Ordering::Relaxed);

        let mut effect = GRAY_MATRIX;
        let _ = MagSetColorEffect(mag, &mut effect);

        let rect = RECT { left: sx, top: sy, right: sx + sw, bottom: sy + sh };
        let _ = MagSetWindowSource(mag, rect);

        // Apply any filter list that was registered before our windows
        // existed (overlay::init runs on its own thread).
        apply_filter_list_locked();

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let _ = MagUninitialize();
    }

    unsafe extern "system" fn host_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_TIMER => {
                if wparam.0 == REFRESH_TIMER {
                    refresh_capture();
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe fn refresh_capture() {
        let mag_val = MAG_HWND.load(Ordering::Relaxed);
        if mag_val == 0 {
            return;
        }
        let mag = HWND(mag_val as *mut _);
        let sx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let sy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let sw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let sh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        let rect = RECT { left: sx, top: sy, right: sx + sw, bottom: sy + sh };
        let _ = MagSetWindowSource(mag, rect);
    }

    pub fn apply_settings(settings: &AppSettings) {
        DESAT_ENABLED.store(settings.desaturate_enabled, Ordering::Relaxed);
        sync_visibility();
    }

    pub fn set_overlay_active(active: bool) {
        OVERLAY_ACTIVE.store(active, Ordering::Relaxed);
        sync_visibility();
    }

    pub fn set_fade_progress(progress: f64) {
        *FADE_PROGRESS.lock().unwrap() = progress.clamp(0.0, 1.0);
        sync_visibility();
    }

    pub fn set_filter_list(overlay_hwnds: &[isize]) {
        {
            let mut list = FILTER_LIST.lock().unwrap();
            *list = overlay_hwnds.to_vec();
        }
        unsafe { apply_filter_list_locked() };
    }

    pub fn position_below(reference: isize) {
        let host_val = HOST_HWND.load(Ordering::Relaxed);
        if host_val == 0 || reference == 0 {
            return;
        }
        unsafe {
            let _ = SetWindowPos(
                HWND(host_val as *mut _),
                Some(HWND(reference as *mut _)),
                0, 0, 0, 0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    unsafe fn apply_filter_list_locked() {
        let mag_val = MAG_HWND.load(Ordering::Relaxed);
        let host_val = HOST_HWND.load(Ordering::Relaxed);
        if mag_val == 0 {
            return;
        }
        let mag = HWND(mag_val as *mut _);

        let list = FILTER_LIST.lock().unwrap();
        let mut combined: Vec<HWND> = list.iter().map(|&v| HWND(v as *mut _)).collect();
        if host_val != 0 {
            combined.push(HWND(host_val as *mut _));
        }
        combined.push(mag);

        let _ = MagSetWindowFilterList(
            mag,
            MW_FILTERMODE_EXCLUDE,
            combined.len() as i32,
            combined.as_mut_ptr(),
        );
    }

    fn sync_visibility() {
        let host_val = HOST_HWND.load(Ordering::Relaxed);
        if host_val == 0 {
            return;
        }
        let host = HWND(host_val as *mut _);

        let enabled = DESAT_ENABLED.load(Ordering::Relaxed);
        let active = OVERLAY_ACTIVE.load(Ordering::Relaxed);
        let progress = *FADE_PROGRESS.lock().unwrap();

        // Visible only when the desat toggle is on, the overlay is
        // active, and there's at least *some* fade progress. Alpha is
        // driven by progress so the backdrop crossfades in lockstep
        // with blur/tint/grain.
        let visible = enabled && active && progress > 0.0;
        let alpha = if visible {
            (progress * 255.0).clamp(0.0, 255.0) as u8
        } else {
            0
        };

        unsafe {
            let _ = SetLayeredWindowAttributes(host, COLORREF(0), alpha, LWA_ALPHA);
        }

        let was_shown = IS_SHOWN.swap(visible, Ordering::Relaxed);
        if visible && !was_shown {
            unsafe {
                let _ = ShowWindow(host, SW_SHOWNOACTIVATE);
                SetTimer(Some(host), REFRESH_TIMER, REFRESH_MS, None);
                refresh_capture();
            }
        } else if !visible && was_shown {
            unsafe {
                let _ = KillTimer(Some(host), REFRESH_TIMER);
                let _ = ShowWindow(host, SW_HIDE);
            }
        }
    }
}

#[cfg(windows)]
pub fn init() { imp::init() }
#[cfg(windows)]
pub fn apply_settings(s: &AppSettings) { imp::apply_settings(s) }
#[cfg(windows)]
pub fn set_overlay_active(active: bool) { imp::set_overlay_active(active) }
#[cfg(windows)]
pub fn set_filter_list(hwnds: &[isize]) { imp::set_filter_list(hwnds) }
#[cfg(windows)]
pub fn position_below(reference: isize) { imp::position_below(reference) }
#[cfg(windows)]
pub fn set_fade_progress(progress: f64) { imp::set_fade_progress(progress) }

#[cfg(not(windows))]
pub fn init() {}
#[cfg(not(windows))]
pub fn apply_settings(_s: &AppSettings) {}
#[cfg(not(windows))]
pub fn set_overlay_active(_active: bool) {}
#[cfg(not(windows))]
pub fn set_filter_list(_hwnds: &[isize]) {}
#[cfg(not(windows))]
pub fn position_below(_reference: isize) {}
#[cfg(not(windows))]
pub fn set_fade_progress(_progress: f64) {}

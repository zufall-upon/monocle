#[cfg(windows)]
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
#[cfg(windows)]
use windows::core::BOOL;
#[cfg(windows)]
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowLongW, GetWindowRect, IsWindowVisible,
    GWL_EXSTYLE, GWL_STYLE, WS_EX_TOOLWINDOW, WS_VISIBLE,
};

#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub rect: (i32, i32, i32, i32),
}

#[cfg(windows)]
pub fn get_foreground_hwnd() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
}

#[cfg(windows)]
pub fn get_visible_windows() -> Vec<WindowInfo> {
    let mut results: Vec<WindowInfo> = Vec::new();

    unsafe {
        let _ = EnumWindows(
            Some(enum_callback),
            LPARAM(&mut results as *mut Vec<WindowInfo> as isize),
        );
    }

    results
}

#[cfg(windows)]
unsafe extern "system" fn enum_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let results = &mut *(lparam.0 as *mut Vec<WindowInfo>);

    if !IsWindowVisible(hwnd).as_bool() {
        return BOOL(1);
    }

    let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;

    if style & WS_VISIBLE.0 == 0 {
        return BOOL(1);
    }
    if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
        return BOOL(1);
    }

    // Skip cloaked windows (virtual desktops, etc.)
    let mut cloaked: u32 = 0;
    let _ = DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut u32 as *mut _,
        std::mem::size_of::<u32>() as u32,
    );
    if cloaked != 0 {
        return BOOL(1);
    }

    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_ok() {
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        if w > 0 && h > 0 {
            results.push(WindowInfo {
                hwnd: hwnd.0 as isize,
                rect: (rect.left, rect.top, rect.right, rect.bottom),
            });
        }
    }

    BOOL(1)
}

#[cfg(not(windows))]
pub fn get_foreground_hwnd() -> isize {
    0
}

#[cfg(not(windows))]
pub fn get_visible_windows() -> Vec<WindowInfo> {
    Vec::new()
}

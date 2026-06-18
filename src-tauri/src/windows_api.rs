//! Window/process identity helpers used by the ignored-apps feature: resolve
//! an HWND to its executable, derive a friendly app name from the exe's
//! version info, enumerate listable app windows, and find the topmost real
//! app (for the settings quick-add card).

use crate::settings::IgnoredApp;

#[cfg(windows)]
use windows::core::{BOOL, PCWSTR, PWSTR};
#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
#[cfg(windows)]
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
#[cfg(windows)]
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowLongW, GetWindowTextLengthW, GetWindowThreadProcessId,
    IsWindowVisible, GWL_EXSTYLE, GWL_STYLE, GW_OWNER, WS_EX_TOOLWINDOW, WS_VISIBLE,
};

/// Our own executable — skipped when listing/identifying apps so the overlay
/// windows and the settings window (all in this process) never appear.
const SELF_EXE: &str = "deep.exe";

fn file_name_lower(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_lowercase()
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Full path to the executable backing `hwnd`'s window, if resolvable.
#[cfg(windows)]
pub fn exe_path_for_hwnd(hwnd: isize) -> Option<String> {
    unsafe {
        let hwnd = HWND(hwnd as *mut _);
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        let res = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        res.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Lowercased executable filename for `hwnd` (e.g. "calc.exe") — the match key
/// used to decide whether a window belongs to an ignored app.
#[cfg(windows)]
pub fn exe_name_for_hwnd(hwnd: isize) -> Option<String> {
    exe_path_for_hwnd(hwnd).map(|p| file_name_lower(&p))
}

/// Friendly display name for an executable: its version-info FileDescription
/// (e.g. "Visual Studio Code"), falling back to the capitalized filename stem.
pub fn friendly_name_for_path(path: &str) -> String {
    #[cfg(windows)]
    {
        if let Some(desc) = file_description(path) {
            let desc = desc.trim();
            if !desc.is_empty() {
                return desc.to_string();
            }
        }
    }
    let stem = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let stem = stem
        .strip_suffix(".exe")
        .or_else(|| stem.strip_suffix(".EXE"))
        .unwrap_or(stem);
    capitalize(stem)
}

#[cfg(windows)]
fn file_description(path: &str) -> Option<String> {
    unsafe {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let pcw = PCWSTR(wide.as_ptr());

        let size = GetFileVersionInfoSizeW(pcw, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(pcw, Some(0), size, data.as_mut_ptr() as *mut _).ok()?;

        // Resolve the file's language/codepage so we query the right string
        // table (rather than guessing the common 040904b0).
        let mut tr_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut tr_len: u32 = 0;
        let trans_q: Vec<u16> = "\\VarFileInfo\\Translation"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        if !VerQueryValueW(
            data.as_ptr() as *const _,
            PCWSTR(trans_q.as_ptr()),
            &mut tr_ptr,
            &mut tr_len,
        )
        .as_bool()
            || tr_len < 4
        {
            return None;
        }
        let lang = *(tr_ptr as *const u16);
        let cp = *((tr_ptr as *const u16).add(1));

        let sub = format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\FileDescription");
        let sub_q: Vec<u16> = sub.encode_utf16().chain(std::iter::once(0)).collect();
        let mut val_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut val_len: u32 = 0;
        if !VerQueryValueW(
            data.as_ptr() as *const _,
            PCWSTR(sub_q.as_ptr()),
            &mut val_ptr,
            &mut val_len,
        )
        .as_bool()
            || val_len == 0
        {
            return None;
        }
        let slice = std::slice::from_raw_parts(val_ptr as *const u16, val_len as usize);
        let s = String::from_utf16_lossy(slice);
        Some(s.trim_end_matches('\0').trim().to_string())
    }
}

/// A top-level window the user could plausibly want to ignore: visible, titled,
/// not a tool window, not cloaked, and not owned by another window.
#[cfg(windows)]
unsafe fn is_listable(hwnd: HWND) -> bool {
    if !IsWindowVisible(hwnd).as_bool() {
        return false;
    }
    let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if style & WS_VISIBLE.0 == 0 {
        return false;
    }
    if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
        return false;
    }
    if !GetWindow(hwnd, GW_OWNER).unwrap_or_default().is_invalid() {
        return false;
    }
    if GetWindowTextLengthW(hwnd) == 0 {
        return false;
    }
    let mut cloaked: u32 = 0;
    let _ = DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut u32 as *mut _,
        std::mem::size_of::<u32>() as u32,
    );
    cloaked == 0
}

#[cfg(windows)]
unsafe extern "system" fn list_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = &mut *(lparam.0 as *mut Vec<isize>);
    if is_listable(hwnd) {
        out.push(hwnd.0 as isize);
    }
    BOOL(1)
}

#[cfg(windows)]
fn resolve_app(hwnd: isize) -> Option<IgnoredApp> {
    let path = exe_path_for_hwnd(hwnd)?;
    let exe = file_name_lower(&path);
    if exe.is_empty() || exe == SELF_EXE {
        return None;
    }
    Some(IgnoredApp {
        name: friendly_name_for_path(&path),
        exe,
    })
}

/// Every currently-listable app, deduped by executable and sorted by name.
/// Backs the settings "add ignored app" picker.
#[cfg(windows)]
pub fn list_app_windows() -> Vec<IgnoredApp> {
    let mut hwnds: Vec<isize> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(list_cb), LPARAM(&mut hwnds as *mut _ as isize));
    }
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<IgnoredApp> = Vec::new();
    for h in hwnds {
        if let Some(app) = resolve_app(h) {
            if seen.insert(app.exe.clone()) {
                out.push(app);
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// The topmost real app, excluding our own windows (overlay + settings). Used
/// for the quick-add card. EnumWindows yields top-level windows in z-order
/// (front to back), so the first self-excluded listable window is frontmost.
#[cfg(windows)]
pub fn foreground_app() -> Option<IgnoredApp> {
    let mut hwnds: Vec<isize> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(list_cb), LPARAM(&mut hwnds as *mut _ as isize));
    }
    hwnds.into_iter().find_map(resolve_app)
}

#[cfg(not(windows))]
pub fn exe_name_for_hwnd(_hwnd: isize) -> Option<String> {
    None
}

#[cfg(not(windows))]
pub fn list_app_windows() -> Vec<IgnoredApp> {
    Vec::new()
}

#[cfg(not(windows))]
pub fn foreground_app() -> Option<IgnoredApp> {
    None
}

//! Absorbs the first mouse click that would raise a blurred background
//! window — the window is focused but the click never reaches its
//! controls. Subsequent clicks behave normally.
//!
//! Implemented as a global low-level mouse hook (WH_MOUSE_LL). The
//! hook thread must run a message pump for the hook to fire.

#![cfg(windows)]

use crate::overlay;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::*;

// LLMHF_INJECTED is 0x1 — synthetic input from SendInput. We never
// absorb injected events so automation / accessibility tools still work.
const LLMHF_INJECTED_FLAG: u32 = 0x00000001;

// Tracks the DOWN message we last absorbed, so we also swallow its
// matching UP. 0 = nothing currently absorbed.
static ABSORBED_DOWN: AtomicU32 = AtomicU32::new(0);

fn matching_down(up_msg: u32) -> u32 {
    match up_msg {
        WM_LBUTTONUP => WM_LBUTTONDOWN,
        WM_RBUTTONUP => WM_RBUTTONDOWN,
        WM_MBUTTONUP => WM_MBUTTONDOWN,
        _ => 0,
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let msg = wparam.0 as u32;
    let info_ptr = lparam.0 as *const MSLLHOOKSTRUCT;
    if info_ptr.is_null() {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let info = &*info_ptr;

    // Synthetic input — pass through.
    if info.flags & LLMHF_INJECTED_FLAG != 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let is_down = matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN);
    let is_up = matches!(msg, WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP);
    if !is_down && !is_up {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    if is_up {
        let pair = matching_down(msg);
        let absorbed = ABSORBED_DOWN.load(Ordering::Relaxed);
        if pair != 0 && absorbed == pair {
            ABSORBED_DOWN.store(0, Ordering::Relaxed);
            return LRESULT(1);
        }
        return CallNextHookEx(None, code, wparam, lparam);
    }

    // is_down — decide whether to absorb.
    if !overlay::is_active() {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let pt = POINT { x: info.pt.x, y: info.pt.y };
    let target = WindowFromPoint(pt);
    if target.0.is_null() {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let root = GetAncestor(target, GA_ROOT);
    if root.0.is_null() {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    // Don't touch our own windows (overlay layers, settings UI).
    let mut target_pid: u32 = 0;
    GetWindowThreadProcessId(root, Some(&mut target_pid));
    if target_pid == GetCurrentProcessId() {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    // Click on the already-focused window: normal interaction.
    let fg = GetForegroundWindow();
    if root == fg {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    // Taskbar, topmost overlays, etc.
    if overlay::is_skip_target(root) {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    // Hit-test the target. Only absorb HTCLIENT — title bars, borders,
    // resize edges, and caption buttons stay one-click responsive.
    // Timeout caps how long a hung window can block the input thread.
    let lo = (pt.x as u32) & 0xFFFF;
    let hi = (pt.y as u32) & 0xFFFF;
    let lparam_xy = ((hi << 16) | lo) as isize;
    let mut hit: usize = 0;
    let _ = SendMessageTimeoutW(
        root,
        WM_NCHITTEST,
        WPARAM(0),
        LPARAM(lparam_xy),
        SMTO_ABORTIFHUNG,
        50,
        Some(&mut hit as *mut _),
    );
    if hit as u32 != HTCLIENT {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    // Absorb the click but still raise the window — the user's intent
    // (bring this app forward) completes; their accidental UI hit doesn't.
    let _ = SetForegroundWindow(root);
    ABSORBED_DOWN.store(msg, Ordering::Relaxed);
    LRESULT(1)
}

pub fn init() {
    std::thread::spawn(|| unsafe {
        let _hook = SetWindowsHookExW(WH_MOUSE_LL, Some(hook_proc), None, 0);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

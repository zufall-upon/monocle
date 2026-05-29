use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Instant;

#[cfg(windows)]
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, SetWindowsHookExW, MSLLHOOKSTRUCT, MSG,
    WH_MOUSE_LL, WM_MOUSEMOVE,
};

static SHAKE_CALLBACK: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

// The hook proc hands shake events to a worker thread through this
// channel instead of running the toggle inline. A WH_MOUSE_LL hook proc
// that runs longer than LowLevelHooksTimeout (~300ms) gets *silently
// removed by Windows for the rest of the session* — and the toggle path
// (update_overlay + Tauri emit) contends for the same locks the 16ms
// foreground tracker holds, so running it on the hook thread is exactly
// that trap. Sending on an unbounded channel is effectively instant, so
// the hook proc always returns well under the timeout.
static SHAKE_TX: Mutex<Option<Sender<()>>> = Mutex::new(None);

struct ShakeState {
    last_x: i32,
    last_dir: i32,
    reversals: u32,
    last_reversal_time: Instant,
}

static SHAKE: Mutex<Option<ShakeState>> = Mutex::new(None);

const SHAKE_REVERSALS_NEEDED: u32 = 4;
const SHAKE_TIME_WINDOW_MS: u128 = 600;
const SHAKE_MIN_DELTA: i32 = 30;

#[cfg(windows)]
unsafe extern "system" fn mouse_hook_proc(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if n_code >= 0 && w_param.0 == WM_MOUSEMOVE as usize {
        let mouse_struct = &*(l_param.0 as *const MSLLHOOKSTRUCT);
        let x = mouse_struct.pt.x;

        let mut state_lock = SHAKE.lock().unwrap();
        let state = state_lock.get_or_insert_with(|| ShakeState {
            last_x: x,
            last_dir: 0,
            reversals: 0,
            last_reversal_time: Instant::now(),
        });

        let dx = x - state.last_x;

        if dx.abs() > SHAKE_MIN_DELTA {
            let dir = dx.signum();
            let now = Instant::now();

            if dir != 0 && dir != state.last_dir && state.last_dir != 0 {
                let elapsed = now.duration_since(state.last_reversal_time).as_millis();
                if elapsed < SHAKE_TIME_WINDOW_MS {
                    state.reversals += 1;
                } else {
                    state.reversals = 1;
                }
                state.last_reversal_time = now;

                if state.reversals >= SHAKE_REVERSALS_NEEDED {
                    state.reversals = 0;
                    drop(state_lock);
                    // Hand off to the worker thread and return immediately;
                    // never run the toggle on the hook thread (see SHAKE_TX).
                    if let Ok(tx) = SHAKE_TX.lock() {
                        if let Some(ref sender) = *tx {
                            let _ = sender.send(());
                        }
                    }
                    return CallNextHookEx(None, n_code, w_param, l_param);
                }
            }

            state.last_dir = dir;
            state.last_x = x;
        }
    }
    CallNextHookEx(None, n_code, w_param, l_param)
}

#[cfg(windows)]
pub fn start_detection<F: Fn() + Send + 'static>(on_shake: F) {
    {
        let mut cb = SHAKE_CALLBACK.lock().unwrap();
        *cb = Some(Box::new(on_shake));
    }

    // Worker thread that actually runs the toggle. The hook proc only
    // signals it through the channel, keeping the hook proc fast enough
    // to never trip LowLevelHooksTimeout.
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    *SHAKE_TX.lock().unwrap() = Some(tx);
    std::thread::spawn(move || {
        while rx.recv().is_ok() {
            let cb = SHAKE_CALLBACK.lock().unwrap();
            if let Some(ref f) = *cb {
                f();
            }
        }
    });

    unsafe {
        let _hook = SetWindowsHookExW(
            WH_MOUSE_LL,
            Some(mouse_hook_proc),
            None,
            0,
        )
        .expect("Failed to set mouse hook");

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
    }
}

#[cfg(not(windows))]
pub fn start_detection<F: Fn() + Send + 'static>(_on_shake: F) {
    eprintln!("Mouse shake detection is only supported on Windows");
}

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
                    if let Ok(cb) = SHAKE_CALLBACK.lock() {
                        if let Some(ref f) = *cb {
                            f();
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

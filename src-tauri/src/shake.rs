use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::{Duration, Instant};

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
static SHAKE_TX: Mutex<Option<Sender<ShakeFire>>> = Mutex::new(None);

/// Diagnostics sent with each fired shake so the log can show whether it was a
/// deliberate gesture or an accidental jitter (a short span = suspicious).
struct ShakeFire {
    reversals: u32,
    span_ms: u128,
    min_delta: i32,
}

struct ShakeState {
    last_x: i32,
    last_dir: i32,
    reversals: u32,
    last_reversal_time: Instant,
    // When the current reversal chain began, so we can report how long the
    // whole shake took (a very short span hints at an accidental trigger).
    chain_start: Instant,
    // After a trigger we ignore movement until this instant, so the tail of the
    // same shake (or an immediate second shake) can't toggle Monocle back.
    cooldown_until: Instant,
}

static SHAKE: Mutex<Option<ShakeState>> = Mutex::new(None);

// How many horizontal direction reversals make a "shake". Kept deliberately
// small — the gesture is defined by how *far* each swing travels, not how many.
const SHAKE_REVERSALS_NEEDED: u32 = 4;
// Each reversal must land within this long of the previous one to keep the
// chain alive; a longer pause resets the count.
const SHAKE_TIME_WINDOW_MS: u128 = 600;
// Quiet period after a trigger. The activation fade is ~0.75s, so this also
// keeps a single vigorous shake from registering as on-then-off.
const SHAKE_COOLDOWN: Duration = Duration::from_millis(900);

// Minimum horizontal travel (px) for a move to count toward a reversal. Driven
// by the sensitivity slider via `set_sensitivity`; bigger = a wider, more
// deliberate swing is required. Defaults to the midpoint (slider 0.5).
static SHAKE_MIN_DELTA: AtomicI32 = AtomicI32::new(57);

// Map the 0..1 sensitivity slider to the required swing distance. Right/high =
// easy (short swing); left/low = deliberate (long swing). Inverse: more
// sensitivity means a smaller distance threshold.
pub fn set_sensitivity(sensitivity: f64) {
    const DELTA_AT_MIN_SENS: f64 = 90.0; // slider 0.0 → must travel far
    const DELTA_AT_MAX_SENS: f64 = 25.0; // slider 1.0 → small swing triggers
    let s = sensitivity.clamp(0.0, 1.0);
    let delta = (DELTA_AT_MIN_SENS - s * (DELTA_AT_MIN_SENS - DELTA_AT_MAX_SENS)).round() as i32;
    SHAKE_MIN_DELTA.store(delta, Ordering::Relaxed);
}

#[cfg(windows)]
unsafe extern "system" fn mouse_hook_proc(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if n_code >= 0 && w_param.0 == WM_MOUSEMOVE as usize {
        let mouse_struct = &*(l_param.0 as *const MSLLHOOKSTRUCT);
        let x = mouse_struct.pt.x;

        let now = Instant::now();
        let mut state_lock = SHAKE.lock().unwrap();
        let state = state_lock.get_or_insert_with(|| ShakeState {
            last_x: x,
            last_dir: 0,
            reversals: 0,
            last_reversal_time: now,
            chain_start: now,
            cooldown_until: now,
        });

        let dx = x - state.last_x;

        if dx.abs() > SHAKE_MIN_DELTA.load(Ordering::Relaxed) {
            let dir = dx.signum();

            // In the post-trigger quiet period: keep tracking position so dx
            // stays sane, but don't accumulate reversals — the tail of the
            // shake that just fired must not toggle Monocle straight back.
            if now < state.cooldown_until {
                state.reversals = 0;
                state.last_dir = dir;
                state.last_x = x;
                return CallNextHookEx(None, n_code, w_param, l_param);
            }

            if dir != 0 && dir != state.last_dir && state.last_dir != 0 {
                let elapsed = now.duration_since(state.last_reversal_time).as_millis();
                if elapsed < SHAKE_TIME_WINDOW_MS {
                    state.reversals += 1;
                } else {
                    // Chain broke (too long a pause) — start a fresh one.
                    state.reversals = 1;
                    state.chain_start = now;
                }
                state.last_reversal_time = now;

                if state.reversals >= SHAKE_REVERSALS_NEEDED {
                    let fire = ShakeFire {
                        reversals: state.reversals,
                        span_ms: now.duration_since(state.chain_start).as_millis(),
                        min_delta: SHAKE_MIN_DELTA.load(Ordering::Relaxed),
                    };
                    state.reversals = 0;
                    state.cooldown_until = now + SHAKE_COOLDOWN;
                    drop(state_lock);
                    // Hand off to the worker thread and return immediately;
                    // never run the toggle on the hook thread (see SHAKE_TX).
                    if let Ok(tx) = SHAKE_TX.lock() {
                        if let Some(ref sender) = *tx {
                            let _ = sender.send(fire);
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
    let (tx, rx) = std::sync::mpsc::channel::<ShakeFire>();
    *SHAKE_TX.lock().unwrap() = Some(tx);
    std::thread::spawn(move || {
        while let Ok(fire) = rx.recv() {
            crate::logging::log(&format!(
                "shake fired: {} reversals in {}ms (min_delta={}px)",
                fire.reversals, fire.span_ms, fire.min_delta
            ));
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

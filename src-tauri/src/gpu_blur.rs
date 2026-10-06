//! Production GPU blur layer.
//!
//! Per monitor it stands up a DirectComposition window
//! that captures that monitor (Windows.Graphics.Capture), runs the
//! captured frame through a Direct2D Saturation -> Gaussian effect chain,
//! and presents the result. It replaces the old acrylic blur layers and
//! folds desaturation into the same pass (so the Magnification-API
//! `magnifier` module is no longer driven).
//!
//! Z-order: each blur window is kept directly above the top acrylic-blur
//! placeholder (`hwnds[BLUR_LAYERS-1]`) and below the tint/grain layers by
//! `reassert_z`, called every tick from the overlay's foreground tracker.
//! So background apps the tracker pushes down sit *behind* the blur, while
//! tint + grain still composite on top and the raised foreground app stays
//! crisp.
//!
//! All windows set WDA_EXCLUDEFROMCAPTURE so the capture pipeline only ever
//! sees real app content — never our own blurred/dimmed/grained output.

// Map the 0..1 settings slider to a Gaussian standard deviation (DIPs).
// ~radius is roughly 3x stddev, so 80 is a very heavy frosted-glass blur.
pub const STDDEV_MAX: f32 = 80.0;

#[cfg(windows)]
pub fn init() {
    imp::init();
}

/// Steady-state blur strength + desaturation. Called from `update_overlay`
/// whenever settings change.
#[cfg(windows)]
pub fn set_params(stddev: f32, desaturate: bool) {
    imp::set_params(stddev, desaturate);
}

/// Deep-focus <-> ambient blend, 0.0 = deep focus (uniform), 1.0 = ambient
/// (progressive). Driven each tick by the mode animator so the switch
/// crossfades between the two treatments instead of cutting hard.
#[cfg(windows)]
pub fn set_mode_mix(mix: f64) {
    imp::set_mode_mix(mix);
}

/// Crossfade progress 0..=1, driven each tick by the fade animator.
#[cfg(windows)]
pub fn set_fade(progress: f64) {
    imp::set_fade(progress);
}

/// Tint uses the existing Direct2D DISSOLVE blend (a per-pixel color wash).
/// `strength` (0..=1) crossfades between
/// the untinted and fully-colorized result. r/g/b are 0..=1. Called from
/// `update_overlay` whenever settings change.
#[cfg(windows)]
pub fn set_tint(r: f32, g: f32, b: f32, strength: f32) {
    imp::set_tint(r, g, b, strength);
}

/// Overlay active/inactive — controls window show/hide.
#[cfg(windows)]
pub fn set_active(active: bool) {
    imp::set_active(active);
}

/// Re-pin every blur window directly above `insert_after` (the top blur
/// placeholder) so it lands below tint/grain. Called every tracker tick.
#[cfg(windows)]
pub fn reassert_z(insert_after: isize) {
    imp::reassert_z(insert_after);
}

/// Tracker geometry notification. This is not an effect input and must not
/// invalidate pixels.
#[cfg(windows)]
pub fn set_focus_rects(rects: &[(i32, i32, i32, i32)]) {
    imp::set_focus_rects(rects);
}

#[cfg(not(windows))]
pub fn init() {}
#[cfg(not(windows))]
pub fn set_params(_stddev: f32, _desaturate: bool) {}
#[cfg(not(windows))]
pub fn set_mode_mix(_mix: f64) {}
#[cfg(not(windows))]
pub fn set_fade(_progress: f64) {}
#[cfg(not(windows))]
pub fn set_tint(_r: f32, _g: f32, _b: f32, _strength: f32) {}
#[cfg(not(windows))]
pub fn set_active(_active: bool) {}
#[cfg(not(windows))]
pub fn reassert_z(_insert_after: isize) {}
#[cfg(not(windows))]
pub fn set_focus_rects(_rects: &[(i32, i32, i32, i32)]) {}

#[cfg(windows)]
mod imp {
    use crate::blur_policy::{processing_plan, required_bands, needs_source_frame, CaptureDemand, EffectKey, EffectValidity, Extent, SessionAction, UpdateState};
    use std::cell::{Cell, RefCell};
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use windows::core::{factory, w, Interface};
    use windows::Graphics::Capture::{
        Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureDirtyRegionMode, GraphicsCaptureItem, GraphicsCaptureSession,
    };
    use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Win32::Foundation::{E_INVALIDARG, HMODULE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Direct2D::Common::{
        D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BLEND_MODE_DISSOLVE,
        D2D1_BORDER_MODE_HARD, D2D1_COLOR_F, D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_GRADIENT_STOP,
        D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
    };
    use windows::Win32::Graphics::Direct2D::{
        D2D1CreateFactory, ID2D1Bitmap1, ID2D1Brush, ID2D1DeviceContext, ID2D1Effect,
        ID2D1Factory1, ID2D1Image, ID2D1LinearGradientBrush, ID2D1Properties, ID2D1RenderTarget,
        CLSID_D2D1Blend, CLSID_D2D1CrossFade, CLSID_D2D1Flood, CLSID_D2D1GaussianBlur,
        CLSID_D2D1Saturation, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
        D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
        D2D1_BLEND_PROP_MODE, D2D1_CROSSFADE_PROP_WEIGHT, D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
        D2D1_EXTEND_MODE_CLAMP, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FLOOD_PROP_COLOR,
        D2D1_GAMMA_2_2, D2D1_GAUSSIANBLUR_PROP_BORDER_MODE,
        D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION, D2D1_INTERPOLATION_MODE_LINEAR,
        D2D1_LAYER_OPTIONS1_NONE, D2D1_LAYER_PARAMETERS1, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES,
        D2D1_PROPERTY_TYPE_ENUM, D2D1_PROPERTY_TYPE_FLOAT, D2D1_PROPERTY_TYPE_VECTOR4,
        D2D1_SATURATION_PROP_SATURATION,
    };
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN};
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11Texture2D,
        D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    };
    use windows::Win32::Graphics::DirectComposition::{
        DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
    };
    use windows::Win32::Graphics::Dxgi::Common::{
        DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
    };
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory2, IDXGIAdapter, IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1,
        DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    };
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::WinRT::Direct3D11::{
        CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
    };
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
    use windows::Win32::UI::WindowsAndMessaging::*;

    // Polling consumes at most one newest frame per tick. This caps application
    // copy/effects/Present work; it does NOT lower WGC's capture resolution or
    // promise to cap the compositor's internal capture work.
    const RENDER_TIMER: usize = 1;
    const FRAME_INTERVAL_MS: u32 = 50; // background blur: at most 20 updates/s
    static REVISION: AtomicU64 = AtomicU64::new(1);

    // Ambient (progressive) blur stacks this many discrete blur bands, each
    // masked by its own vertical gradient, to approximate a continuous
    // top-to-bottom blur ramp.
    const PROGRESSIVE_BANDS: u32 = 3;

    // --- Global state shared with the overlay (one set for all monitors) ---
    static STDDEV_BITS: AtomicU32 = AtomicU32::new(0);
    static FADE_BITS: AtomicU64 = AtomicU64::new(0);
    static DESATURATE: AtomicBool = AtomicBool::new(false);
    // Deep-focus <-> ambient blend (f64 bits): 0.0 = deep focus (uniform
    // blur), 1.0 = ambient (progressive). Animated by the overlay's mode
    // animator so switching modes dissolves smoothly. Mid-values render both
    // treatments and crossfade between them.
    static MODE_MIX_BITS: AtomicU64 = AtomicU64::new(0);
    // Tint color (f32 bits, 0..1 per channel) and blend strength (f32 bits,
    // 0..1). Applied as a DISSOLVE blend over the (de)saturated source —
    // see draw(). Strength 0 = untinted; 1 = fully colorized.
    static TINT_R_BITS: AtomicU32 = AtomicU32::new(0);
    static TINT_G_BITS: AtomicU32 = AtomicU32::new(0);
    static TINT_B_BITS: AtomicU32 = AtomicU32::new(0);
    static TINT_STRENGTH_BITS: AtomicU32 = AtomicU32::new(0);
    static ACTIVE: AtomicBool = AtomicBool::new(false);
    // Every live blur window, so sync()/reassert_z() can address them all.
    static WINDOWS: Mutex<Vec<isize>> = Mutex::new(Vec::new());
    // Exact opaque-client candidates are separate from animated focus bounds.
    static FOCUS_RECTS: Mutex<Vec<(i32, i32, i32, i32)>> = Mutex::new(Vec::new());

    fn stddev() -> f32 {
        f32::from_bits(STDDEV_BITS.load(Ordering::Relaxed))
    }
    fn fade() -> f64 {
        f64::from_bits(FADE_BITS.load(Ordering::Relaxed))
    }
    fn mode_mix() -> f64 {
        f64::from_bits(MODE_MIX_BITS.load(Ordering::Relaxed))
    }
    fn tint_rgb() -> (f32, f32, f32) {
        (
            f32::from_bits(TINT_R_BITS.load(Ordering::Relaxed)),
            f32::from_bits(TINT_G_BITS.load(Ordering::Relaxed)),
            f32::from_bits(TINT_B_BITS.load(Ordering::Relaxed)),
        )
    }
    fn tint_strength() -> f32 {
        f32::from_bits(TINT_STRENGTH_BITS.load(Ordering::Relaxed))
    }

    pub fn set_params(stddev: f32, desaturate: bool) {
        let old_sigma = STDDEV_BITS.swap(stddev.to_bits(), Ordering::Relaxed);
        let old_mono = DESATURATE.swap(desaturate, Ordering::Relaxed);
        if old_sigma != stddev.to_bits() || old_mono != desaturate { sync(); }
    }

    pub fn set_mode_mix(mix: f64) {
        if MODE_MIX_BITS.swap(mix.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed)
            != mix.clamp(0.0, 1.0).to_bits() { sync(); }
    }

    pub fn set_fade(progress: f64) {
        if FADE_BITS.swap(progress.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed)
            != progress.clamp(0.0, 1.0).to_bits() { sync(); }
    }

    pub fn set_tint(r: f32, g: f32, b: f32, strength: f32) {
        TINT_R_BITS.store(r.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        TINT_G_BITS.store(g.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        TINT_B_BITS.store(b.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        TINT_STRENGTH_BITS.store(strength.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        sync();
    }

    pub fn set_active(active: bool) {
        if ACTIVE.swap(active, Ordering::Relaxed) != active { sync(); }
    }

    pub fn set_focus_rects(rects: &[(i32, i32, i32, i32)]) {
        let mut cur = FOCUS_RECTS.lock().unwrap();
        if cur.as_slice() == rects {
            return; // No change — don't force a redraw.
        }
        *cur = rects.to_vec();
        drop(cur);
        // Rectangles currently do not alter pixels. Do not invalidate every GPU.
    }

    fn sync() {
        REVISION.fetch_add(1, Ordering::Release);
    }

    pub fn reassert_z(insert_after: isize) {
        if insert_after == 0 {
            return;
        }
        let windows = WINDOWS.lock().unwrap();
        for &w in windows.iter() {
            unsafe {
                let _ = SetWindowPos(
                    HWND(w as *mut _),
                    Some(HWND(insert_after as *mut _)),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }

    pub fn init() {
        // Reconcile monitor handles AND geometry. Recreate resources on resize,
        // rotation, reconnect and resume instead of copying mismatched textures.
        std::thread::spawn(|| {
            let mut workers: Vec<(isize, windows::Win32::Foundation::RECT,
                Arc<AtomicBool>, std::thread::JoinHandle<()>)> = Vec::new();
            loop {
                let mut monitors: Vec<(isize, windows::Win32::Foundation::RECT)> = Vec::new();
                unsafe {
                    let _ = EnumDisplayMonitors(None, None, Some(monitor_enum),
                        LPARAM(&mut monitors as *mut _ as isize));
                }
                let mut i = 0;
                while i < workers.len() {
                    let (handle, rect, _, thread) = &workers[i];
                    if thread.is_finished() || !monitors.iter().any(|(h, r)| h == handle && r == rect) {
                        let (_, _, stop, thread) = workers.remove(i);
                        stop.store(true, Ordering::Release);
                        let _ = thread.join();
                    } else {
                        i += 1;
                    }
                }
                for (hmon, rect) in monitors {
                    if !workers.iter().any(|(h, _, _, _)| *h == hmon) {
                        let stop = Arc::new(AtomicBool::new(false));
                        let worker_stop = stop.clone();
                        let thread = std::thread::spawn(move || unsafe {
                            run_thread(hmon, rect, worker_stop)
                        });
                        workers.push((hmon, rect, stop, thread));
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        });
    }

    unsafe extern "system" fn monitor_enum(
        hmon: HMONITOR,
        _hdc: HDC,
        _rect: *mut windows::Win32::Foundation::RECT,
        lparam: LPARAM,
    ) -> windows::core::BOOL {
        let out = &mut *(lparam.0 as *mut Vec<(isize, windows::Win32::Foundation::RECT)>);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(hmon, &mut mi).as_bool() {
            out.push((hmon.0 as isize, mi.rcMonitor));
        }
        windows::core::BOOL(1)
    }

    struct Render {
        ctx: ID2D1DeviceContext,
        swapchain: IDXGISwapChain1,
        _dcomp: IDCompositionDevice,
        saturation: ID2D1Effect,
        gaussian: ID2D1Effect,
        // Tint chain: flood (solid tint color) -> blend (DISSOLVE over the
        // saturated source) -> crossfade (mix untinted<->colorized by
        // strength). Output feeds the blur, so the whole background is tinted
        // uniformly across both deep-focus and ambient treatments.
        flood: ID2D1Effect,
        blend: ID2D1Effect,
        crossfade: ID2D1Effect,
        // Per-band vertical opacity gradients (transparent->opaque) used as
        // layer masks for ambient (progressive) blur. Each band feathers the
        // handoff between adjacent blur levels across its own screen slice.
        grad_bands: Vec<ID2D1LinearGradientBrush>,
        framepool: Direct3D11CaptureFramePool,
        frame: RefCell<Option<Direct3D11CaptureFrame>>,
        frame_id: Cell<u64>,
        effects: RefCell<Option<EffectSurfaces>>,
        perf: RefCell<Perf>,
        fallback_texture: RefCell<Option<ID3D11Texture2D>>,
        // This monitor's top-left in virtual-screen coords, to map the
        // focus rects (which arrive in virtual coords) into local pixels.
        origin_x: i32,
        origin_y: i32,
        updates: RefCell<UpdateState>,

        item: GraphicsCaptureItem,
        session: RefCell<Option<GraphicsCaptureSession>>,
        _d3d: ID3D11Device,
        _d3d_device: IDirect3DDevice,
        _target: IDCompositionTarget,
        _visual: IDCompositionVisual,
        width: u32,
        height: u32,
        shown: Cell<bool>,
    }

    impl Drop for Render {
        fn drop(&mut self) {
            if let Some(session) = self.session.get_mut().take() {
                let _ = session.Close();
            }
            if let Some(frame) = self.frame.get_mut().take() { let _ = frame.Close(); }
            let _ = self.framepool.Close();
        }
    }

    thread_local! {
        static STOP: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
        static RENDER: RefCell<Option<Render>> = const { RefCell::new(None) };
    }

    unsafe fn run_thread(hmon: isize, rect: windows::Win32::Foundation::RECT, stop: Arc<AtomicBool>) {
        STOP.with(|s| *s.borrow_mut() = Some(stop));
        let _ = RoInitialize(RO_INIT_MULTITHREADED);

        let hinstance = GetModuleHandleW(None).unwrap();
        let class_name = w!("DeepGpuBlur");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        // Shared class for every monitor; second+ RegisterClassExW is a
        // harmless no-op.
        RegisterClassExW(&wc);

        let x = rect.left;
        let y = rect.top;
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;

        // Click-through, no activation, not in Alt+Tab, true per-pixel alpha
        // (DComp). NOT topmost: the foreground app is raised above us.
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP
                | WS_EX_TOOLWINDOW
                | WS_EX_NOACTIVATE
                | WS_EX_TRANSPARENT,
            class_name,
            w!("DeepGpuBlur"),
            WS_POPUP,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
        .expect("gpu blur window creation failed");

        // Exclude from screen capture: the pipeline must never feed our own
        // output back in.
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);

        WINDOWS.lock().unwrap().push(hwnd.0 as isize);

        if let Err(e) = init_render(hwnd, hmon, x, y, w as u32, h as u32) {
            log(&format!("gpu_blur init_render failed: {e:?}"));
            let _ = DestroyWindow(hwnd);
            WINDOWS.lock().unwrap().retain(|&h| h != hwnd.0 as isize);
            return;
        }

        // The timer also services shutdown while capture is suspended.
        if SetTimer(Some(hwnd), RENDER_TIMER, FRAME_INTERVAL_MS, None) == 0 {
            log("gpu_blur timer creation failed");
            let _ = DestroyWindow(hwnd);
            RENDER.with(|r| *r.borrow_mut() = None);
            WINDOWS.lock().unwrap().retain(|&h| h != hwnd.0 as isize);
            return;
        }
        // Created hidden; the first tick applies the latest shared state.
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        RENDER.with(|r| *r.borrow_mut() = None);
        let val = hwnd.0 as isize;
        WINDOWS.lock().unwrap().retain(|&h| h != val);
    }

    unsafe fn init_render(
        hwnd: HWND,
        hmon: isize,
        origin_x: i32,
        origin_y: i32,
        width: u32,
        height: u32,
    ) -> windows::core::Result<()> {
        // Prefer the adapter physically owning this monitor; never alter OS settings.
        let adapter_factory: IDXGIFactory2 = CreateDXGIFactory2(Default::default())?;
        let mut selected: Option<IDXGIAdapter> = None;
        let mut index = 0;
        while let Ok(adapter) = adapter_factory.EnumAdapters1(index) {
            let mut output_index = 0;
            while let Ok(output) = adapter.EnumOutputs(output_index) {
                if output.GetDesc()?.Monitor.0 as isize == hmon {
                    selected = Some(adapter.cast()?); break;
                }
                output_index += 1;
            }
            if selected.is_some() { break; }
            index += 1;
        }
        // --- D3D11 device (BGRA for D2D interop) ---
        let mut d3d: Option<ID3D11Device> = None;
        D3D11CreateDevice(
            selected.as_ref(),
            if selected.is_some() { D3D_DRIVER_TYPE_UNKNOWN } else { D3D_DRIVER_TYPE_HARDWARE },
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut d3d),
            None,
            None,
        )?;
        let d3d = d3d.unwrap();
        let dxgi_device: IDXGIDevice = d3d.cast()?;
        let actual = dxgi_device.GetAdapter()?.GetDesc()?;
        let name = String::from_utf16_lossy(&actual.Description);
        log(&format!("adapter monitor={hmon:#x} origin={origin_x},{origin_y} size={width}x{height} selected_by_output={} name={} luid={}:{}", selected.is_some(), name.trim_end_matches('\0'), actual.AdapterLuid.HighPart, actual.AdapterLuid.LowPart));

        // --- Direct2D device + context, pinned to 96 dpi (physical px) ---
        let d2d_factory: ID2D1Factory1 =
            D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let d2d_device = d2d_factory.CreateDevice(&dxgi_device)?;
        let ctx = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
        ctx.SetDpi(96.0, 96.0);

        // --- Composition swapchain (premultiplied alpha) ---
        let dxgi_factory: IDXGIFactory2 = CreateDXGIFactory2(Default::default())?;
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 3,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            ..Default::default()
        };
        let swapchain = dxgi_factory.CreateSwapChainForComposition(&d3d, &desc, None)?;

        // --- DirectComposition tree (committed once) ---
        let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)?;
        let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
        let visual = dcomp.CreateVisual()?;
        visual.SetContent(&swapchain)?;
        target.SetRoot(&visual)?;
        dcomp.Commit()?;

        // --- Effect chain: cap_bmp -> saturation -> [tint] -> gaussian ---
        let saturation = ctx.CreateEffect(&CLSID_D2D1Saturation)?;
        let gaussian = ctx.CreateEffect(&CLSID_D2D1GaussianBlur)?;

        // Tint chain. The blend composites the flood (tint color, source) over
        // the saturated capture (destination); CrossFade then mixes the untinted
        // and tinted images by `strength`. Dissolve mode — a per-pixel dither —
        // gives the grainy color wash that reads best over the blur. Set once;
        // the flood color and crossfade weight are updated per-frame in draw().
        let flood = ctx.CreateEffect(&CLSID_D2D1Flood)?;
        let blend = ctx.CreateEffect(&CLSID_D2D1Blend)?;
        let crossfade = ctx.CreateEffect(&CLSID_D2D1CrossFade)?;
        {
            let props: &ID2D1Properties = (&blend).into();
            let mode = D2D1_BLEND_MODE_DISSOLVE.0 as u32;
            let _ = props.SetValue(
                D2D1_BLEND_PROP_MODE.0 as u32,
                D2D1_PROPERTY_TYPE_ENUM,
                &mode.to_le_bytes(),
            );
        }

        // Clamp/extend edge pixels instead of padding with transparent black.
        // The default SOFT border mode samples transparent pixels beyond the
        // capture bounds, so within ~3*stddev of each screen edge the blur
        // averages in that transparency and the edges fade/darken. HARD mode
        // extends the edge pixels, keeping the blur full-strength to the very
        // edge of the screen. Set once — it isn't tied to the stddev.
        {
            let props: &ID2D1Properties = (&gaussian).into();
            let mode = D2D1_BORDER_MODE_HARD.0 as u32;
            let _ = props.SetValue(
                D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                D2D1_PROPERTY_TYPE_ENUM,
                &mode.to_le_bytes(),
            );
        }

        // Per-band vertical opacity gradients for ambient (progressive) blur.
        // We stack PROGRESSIVE_BANDS increasingly-blurred copies of the desktop
        // back-to-front over a sharp base. Band i is masked by its own gradient
        // ramping alpha 0->1 across the screen slice y in [H*(i-1)/N, H*i/N]
        // (CLAMP outside), so it fades in over its slice and stays fully opaque
        // below it. The net result is crisp at the top, peak blur at the bottom,
        // with feathered handoffs between adjacent (near-identical) blur levels.
        // The stop collection is shared (a single 0->1 ramp); only the per-band
        // start/end Y differ. Only the alpha channel matters; RGB is ignored.
        let grad_stops = [
            D2D1_GRADIENT_STOP {
                position: 0.0,
                color: D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 },
            },
            D2D1_GRADIENT_STOP {
                position: 1.0,
                color: D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 },
            },
        ];
        let mut grad_bands: Vec<ID2D1LinearGradientBrush> = Vec::new();
        {
            // The 3-arg overload lives on ID2D1RenderTarget; ID2D1DeviceContext
            // exposes a different 6-arg one. Cast to the base interface to pick
            // the simpler overload.
            let rt: &ID2D1RenderTarget = (&ctx).into();
            let coll = rt.CreateGradientStopCollection(
                &grad_stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )?;
            let n = PROGRESSIVE_BANDS as f32;
            let h = height as f32;
            for i in 1..=PROGRESSIVE_BANDS {
                let y0 = h * (i as f32 - 1.0) / n;
                let y1 = h * i as f32 / n;
                let brush = rt.CreateLinearGradientBrush(
                    &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                        startPoint: windows_numerics::Vector2 { X: 0.0, Y: y0 },
                        endPoint: windows_numerics::Vector2 { X: 0.0, Y: y1 },
                    },
                    None,
                    &coll,
                )?;
                grad_bands.push(brush);
            }
        }

        // --- WGC capture of this monitor ---
        let d3d_device: IDirect3DDevice =
            CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)?.cast()?;
        let hmonitor = HMONITOR(hmon as *mut _);
        let interop: IGraphicsCaptureItemInterop = factory::<GraphicsCaptureItem, _>()?;
        let item: GraphicsCaptureItem = interop.CreateForMonitor(hmonitor)?;

        let size = item.Size()?;
        let framepool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &d3d_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )?;
        RENDER.with(|r| {
            *r.borrow_mut() = Some(Render {
                ctx,
                swapchain,
                _dcomp: dcomp,
                saturation,
                gaussian,
                flood,
                blend,
                crossfade,
                grad_bands,
                framepool,
                frame: RefCell::new(None),
                frame_id: Cell::new(0),
                effects: RefCell::new(None),
                perf: RefCell::new(Perf::new()),
                fallback_texture: RefCell::new(None),
                origin_x,
                origin_y,
                updates: RefCell::new(UpdateState::new(Extent { width, height })),

                item,
                session: RefCell::new(None),
                _d3d: d3d,
                _d3d_device: d3d_device,
                _target: target,
                _visual: visual,
                width,
                height,
                shown: Cell::new(false),
            });
        });
        Ok(())
    }

    fn render(hwnd: HWND) {
        RENDER.with(|r| {
            if let Some(rs) = r.borrow().as_ref() {
                if let Err(e) = unsafe { draw(hwnd, rs) } {
                    log(&format!("gpu_blur draw failed: {e:?}"));
                    // Reconciliation retries with fresh capture/device resources.
                    unsafe { let _ = DestroyWindow(hwnd); }
                }
            }
        });
    }

    struct Perf {
        started: std::time::Instant, last: std::time::Instant,
        acquired: u64, skipped: u64, downsample: u64, sharp: u64,
        blur: u64, present: u64, starts: u64, stops: u64, copies: u64, native_ingest: u64, color: u64,
    }
    impl Perf {
        fn new() -> Self { Self { started: std::time::Instant::now(), last: std::time::Instant::now(), acquired: 0, skipped: 0, downsample: 0, sharp: 0, blur: 0, present: 0, starts: 0, stops: 0, copies: 0, native_ingest: 0, color: 0 } }
    }
    struct EffectSurfaces {
        extent: Extent, input: ID2D1Bitmap1, input_frame: Option<u64>,
        tinted: Option<ID2D1Bitmap1>, tint_key: Option<EffectKey>,
        bands: [Option<ID2D1Bitmap1>; 3], sharp: Option<ID2D1Bitmap1>, raw_sharp: Option<ID2D1Bitmap1>, raw_frame: Option<u64>, validity: EffectValidity,
    }

    unsafe fn draw(hwnd: HWND, rs: &Render) -> windows::core::Result<()> {
        {
            let mut p = rs.perf.borrow_mut();
            if p.last.elapsed().as_secs() >= 10 {
                log(&format!("perf monitor={},{} elapsed_ms={} capture={} skipped={} copy_resource={} downsample={} native_ingest={} color={} sharp={} blur={} present={} starts={} stops={} session={}", rs.origin_x, rs.origin_y, p.started.elapsed().as_millis(), p.acquired, p.skipped, p.copies, p.downsample, p.native_ingest, p.color, p.sharp, p.blur, p.present, p.starts, p.stops, rs.session.borrow().is_some()));
                p.last = std::time::Instant::now();
            }
        }
        let revision = REVISION.load(Ordering::Acquire);
        let wanted = wants_capture() && fade() > 0.0;
        let action = rs.updates.borrow_mut().set_capture(wanted);
        if !wanted {
            sync_visibility(hwnd, rs);
            if let Some(session) = rs.session.borrow_mut().take() {
                session.Close()?;
                rs.perf.borrow_mut().stops += 1;
                if let Some(frame) = rs.frame.borrow_mut().take() { let _ = frame.Close(); }
                *rs.effects.borrow_mut() = None;
                for _ in 0..2 { if let Ok(frame) = rs.framepool.TryGetNextFrame() { let _ = frame.Close(); } else { break; } }
            }
            return Ok(());
        }
        if action == SessionAction::Start {
            let session = rs.framepool.CreateCaptureSession(&rs.item)?;
            let _ = session.SetIsCursorCaptureEnabled(false);
            let _ = session.SetIsBorderRequired(false);
            // Require whole rendered frames when supported; never infer image
            // equality from non-cumulative dirty metadata on a dropping pool.
            let dirty = session.SetDirtyRegionMode(GraphicsCaptureDirtyRegionMode::ReportOnly).is_ok();
            let interval = session.SetMinUpdateInterval(windows::Foundation::TimeSpan { Duration: i64::from(FRAME_INTERVAL_MS) * 10_000 }).is_ok();
            log(&format!("capture monitor={},{} dirty_regions={} min_interval={}", rs.origin_x, rs.origin_y, dirty, interval));
            session.StartCapture()?;
            rs.perf.borrow_mut().starts += 1;
            *rs.session.borrow_mut() = Some(session);
        }
        // Frames live only through this tick's source ingestion. EndDraw before
        // Close; no borrowed WGC surface survives in an effect or cached image.
        let mut newest: Option<Direct3D11CaptureFrame> = None;

        for _ in 0..2 {
            match rs.framepool.TryGetNextFrame() {
                Ok(frame) => {
                    rs.perf.borrow_mut().acquired += 1;
                    if let Some(old) = newest.replace(frame) { let _ = old.Close(); rs.perf.borrow_mut().skipped += 1; }
                }
                Err(e) if e.code() == windows::core::Error::empty().code() => break,
                Err(e) => { if let Some(frame) = newest { let _ = frame.Close(); } return Err(e); }
            }
        }
        if let Some(frame) = newest {
            {
                let result = (|| -> windows::core::Result<()> {
                    let size = frame.ContentSize()?;
                    let access = frame.Surface()?.cast::<IDirect3DDxgiInterfaceAccess>()?;
                    let texture = access.GetInterface::<ID3D11Texture2D>()?;
                    let mut desc = D3D11_TEXTURE2D_DESC::default(); texture.GetDesc(&mut desc);
                    if !rs.updates.borrow_mut().accept_frame(Extent { width: size.Width as u32, height: size.Height as u32 }, Extent { width: desc.Width, height: desc.Height }) {
                        return Err(windows::core::Error::from_hresult(E_INVALIDARG));
                    }
                    Ok(())
                })();
                if let Err(e) = result { let _ = frame.Close(); return Err(e); }
                if let Some(old) = rs.frame.borrow_mut().replace(frame) { let _ = old.Close(); }
                rs.frame_id.set(rs.frame_id.get().wrapping_add(1));
            }
        }
        if !rs.updates.borrow().needs_draw(revision) { return Ok(()); }
        if fade() <= 0.0 {
            if let Some(frame) = rs.frame.borrow_mut().take() { let _ = frame.Close(); }
            return Ok(());
        }
        let sigma = stddev();
        let mix = mode_mix();
        let fade = fade() as f32;
        let (r,g,b) = tint_rgb();
        let key = EffectKey { frame: rs.frame_id.get(), sigma: sigma.to_bits(), desaturate: DESATURATE.load(Ordering::Relaxed), tint: [r.to_bits(),g.to_bits(),b.to_bits(),tint_strength().to_bits()] };
        let plan = processing_plan(Extent { width:rs.width,height:rs.height },sigma);
        let need_fresh_source = {
            let cache = rs.effects.borrow();
            cache.as_ref().map(|c| c.extent != plan.extent ||
                needs_source_frame(mix,sigma,c.input_frame == Some(key.frame),c.raw_frame == Some(key.frame))).unwrap_or(true)
        };
        if need_fresh_source && rs.frame.borrow().is_none() {
            // Deep did not keep a native source. Request a fresh frame for a
            // sharp Ambient base / processing-size change, without holding a
            // WGC pool buffer indefinitely on a static desktop.
            if let Some(session) = rs.session.borrow_mut().take() { session.Close()?; rs.perf.borrow_mut().stops += 1; }
            rs.updates.borrow_mut().set_capture(false);
            return Ok(());
        }
        let prepared = prepare_effects(rs, key, mix);
        if let Some(frame) = rs.frame.borrow_mut().take() { let _ = frame.Close(); }
        prepared?;
        let ctx = &rs.ctx;
        let back: IDXGISurface = rs.swapchain.GetBuffer(0)?;
        let target = ctx.CreateBitmapFromDxgiSurface(&back, Some(&D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0, dpiY: 96.0, bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            colorContext: std::mem::ManuallyDrop::new(None),
        }))?;
        let bounds = rect(rs.width, rs.height);
        let cache = rs.effects.borrow(); let cache = cache.as_ref().unwrap();
        ctx.SetTarget(&target); ctx.BeginDraw(); ctx.Clear(Some(&transparent()));
        if mix < 0.999 {
            let bitmap = if sigma > 0.0 { cache.bands[2].as_ref().unwrap() } else { cache.sharp.as_ref().unwrap() };
            ctx.DrawBitmap(bitmap, Some(&bounds), fade, D2D1_INTERPOLATION_MODE_LINEAR, None, None);
        }
        if mix > 0.001 {
            let layer = make_layer(bounds, fade * mix as f32, None);
            ctx.PushLayer(&layer, None);
            ctx.DrawBitmap(cache.sharp.as_ref().unwrap(), Some(&bounds), 1.0, D2D1_INTERPOLATION_MODE_LINEAR, None, None);
            if sigma > 0.0 {
                for (index, gradient) in rs.grad_bands.iter().enumerate() {
                    let layer = make_layer(bounds, 1.0, Some(gradient.cast()?));
                    ctx.PushLayer(&layer, None);
                    ctx.DrawBitmap(cache.bands[index].as_ref().unwrap(), Some(&bounds), 1.0, D2D1_INTERPOLATION_MODE_LINEAR, None, None);
                    ctx.PopLayer(); drop(std::mem::ManuallyDrop::into_inner(layer.opacityBrush));
                }
            }
            ctx.PopLayer();
        }
        ctx.EndDraw(None, None)?;
        rs.swapchain.Present(0, Default::default()).ok()?;
        rs.perf.borrow_mut().present += 1;
        rs.updates.borrow_mut().presented(revision);
        sync_visibility(hwnd, rs);
        Ok(())
    }

    fn rect(width: u32, height: u32) -> D2D_RECT_F { D2D_RECT_F { left: 0.0, top: 0.0, right: width as f32, bottom: height as f32 } }
    fn make_layer(bounds: D2D_RECT_F, opacity: f32, brush: Option<ID2D1Brush>) -> D2D1_LAYER_PARAMETERS1 {
        D2D1_LAYER_PARAMETERS1 { contentBounds: bounds, geometricMask: std::mem::ManuallyDrop::new(None), maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, maskTransform: identity(), opacity, opacityBrush: std::mem::ManuallyDrop::new(brush), layerOptions: D2D1_LAYER_OPTIONS1_NONE }
    }

    unsafe fn prepare_effects(rs: &Render, key: EffectKey, mix: f64) -> windows::core::Result<()> {
        let sigma = f32::from_bits(key.sigma);
        let plan = processing_plan(Extent { width: rs.width, height: rs.height }, sigma);
        let mut cache = rs.effects.borrow_mut();
        if cache.as_ref().map(|c| c.extent != plan.extent).unwrap_or(true) {
            log(&format!("processing monitor={},{} extent={}x{} divisor={}",rs.origin_x,rs.origin_y,plan.extent.width,plan.extent.height,plan.divisor));
            *cache = Some(EffectSurfaces { extent: plan.extent, input: effect_bitmap(&rs.ctx, plan.extent)?, input_frame: None, tinted: None, tint_key: None, bands: [None,None,None], sharp: None, raw_sharp: None, raw_frame: None, validity: EffectValidity::default() });
        }
        let c = cache.as_mut().unwrap(); c.validity.update(key);
        let needed = required_bands(mix, sigma);
        let needs_input = needed.iter().any(|&v|v) && c.input_frame != Some(key.frame);
        let needs_sharp = (mix > 0.001 || sigma <= 0.0) && c.validity.needs_sharp();
        let needs_raw_sharp = needs_sharp && c.raw_frame != Some(key.frame);
        if needs_input || needs_raw_sharp {
            let frame = rs.frame.borrow(); let frame = frame.as_ref().ok_or_else(|| windows::core::Error::from_hresult(E_INVALIDARG))?;
            let access = frame.Surface()?.cast::<IDirect3DDxgiInterfaceAccess>()?;
            let texture = access.GetInterface::<ID3D11Texture2D>()?;
            let surface: IDXGISurface = texture.cast()?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE }, dpiX:96.0,dpiY:96.0,bitmapOptions:D2D1_BITMAP_OPTIONS_NONE,colorContext:std::mem::ManuallyDrop::new(None),
            };
            let direct = if rs.fallback_texture.borrow().is_some() {
                Err(windows::core::Error::from_hresult(E_INVALIDARG))
            } else { rs.ctx.CreateBitmapFromDxgiSurface(&surface, Some(&props)) };
            let native = match direct {
                Ok(bitmap) => bitmap,
                Err(_) => {
                    // Compatibility fallback only: do not repeatedly recreate
                    // the device if a driver cannot sample the WGC surface.
                    let mut fallback = rs.fallback_texture.borrow_mut();
                    if fallback.is_none() {
                        let mut desc = D3D11_TEXTURE2D_DESC::default(); texture.GetDesc(&mut desc);
                        desc.BindFlags = (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32;
                        desc.Usage = D3D11_USAGE_DEFAULT; desc.CPUAccessFlags = 0; desc.MiscFlags = 0;
                        rs._d3d.CreateTexture2D(&desc,None,Some(&mut *fallback))?;
                        log(&format!("capture compatibility-copy fallback monitor={},{}",rs.origin_x,rs.origin_y));
                    }
                    rs._d3d.GetImmediateContext()?.CopyResource(fallback.as_ref().unwrap(),&texture);
                    rs.perf.borrow_mut().copies += 1;
                    rs.ctx.CreateBitmapFromDxgiSurface(&fallback.as_ref().unwrap().cast::<IDXGISurface>()?,Some(&props))?
                }
            };
            if needs_input {
                rs.ctx.SetTarget(&c.input); rs.ctx.BeginDraw();
                rs.ctx.DrawBitmap(&native, Some(&rect(plan.extent.width,plan.extent.height)), 1.0, D2D1_INTERPOLATION_MODE_LINEAR, Some(&rect(rs.width,rs.height)), None);
                rs.ctx.EndDraw(None,None)?;
                rs.perf.borrow_mut().downsample += 1; c.input_frame = Some(key.frame);
            }
            if needs_raw_sharp {
                if c.raw_sharp.is_none() { c.raw_sharp = Some(effect_bitmap(&rs.ctx, Extent { width:rs.width,height:rs.height })?); }
                rs.ctx.SetTarget(c.raw_sharp.as_ref().unwrap()); rs.ctx.BeginDraw();
                rs.ctx.DrawBitmap(&native,Some(&rect(rs.width,rs.height)),1.0,D2D1_INTERPOLATION_MODE_LINEAR,None,None);
                rs.ctx.EndDraw(None,None)?;
                rs.perf.borrow_mut().native_ingest += 1;
                c.raw_frame = Some(key.frame);
            }
        }
        if needs_sharp {
            if c.sharp.is_none() { c.sharp = Some(effect_bitmap(&rs.ctx, Extent { width:rs.width,height:rs.height })?); }
            paint_effect(rs, c.raw_sharp.as_ref().unwrap(), c.sharp.as_ref().unwrap(), key, None, Extent { width:rs.width,height:rs.height })?;
            rs.perf.borrow_mut().sharp += 1; c.validity.sharp_done();
        }
        if needed.iter().any(|&v|v) && c.tint_key != Some(key) {
            if c.tinted.is_none() { c.tinted = Some(effect_bitmap(&rs.ctx,plan.extent)?); }
            // Materialize finite bounds before Gaussian: Flood's infinite
            // extent must not replace HARD edge clamping with colored padding.
            paint_effect(rs,&c.input,c.tinted.as_ref().unwrap(),key,None,plan.extent)?;
            rs.perf.borrow_mut().color += 1;
            c.tint_key = Some(key);
        }
        for index in 0..3 {
            if needed[index] && c.validity.needs_band(index) {
                if c.bands[index].is_none() { c.bands[index] = Some(effect_bitmap(&rs.ctx,plan.extent)?); }
                let result = (|| -> windows::core::Result<()> {
                    rs.gaussian.SetInput(0,c.tinted.as_ref().unwrap(),true);
                    set_blur(&rs.gaussian,plan.stddev * (index+1) as f32 / 3.0);
                    let output = rs.gaussian.GetOutput()?;
                    rs.ctx.SetTarget(c.bands[index].as_ref().unwrap()); rs.ctx.BeginDraw();
                    rs.ctx.Clear(Some(&transparent()));
                    rs.ctx.DrawImage(&output,None,Some(&rect(plan.extent.width,plan.extent.height)),D2D1_INTERPOLATION_MODE_LINEAR,D2D1_COMPOSITE_MODE_SOURCE_OVER);
                    rs.ctx.EndDraw(None,None)
                })();
                rs.gaussian.SetInput(0,None::<&ID2D1Image>,true);
                result?;
                rs.perf.borrow_mut().blur += 1; c.validity.band_done(index);
            }
        }
        Ok(())
    }

    unsafe fn paint_effect(rs: &Render, input: &ID2D1Bitmap1, target: &ID2D1Bitmap1, key: EffectKey, sigma: Option<f32>, extent: Extent) -> windows::core::Result<()> {
        let result = (|| -> windows::core::Result<()> {
            rs.saturation.SetInput(0,input,true);
            set_saturation(&rs.saturation, if key.desaturate {0.0} else {1.0});
            let sat = rs.saturation.GetOutput()?;
            let strength = f32::from_bits(key.tint[3]);
            let source = if strength > 0.001 {
                set_flood_color(&rs.flood,f32::from_bits(key.tint[0]),f32::from_bits(key.tint[1]),f32::from_bits(key.tint[2]));
                rs.blend.SetInput(0,&sat,true); rs.blend.SetInput(1,&rs.flood.GetOutput()?,true);
                rs.crossfade.SetInput(0,&rs.blend.GetOutput()?,true); rs.crossfade.SetInput(1,&sat,true);
                set_crossfade_weight(&rs.crossfade,strength); rs.crossfade.GetOutput()?
            } else { sat };
            let output = if let Some(sigma) = sigma { rs.gaussian.SetInput(0,&source,true); set_blur(&rs.gaussian,sigma); rs.gaussian.GetOutput()? } else { source };
            rs.ctx.SetTarget(target); rs.ctx.BeginDraw(); rs.ctx.Clear(Some(&transparent()));
            rs.ctx.DrawImage(&output,None,Some(&rect(extent.width,extent.height)),D2D1_INTERPOLATION_MODE_LINEAR,D2D1_COMPOSITE_MODE_SOURCE_OVER);
            rs.ctx.EndDraw(None,None)
        })();
        // Break every effect's reference to the borrowed WGC surface, also on error.
        rs.saturation.SetInput(0,None::<&ID2D1Image>,true);
        rs.gaussian.SetInput(0,None::<&ID2D1Image>,true);
        rs.blend.SetInput(0,None::<&ID2D1Image>,true); rs.blend.SetInput(1,None::<&ID2D1Image>,true);
        rs.crossfade.SetInput(0,None::<&ID2D1Image>,true); rs.crossfade.SetInput(1,None::<&ID2D1Image>,true);
        result
    }

    unsafe fn effect_bitmap(ctx: &ID2D1DeviceContext, extent: Extent) -> windows::core::Result<ID2D1Bitmap1> {
        ctx.CreateBitmap(
            D2D_SIZE_U { width: extent.width, height: extent.height }, None, 0,
            &D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                // TARGET without CANNOT_DRAW: read in a later, separate pass.
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                colorContext: std::mem::ManuallyDrop::new(None),
            },
        )
    }

    fn transparent() -> D2D1_COLOR_F {
        D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }
    }

    unsafe fn set_blur(gaussian: &ID2D1Effect, stddev: f32) {
        let props: &ID2D1Properties = gaussian.into();
        let _ = props.SetValue(
            D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
            D2D1_PROPERTY_TYPE_FLOAT,
            &stddev.to_le_bytes(),
        );
    }

    unsafe fn set_saturation(saturation: &ID2D1Effect, value: f32) {
        let props: &ID2D1Properties = saturation.into();
        let _ = props.SetValue(
            D2D1_SATURATION_PROP_SATURATION.0 as u32,
            D2D1_PROPERTY_TYPE_FLOAT,
            &value.to_le_bytes(),
        );
    }

    unsafe fn set_flood_color(flood: &ID2D1Effect, r: f32, g: f32, b: f32) {
        // D2D1_FLOOD_PROP_COLOR is a VECTOR4 (R, G, B, A) of f32.
        let props: &ID2D1Properties = flood.into();
        let mut buf = [0u8; 16];
        buf[0..4].copy_from_slice(&r.to_le_bytes());
        buf[4..8].copy_from_slice(&g.to_le_bytes());
        buf[8..12].copy_from_slice(&b.to_le_bytes());
        buf[12..16].copy_from_slice(&1.0f32.to_le_bytes());
        let _ = props.SetValue(
            D2D1_FLOOD_PROP_COLOR.0 as u32,
            D2D1_PROPERTY_TYPE_VECTOR4,
            &buf,
        );
    }

    unsafe fn set_crossfade_weight(crossfade: &ID2D1Effect, weight: f32) {
        let props: &ID2D1Properties = crossfade.into();
        let _ = props.SetValue(
            D2D1_CROSSFADE_PROP_WEIGHT.0 as u32,
            D2D1_PROPERTY_TYPE_FLOAT,
            &weight.to_le_bytes(),
        );
    }

    fn identity() -> windows_numerics::Matrix3x2 {
        windows_numerics::Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: 0.0,
            M32: 0.0,
        }
    }

    // Decide whether this window should currently be visible and update its
    // show state to match. Visible whenever the overlay is active or still
    // fading out.
    fn wants_capture() -> bool {
        CaptureDemand {
            active: ACTIVE.load(Ordering::Relaxed),
            fade: fade(),
            stddev: stddev(),
            desaturate: DESATURATE.load(Ordering::Relaxed),
            tint_strength: tint_strength(),
        }.wanted()
    }

    unsafe fn sync_visibility(hwnd: HWND, rs: &Render) {
        // Never show a stale pre-suspension backbuffer before the first fresh
        // frame has been presented. With every effect off the GPU layer is idle.
        let want = wants_capture() && fade() > 0.0 && rs.updates.borrow().has_frame();
        let was = rs.shown.get();
        if want && !was {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            rs.shown.set(true);
        } else if !want && was {
            let _ = ShowWindow(hwnd, SW_HIDE);
            rs.shown.set(false);
        }
    }

    extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe {
            match msg {
                WM_TIMER if wparam.0 == RENDER_TIMER => {
                    let stop = STOP.with(|s| s.borrow().as_ref()
                        .map(|s| s.load(Ordering::Acquire)).unwrap_or(false));
                    if stop {
                        let _ = DestroyWindow(hwnd);
                    } else {
                        render(hwnd);
                    }
                    LRESULT(0)
                }
                WM_DISPLAYCHANGE => {
                    let _ = DestroyWindow(hwnd);
                    LRESULT(0)
                }
                WM_POWERBROADCAST if wparam.0 == 18 || wparam.0 == 7 => {
                    // PBT_APMRESUMEAUTOMATIC / PBT_APMRESUMESUSPEND. Recreate
                    // even if the monitor handle and rectangle did not change.
                    let _ = DestroyWindow(hwnd);
                    LRESULT(1)
                }
                WM_DESTROY => {
                    let _ = KillTimer(Some(hwnd), RENDER_TIMER);
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
    }

    fn log(s: &str) {
        use std::io::Write;
        static LOG_WRITE: Mutex<()> = Mutex::new(());
        let Ok(_guard) = LOG_WRITE.lock() else { return; };
        let path = std::env::temp_dir().join("deep-lite-gpu-blur.log");
        if std::fs::metadata(&path).map(|m| m.len() >= 1_048_576).unwrap_or(false) {
            let old = path.with_extension("log.old");
            let _ = std::fs::remove_file(&old);
            let _ = std::fs::rename(&path,&old);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
            let _ = writeln!(f, "{now} {s}");
        }
    }
}

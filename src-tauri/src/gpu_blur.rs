//! Production GPU blur layer.
//!
//! This is the prototype from `blur_proto.rs` promoted into the real
//! overlay stack. Per monitor it stands up a DirectComposition window
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

/// Tint applied to the blurred background as a Direct2D HSL "Color" blend:
/// the background keeps its luminance (light/dark detail) but takes the
/// hue + saturation of `(r, g, b)`. `strength` (0..=1) crossfades between
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

/// Focused-window rectangles (virtual-screen coords, `(left, top, right,
/// bottom)`) whose footprint must be "neutralized" in the captured frame
/// before blurring. Painting each window's region with its surrounding
/// background pixels removes the crisp window content from the blur input,
/// so it can't smear outward into a halo around the raised foreground app.
/// Called every tracker tick with the current focused group(s).
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
    use std::cell::{Cell, RefCell};
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use windows::core::{factory, w, IInspectable, Interface};
    use windows::Foundation::TypedEventHandler;
    use windows::Graphics::Capture::{
        Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
    };
    use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Direct2D::Common::{
        D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BLEND_MODE_COLOR,
        D2D1_BORDER_MODE_HARD, D2D1_COLOR_F, D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_GRADIENT_STOP,
        D2D1_PIXEL_FORMAT, D2D_RECT_F,
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
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
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
        CreateDXGIFactory2, IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1,
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

    // "A new capture frame is ready" — posted from the capture pool thread.
    const WM_FRAME: u32 = WM_APP + 1;
    // "Overlay state changed (fade / params / active)" — posted from the
    // overlay threads via sync(). WGC only fires on screen *change*, so a
    // static desktop wouldn't otherwise redraw when the user scrubs the
    // slider or the fade advances. This forces a redraw.
    const WM_GPU_SYNC: u32 = WM_APP + 2;

    // Ambient (progressive) blur stacks this many discrete blur bands, each
    // masked by its own vertical gradient, to approximate a continuous
    // top-to-bottom blur ramp.
    const PROGRESSIVE_BANDS: u32 = 8;

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
    // 0..1). Applied as an HSL "Color" blend over the (de)saturated source —
    // see draw(). Strength 0 = untinted; 1 = fully colorized.
    static TINT_R_BITS: AtomicU32 = AtomicU32::new(0);
    static TINT_G_BITS: AtomicU32 = AtomicU32::new(0);
    static TINT_B_BITS: AtomicU32 = AtomicU32::new(0);
    static TINT_STRENGTH_BITS: AtomicU32 = AtomicU32::new(0);
    static ACTIVE: AtomicBool = AtomicBool::new(false);
    // Every live blur window, so sync()/reassert_z() can address them all.
    static WINDOWS: Mutex<Vec<isize>> = Mutex::new(Vec::new());
    // Focused-window rects (virtual-screen coords) to neutralize before the
    // blur. Updated each tracker tick; draw() reads them per monitor.
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
        STDDEV_BITS.store(stddev.to_bits(), Ordering::Relaxed);
        DESATURATE.store(desaturate, Ordering::Relaxed);
        sync();
    }

    pub fn set_mode_mix(mix: f64) {
        MODE_MIX_BITS.store(mix.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        sync();
    }

    pub fn set_fade(progress: f64) {
        FADE_BITS.store(progress.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        sync();
    }

    pub fn set_tint(r: f32, g: f32, b: f32, strength: f32) {
        TINT_R_BITS.store(r.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        TINT_G_BITS.store(g.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        TINT_B_BITS.store(b.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        TINT_STRENGTH_BITS.store(strength.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        sync();
    }

    pub fn set_active(active: bool) {
        ACTIVE.store(active, Ordering::Relaxed);
        sync();
    }

    pub fn set_focus_rects(rects: &[(i32, i32, i32, i32)]) {
        let mut cur = FOCUS_RECTS.lock().unwrap();
        if cur.as_slice() == rects {
            return; // No change — don't force a redraw.
        }
        *cur = rects.to_vec();
        drop(cur);
        sync();
    }

    fn sync() {
        let windows = WINDOWS.lock().unwrap();
        for &w in windows.iter() {
            unsafe {
                let _ = PostMessageW(Some(HWND(w as *mut _)), WM_GPU_SYNC, WPARAM(0), LPARAM(0));
            }
        }
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
        // Enumerate monitors on the calling thread, then spawn one capture
        // thread per monitor. HMONITOR is just a handle; pass it as isize.
        let mut monitors: Vec<(isize, windows::Win32::Foundation::RECT)> = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(
                None,
                None,
                Some(monitor_enum),
                LPARAM(&mut monitors as *mut _ as isize),
            );
        }
        for (hmon, rect) in monitors {
            std::thread::spawn(move || unsafe { run_thread(hmon, rect) });
        }
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
        // Tint chain: flood (solid tint color) -> blend (HSL Color over the
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
        d3d_ctx: ID3D11DeviceContext,
        cap_tex: ID3D11Texture2D,
        cap_bmp: ID2D1Bitmap1,
        // This monitor's top-left in virtual-screen coords, to map the
        // focus rects (which arrive in virtual coords) into local pixels.
        origin_x: i32,
        origin_y: i32,
        has_frame: Cell<bool>,
        frame_pending: Arc<AtomicBool>,
        frame_token: i64,
        _session: GraphicsCaptureSession,
        _d3d: ID3D11Device,
        _d3d_device: IDirect3DDevice,
        _target: IDCompositionTarget,
        _visual: IDCompositionVisual,
        width: u32,
        height: u32,
        shown: Cell<bool>,
    }

    thread_local! {
        static RENDER: RefCell<Option<Render>> = const { RefCell::new(None) };
    }

    unsafe fn run_thread(hmon: isize, rect: windows::Win32::Foundation::RECT) {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);

        let hinstance = GetModuleHandleW(None).unwrap();
        let class_name = w!("MonocleGpuBlur");
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
            w!("MonocleGpuBlur"),
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
        }

        // Created hidden; sync() shows it once the overlay activates.
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
        // --- D3D11 device (BGRA for D2D interop) ---
        let mut d3d: Option<ID3D11Device> = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
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

        // Tint chain. The Color blend takes hue+saturation from its source
        // (the flood color) and luminance from its destination (the saturated
        // capture), i.e. a true colorize. CrossFade then mixes the untinted
        // and colorized images by `strength`. Blend mode is set once; the
        // flood color and crossfade weight are updated per-frame in draw().
        let flood = ctx.CreateEffect(&CLSID_D2D1Flood)?;
        let blend = ctx.CreateEffect(&CLSID_D2D1Blend)?;
        let crossfade = ctx.CreateEffect(&CLSID_D2D1CrossFade)?;
        {
            let props: &ID2D1Properties = (&blend).into();
            let mode = D2D1_BLEND_MODE_COLOR.0 as u32;
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
        // edge of the screen. The residual edge bias (clamped pixels still read
        // as an under-blurred strip) is then hidden by `overscan` at draw time.
        // Set once — it isn't tied to the stddev.
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
        let session = framepool.CreateCaptureSession(&item)?;
        let _ = session.SetIsCursorCaptureEnabled(false);
        let _ = session.SetIsBorderRequired(false);

        let frame_pending = Arc::new(AtomicBool::new(false));
        let fp = frame_pending.clone();
        let hwnd_isize = hwnd.0 as isize;
        let handler = TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(
            move |_pool, _args| {
                if !fp.swap(true, Ordering::AcqRel) {
                    unsafe {
                        let _ = PostMessageW(
                            Some(HWND(hwnd_isize as *mut _)),
                            WM_FRAME,
                            WPARAM(0),
                            LPARAM(0),
                        );
                    }
                }
                Ok(())
            },
        );
        let frame_token = framepool.FrameArrived(&handler)?;
        session.StartCapture()?;

        // Owned texture we CopyResource each frame into (frame-pool textures
        // recycle after Close()), wrapped as a D2D bitmap for the chain.
        let cap_desc = D3D11_TEXTURE2D_DESC {
            Width: size.Width as u32,
            Height: size.Height as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut cap_tex: Option<ID3D11Texture2D> = None;
        d3d.CreateTexture2D(&cap_desc, None, Some(&mut cap_tex))?;
        let cap_tex = cap_tex.unwrap();
        let cap_surf: IDXGISurface = cap_tex.cast()?;
        let cap_props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let cap_bmp: ID2D1Bitmap1 = ctx.CreateBitmapFromDxgiSurface(&cap_surf, Some(&cap_props))?;

        let d3d_ctx = d3d.GetImmediateContext()?;

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
                d3d_ctx,
                cap_tex,
                cap_bmp,
                origin_x,
                origin_y,
                has_frame: Cell::new(false),
                frame_pending,
                frame_token,
                _session: session,
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

    fn render() {
        RENDER.with(|r| {
            if let Some(rs) = r.borrow().as_ref() {
                if let Err(e) = unsafe { draw(rs) } {
                    log(&format!("gpu_blur draw failed: {e:?}"));
                }
            }
        });
    }

    unsafe fn draw(rs: &Render) -> windows::core::Result<()> {
        let ctx = &rs.ctx;

        // Fully idle: overlay off and faded out. WGC keeps firing FrameArrived
        // on every screen change regardless of our state, so without this we'd
        // run a full-screen GPU copy + present per monitor on each change even
        // while Monocle is disabled. Drain & discard the frames cheaply and
        // bail before any real work.
        if !ACTIVE.load(Ordering::Relaxed) && fade() <= 0.0 {
            while let Ok(frame) = rs.framepool.TryGetNextFrame() {
                let _ = frame.Close();
            }
            return Ok(());
        }

        // Drain the capture pool to the freshest frame.
        let mut got = false;
        while let Ok(frame) = rs.framepool.TryGetNextFrame() {
            if let Ok(surface) = frame.Surface() {
                if let Ok(access) = surface.cast::<IDirect3DDxgiInterfaceAccess>() {
                    if let Ok(src_tex) = access.GetInterface::<ID3D11Texture2D>() {
                        rs.d3d_ctx.CopyResource(&rs.cap_tex, &src_tex);
                        got = true;
                    }
                }
            }
            let _ = frame.Close();
        }
        if got {
            rs.has_frame.set(true);
        }

        let fade = fade();

        let back: IDXGISurface = rs.swapchain.GetBuffer(0)?;
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let target_bitmap: ID2D1Bitmap1 = ctx.CreateBitmapFromDxgiSurface(&back, Some(&props))?;
        ctx.SetTarget(&target_bitmap);

        ctx.BeginDraw();
        ctx.Clear(Some(&D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));

        let w = rs.width as f32;
        let h = rs.height as f32;

        // Only paint blur while there's something to show. When fully faded
        // out we present a transparent frame (clears the last image) and let
        // the sync handler hide the window.
        if rs.has_frame.get() && fade > 0.0 {
            let stddev = stddev();
            let desaturate = DESATURATE.load(Ordering::Relaxed);

            // Source region = the whole monitor, drawn 1:1 at the origin.
            let source_rect = D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: w,
                bottom: h,
            };

            // cap_bmp -> saturation (shared sharp, optionally desaturated source)
            rs.saturation.SetInput(0, &rs.cap_bmp, true);
            set_saturation(&rs.saturation, if desaturate { 0.0 } else { 1.0 });
            let sat_out = rs.saturation.GetOutput()?;

            // Tint: colorize the (de)saturated source before it fans out to the
            // blur. Applied here (not on the raw capture) so the desaturate
            // toggle can't strip the tint color back out. At strength ~0 we
            // skip the blend chain and feed the source straight through.
            let (tr, tg, tb) = tint_rgb();
            let strength = tint_strength();
            let src: ID2D1Image = if strength > 0.001 {
                set_flood_color(&rs.flood, tr, tg, tb);
                let flood_out = rs.flood.GetOutput()?;
                // Color blend: dest (input 0) supplies luminance, source
                // (input 1) supplies hue+saturation.
                rs.blend.SetInput(0, &sat_out, true);
                rs.blend.SetInput(1, &flood_out, true);
                let blend_out = rs.blend.GetOutput()?;
                // CrossFade output = input0*weight + input1*(1-weight), so put
                // the colorized image on input 0 and the untinted one on input
                // 1. Weight then reads directly as the colorize amount: 0 =
                // untinted, `strength` = how strongly to tint.
                rs.crossfade.SetInput(0, &blend_out, true);
                rs.crossfade.SetInput(1, &sat_out, true);
                set_crossfade_weight(&rs.crossfade, strength);
                rs.crossfade.GetOutput()?
            } else {
                sat_out.clone()
            };

            // Deep-focus <-> ambient blend. 0 = uniform (deep focus), 1 =
            // progressive (ambient). Mid-values render both and crossfade so
            // the mode switch dissolves smoothly instead of cutting hard.
            let mix = mode_mix();
            let show_deep = mix < 0.999;
            let show_ambient = mix > 0.001;

            // --- Deep focus: uniform full-screen blur (drawn first, beneath
            // the ambient stack). Opacity = fade. During a transition the
            // ambient stack dissolves in on top; at the bottom of the screen
            // both treatments are full blur, so the still-opaque deep layer
            // never reads as a double image.
            if show_deep {
                rs.gaussian.SetInput(0, &src, true);
                set_blur(&rs.gaussian, stddev);
                let output = rs.gaussian.GetOutput()?;

                let faded = fade < 0.999;
                if faded {
                    let layer = D2D1_LAYER_PARAMETERS1 {
                        contentBounds: source_rect,
                        geometricMask: std::mem::ManuallyDrop::new(None),
                        maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                        maskTransform: identity(),
                        opacity: fade as f32,
                        opacityBrush: std::mem::ManuallyDrop::new(None),
                        layerOptions: D2D1_LAYER_OPTIONS1_NONE,
                    };
                    ctx.PushLayer(&layer, None);
                }
                // Overscan so the bounded-blur edge band lands off-screen; the
                // layer / target clips the overshoot back to the monitor.
                ctx.SetTransform(&overscan(w, h, stddev));
                ctx.DrawImage(
                    &output,
                    None,
                    Some(&source_rect),
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    D2D1_COMPOSITE_MODE_SOURCE_OVER,
                );
                ctx.SetTransform(&identity());
                if faded {
                    ctx.PopLayer();
                }
            }

            // --- Ambient (progressive) blur: stack PROGRESSIVE_BANDS
            // increasingly-blurred copies back-to-front over the sharp
            // (desaturated) base. Each band is masked by its own vertical
            // gradient so it fades in across its screen slice — crisp at the
            // top, peak blur (stddev) at the bottom. The whole stack is wrapped
            // in one outer layer at opacity = fade * mix, which folds in both
            // the activation crossfade and the mode-switch dissolve.
            if show_ambient {
                let amb_opacity = (fade * mix) as f32;
                let layered = amb_opacity < 0.999;
                if layered {
                    let layer = D2D1_LAYER_PARAMETERS1 {
                        contentBounds: source_rect,
                        geometricMask: std::mem::ManuallyDrop::new(None),
                        maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                        maskTransform: identity(),
                        opacity: amb_opacity,
                        opacityBrush: std::mem::ManuallyDrop::new(None),
                        layerOptions: D2D1_LAYER_OPTIONS1_NONE,
                    };
                    ctx.PushLayer(&layer, None);
                }

                // Sharp base (blur 0).
                ctx.DrawImage(
                    &src,
                    None,
                    Some(&source_rect),
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    D2D1_COMPOSITE_MODE_SOURCE_OVER,
                );

                // Stack increasingly-blurred bands, each masked by its gradient.
                rs.gaussian.SetInput(0, &src, true);
                let n = rs.grad_bands.len() as f32;
                for (idx, band) in rs.grad_bands.iter().enumerate() {
                    let level = stddev * (idx as f32 + 1.0) / n;
                    set_blur(&rs.gaussian, level);
                    let output = rs.gaussian.GetOutput()?;
                    // AddRef the brush into the layer's ManuallyDrop slot, then
                    // reclaim + drop it after PopLayer to balance the reference.
                    let brush: ID2D1Brush = band.cast()?;
                    let layer = D2D1_LAYER_PARAMETERS1 {
                        contentBounds: source_rect,
                        geometricMask: std::mem::ManuallyDrop::new(None),
                        maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                        maskTransform: identity(),
                        opacity: 1.0,
                        opacityBrush: std::mem::ManuallyDrop::new(Some(brush)),
                        layerOptions: D2D1_LAYER_OPTIONS1_NONE,
                    };
                    ctx.PushLayer(&layer, None);
                    // Overscan the blurred band (gradient mask was captured in
                    // screen space at PushLayer, so it stays put); reset before
                    // PopLayer so the mask composites unscaled.
                    ctx.SetTransform(&overscan(w, h, level));
                    ctx.DrawImage(
                        &output,
                        None,
                        Some(&source_rect),
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        D2D1_COMPOSITE_MODE_SOURCE_OVER,
                    );
                    ctx.SetTransform(&identity());
                    ctx.PopLayer();
                    let _ = std::mem::ManuallyDrop::into_inner(layer.opacityBrush);
                }

                if layered {
                    ctx.PopLayer();
                }
            }
        }

        ctx.EndDraw(None, None)?;
        rs.swapchain.Present(0, Default::default()).ok()?;
        Ok(())
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

    // Overscan transform for the blurred image. A Gaussian blur bounded to the
    // screen can't sample real content past the edges, so even with HARD border
    // clamping the outermost ~k*stddev band is biased toward the replicated edge
    // pixels and reads as a sharp, under-blurred strip — the classic bounded-blur
    // edge artifact. This zooms the blurred image about the screen center just
    // enough to push that band off-screen, so the visible edges sample
    // fully-blurred interior pixels instead. The overshoot is clipped back to the
    // screen by the surrounding layer / render target. Blurred content has no
    // sharp features, so the slight scale is imperceptible — only ever apply this
    // to blurred draws, never to the sharp ambient base. Identity when stddev≈0.
    fn overscan(w: f32, h: f32, stddev: f32) -> windows_numerics::Matrix3x2 {
        let inset = 2.5 * stddev; // edge pixels to hide per side
        if inset <= 0.5 || inset * 2.0 >= w.min(h) {
            return identity();
        }
        let sx = w / (w - 2.0 * inset);
        let sy = h / (h - 2.0 * inset);
        windows_numerics::Matrix3x2 {
            M11: sx,
            M12: 0.0,
            M21: 0.0,
            M22: sy,
            M31: -sx * inset,
            M32: -sy * inset,
        }
    }

    // Decide whether this window should currently be visible and update its
    // show state to match. Visible whenever the overlay is active or still
    // fading out.
    unsafe fn sync_visibility(hwnd: HWND, rs: &Render) {
        let want = ACTIVE.load(Ordering::Relaxed) || fade() > 0.0;
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
                WM_FRAME => {
                    RENDER.with(|r| {
                        if let Some(rs) = r.borrow().as_ref() {
                            rs.frame_pending.store(false, Ordering::Release);
                        }
                    });
                    render();
                    LRESULT(0)
                }
                WM_GPU_SYNC => {
                    RENDER.with(|r| {
                        if let Some(rs) = r.borrow().as_ref() {
                            sync_visibility(hwnd, rs);
                        }
                    });
                    render();
                    LRESULT(0)
                }
                WM_DESTROY => {
                    RENDER.with(|r| {
                        if let Some(rs) = r.borrow().as_ref() {
                            let _ = rs.framepool.RemoveFrameArrived(rs.frame_token);
                        }
                    });
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
    }

    fn log(s: &str) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(std::env::temp_dir().join("monocle-gpu-blur.log"))
        {
            let _ = writeln!(f, "{s}");
        }
    }
}

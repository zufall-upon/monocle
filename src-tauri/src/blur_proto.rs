//! Standalone blur prototype.
//!
//! Purpose: answer one question definitively — can we render a *tunable*
//! blur of the content sitting behind an overlay, lighter than the fixed
//! acrylic floor? This is the macOS NSVisualEffectView approach: capture
//! the screen behind us, run a GPU Gaussian over it, and present the
//! result clipped to a panel.
//!
//! It is deliberately decoupled from the real overlay system — no
//! triggers, no per-monitor logic, no window management. Just a floating
//! rounded-rect panel, opened from the tray (and on launch), so we can
//! eyeball the blur and tune it.
//!
//! Two prototype windows, same plumbing, selected by `Mode`:
//!   - Uniform: one blur strength across the whole panel; mouse wheel
//!     scrubs it. Proves the capture + GPU-Gaussian pipeline.
//!   - Progressive: blur strength varies vertically — heavy at the
//!     bottom, fading to crisp at the top — by compositing a sharp base
//!     layer with a Gaussian-blurred layer masked by a vertical linear
//!     gradient opacity brush. The "gradient blur" game-changer.

#[cfg(windows)]
pub fn open() {
    imp::open(imp::Mode::Uniform);
}

#[cfg(windows)]
pub fn open_progressive() {
    imp::open(imp::Mode::Progressive);
}

#[cfg(not(windows))]
pub fn open() {}

#[cfg(not(windows))]
pub fn open_progressive() {}

#[cfg(windows)]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::Arc;

    use windows::core::{factory, w, IInspectable, Interface};
    use windows::Foundation::TypedEventHandler;
    use windows::Graphics::Capture::{
        Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
    };
    use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Direct2D::Common::{
        D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F,
        D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_GRADIENT_STOP, D2D1_PIXEL_FORMAT, D2D_RECT_F,
    };
    use windows::Win32::Graphics::Direct2D::{
        D2D1CreateFactory, ID2D1Bitmap1, ID2D1Brush, ID2D1DeviceContext, ID2D1Effect,
        ID2D1Factory1, ID2D1Geometry, ID2D1LinearGradientBrush, ID2D1Properties, ID2D1RenderTarget,
        ID2D1RoundedRectangleGeometry, CLSID_D2D1GaussianBlur, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
        D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET,
        D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_EXTEND_MODE_CLAMP,
        D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_GAMMA_2_2,
        D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION, D2D1_INTERPOLATION_MODE_LINEAR,
        D2D1_LAYER_OPTIONS1_NONE, D2D1_LAYER_PARAMETERS1, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES,
        D2D1_PROPERTY_TYPE_FLOAT, D2D1_ROUNDED_RECT,
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
        GetMonitorInfoW, MonitorFromWindow, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::WinRT::Direct3D11::{
        CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
    };
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows_numerics::{Matrix3x2, Vector2};

    // Panel geometry (logical pixels at the window's DPI; we pin D2D to
    // 96 dpi so 1 unit == 1 physical pixel and skip scaling math).
    const PANEL_W: i32 = 760;
    const PANEL_H: i32 = 480;
    const CORNER_RADIUS: f32 = 28.0;

    // Blur tuning range (Gaussian standard deviation, in DIPs). The
    // mouse wheel scrubs between these; ~radius is roughly 3x stddev. In
    // progressive mode this is the *peak* (bottom) strength.
    const STDDEV_MIN: f32 = 0.0;
    const STDDEV_MAX: f32 = 40.0;
    const STDDEV_DEFAULT: f32 = 8.0;
    const STDDEV_STEP: f32 = 2.0;

    // Progressive mode: how many discrete blur levels we stack to fake a
    // continuous variable-radius blur. Each band crossfades into the next
    // over a feathered horizontal slice, so neighbouring (similar) blur
    // levels blend — no crisp-vs-heavy ghosting. More bands = smoother but
    // more GPU (one Gaussian evaluation per band per frame).
    const PROGRESSIVE_BANDS: u32 = 8;

    // Custom message: "a new capture frame is ready, please redraw".
    // Posted from the capture pool thread, handled on the window thread so
    // all D2D/D3D work stays single-threaded.
    const WM_FRAME: u32 = WM_APP + 1;

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum Mode {
        Uniform,
        Progressive,
    }

    // Each mode gets at most one live window; these track the live HWND so
    // a second open() brings the existing panel forward instead of
    // spawning a duplicate. Per-mode (not one shared static) so the two
    // prototypes coexist.
    static UNIFORM_HWND: AtomicIsize = AtomicIsize::new(0);
    static PROGRESSIVE_HWND: AtomicIsize = AtomicIsize::new(0);

    fn hwnd_slot(mode: Mode) -> &'static AtomicIsize {
        match mode {
            Mode::Uniform => &UNIFORM_HWND,
            Mode::Progressive => &PROGRESSIVE_HWND,
        }
    }

    struct Render {
        mode: Mode,
        ctx: ID2D1DeviceContext,
        swapchain: IDXGISwapChain1,
        // Held only to keep the composition device alive; we commit the
        // tree once at init and never touch it per-frame.
        _dcomp: IDCompositionDevice,
        // The Gaussian effect and the rounded-rect clip geometry, built
        // once and reused every frame.
        gaussian: ID2D1Effect,
        clip: ID2D1RoundedRectangleGeometry,
        // Per-band vertical opacity-gradient brushes (Progressive mode).
        // Band i ramps alpha 0 -> 1 across the slice y in
        // [H*(i-1)/N, H*i/N] (CLAMP outside), so stacking the bands back to
        // front reveals progressively heavier blur toward the bottom, each
        // handoff feathered between adjacent levels. Empty in Uniform mode.
        grad_bands: Vec<ID2D1LinearGradientBrush>,
        // Capture pipeline: free-threaded frame pool we drain each tick.
        framepool: Direct3D11CaptureFramePool,
        hwnd: HWND,
        // Owned copy of the latest capture. WGC frame-pool textures get
        // recycled after Close(), and WGC only emits frames when the
        // screen *changes*. So each new frame is CopyResource'd into our
        // own texture (cap_tex), which a single D2D bitmap (cap_bmp)
        // wraps for the blur input. That lets us re-blur a static desktop
        // every tick while scrubbing — and avoids the recycled-surface
        // race entirely.
        d3d_ctx: ID3D11DeviceContext,
        cap_tex: ID3D11Texture2D,
        cap_bmp: ID2D1Bitmap1,
        has_frame: Cell<bool>,
        // Current blur strength (peak, in Progressive). Lives here, not in
        // a global, so the two windows don't share one knob; only ever
        // touched on this window's thread.
        stddev: Cell<f32>,
        // Coalesces frame notifications for *this* window: while a redraw
        // is queued we skip posting another, so a fast source can't flood
        // the message queue. The FrameArrived closure (on the pool thread)
        // holds a clone; the window thread clears it on WM_FRAME.
        frame_pending: Arc<AtomicBool>,
        // FrameArrived registration; removed on teardown.
        frame_token: i64,
        // Held to keep the composition tree / capture session alive.
        _session: GraphicsCaptureSession,
        _d3d: ID3D11Device,
        _d3d_device: IDirect3DDevice,
        _target: IDCompositionTarget,
        _visual: IDCompositionVisual,
        width: u32,
        height: u32,
    }

    thread_local! {
        static RENDER: RefCell<Option<Render>> = const { RefCell::new(None) };
    }

    pub fn open(mode: Mode) {
        // Already open? Bring it forward instead of spawning a duplicate.
        let existing = hwnd_slot(mode).load(Ordering::Relaxed);
        if existing != 0 {
            unsafe {
                let _ = ShowWindow(HWND(existing as *mut _), SW_SHOW);
                let _ = SetForegroundWindow(HWND(existing as *mut _));
            }
            return;
        }
        std::thread::spawn(move || unsafe { run_thread(mode) });
    }

    unsafe fn run_thread(mode: Mode) {
        // Windows.Graphics.Capture is WinRT; initialise this thread's
        // apartment before touching any activation factories. Multi-
        // threaded: the free-threaded frame pool delivers/serves frames
        // off our message pump, so we don't need an STA + DispatcherQueue.
        let _ = RoInitialize(RO_INIT_MULTITHREADED);

        let hinstance = GetModuleHandleW(None).unwrap();
        let class_name = w!("MonocleBlurProto");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: class_name,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        // Shared class for both windows; the second RegisterClassExW is a
        // harmless no-op (ERROR_CLASS_ALREADY_EXISTS).
        RegisterClassExW(&wc);

        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        let (x, y) = panel_origin(mode, sw, sh);

        let title = match mode {
            Mode::Uniform => w!("Monocle Blur Test — Uniform"),
            Mode::Progressive => w!("Monocle Blur Test — Progressive"),
        };

        // WS_EX_NOREDIRECTIONBITMAP is mandatory for a DirectComposition
        // window: it removes the GDI redirection surface so the swapchain
        // composites with true per-pixel alpha against the desktop.
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class_name,
            title,
            WS_POPUP,
            x,
            y,
            PANEL_W,
            PANEL_H,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
        .expect("blur proto window creation failed");

        hwnd_slot(mode).store(hwnd.0 as isize, Ordering::Relaxed);

        // Exclude ourselves from screen capture so the capture pipeline
        // won't feed our own pixels back into the blur.
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);

        if let Err(e) = init_render(hwnd, mode) {
            log(&format!("init_render failed: {e:?}"));
        }

        let _ = ShowWindow(hwnd, SW_SHOW);
        render();

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        RENDER.with(|r| *r.borrow_mut() = None);
        hwnd_slot(mode).store(0, Ordering::Relaxed);
    }

    // Where each prototype window sits. Side by side, centred as a pair,
    // when the screen is wide enough; otherwise nudged apart so neither
    // fully hides the other.
    fn panel_origin(mode: Mode, sw: i32, sh: i32) -> (i32, i32) {
        let gap = 24;
        let pair_w = PANEL_W * 2 + gap;
        if pair_w <= sw {
            let left = (sw - pair_w) / 2;
            let top = (sh - PANEL_H) / 2;
            match mode {
                Mode::Uniform => (left, top),
                Mode::Progressive => (left + PANEL_W + gap, top),
            }
        } else {
            let cx = (sw - PANEL_W) / 2;
            let cy = (sh - PANEL_H) / 2;
            match mode {
                Mode::Uniform => (cx - 36, cy - 36),
                Mode::Progressive => (cx + 36, cy + 36),
            }
        }
    }

    unsafe fn init_render(hwnd: HWND, mode: Mode) -> windows::core::Result<()> {
        // --- D3D11 device (BGRA support required for D2D interop) ---
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

        // --- Direct2D device + context ---
        let d2d_factory: ID2D1Factory1 =
            D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let d2d_device = d2d_factory.CreateDevice(&dxgi_device)?;
        let ctx = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
        // Pin to 96 dpi: draw in physical-pixel units, no scaling math.
        ctx.SetDpi(96.0, 96.0);

        // --- Composition swapchain (premultiplied alpha) ---
        // Triple-buffered: a little extra headroom so a late frame doesn't
        // stall presentation (helps smooth out the occasional hitch).
        let dxgi_factory: IDXGIFactory2 = CreateDXGIFactory2(Default::default())?;
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: PANEL_W as u32,
            Height: PANEL_H as u32,
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

        // --- DirectComposition tree: target <- visual <- swapchain ---
        // Committed once here. After this the visual just references the
        // swapchain, so presenting new frames updates the screen without
        // another Commit — dropping the per-frame Commit removes a DWM
        // round-trip and a source of micro-stutter.
        let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)?;
        let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
        let visual = dcomp.CreateVisual()?;
        visual.SetContent(&swapchain)?;
        target.SetRoot(&visual)?;
        dcomp.Commit()?;

        // --- Gaussian blur effect (tunable standard deviation) ---
        let gaussian = ctx.CreateEffect(&CLSID_D2D1GaussianBlur)?;

        // --- Rounded-rect clip geometry (matches the panel outline) ---
        let factory_geo: ID2D1Factory1 = ctx.GetFactory()?.cast()?;
        let clip_rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: 1.0,
                top: 1.0,
                right: PANEL_W as f32 - 1.0,
                bottom: PANEL_H as f32 - 1.0,
            },
            radiusX: CORNER_RADIUS,
            radiusY: CORNER_RADIUS,
        };
        let clip = factory_geo.CreateRoundedRectangleGeometry(&clip_rect)?;

        // --- Per-band vertical opacity gradients (Progressive mode) ---
        // One brush per band: alpha ramps 0 -> 1 across that band's slice,
        // CLAMP outside (so above the slice it's fully transparent, below
        // it's fully opaque). Stacking bands back-to-front then yields a
        // smooth blur ramp (see Render::grad_bands). Use the
        // ID2D1RenderTarget 3-arg overload (the ID2D1DeviceContext one
        // shadows it with a wider, color-space-aware signature we don't
        // need).
        let mut grad_bands: Vec<ID2D1LinearGradientBrush> = Vec::new();
        if mode == Mode::Progressive {
            let rt: &ID2D1RenderTarget = (&ctx).into();
            let stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: D2D1_COLOR_F {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.0,
                    },
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: D2D1_COLOR_F {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 1.0,
                    },
                },
            ];
            // Shared stop collection (same 0->1 ramp); only the brush
            // start/end points differ per band.
            let coll =
                rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
            let n = PROGRESSIVE_BANDS as f32;
            let h = PANEL_H as f32;
            for i in 1..=PROGRESSIVE_BANDS {
                let y0 = h * (i as f32 - 1.0) / n;
                let y1 = h * i as f32 / n;
                let gprops = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: Vector2 { X: 0.0, Y: y0 },
                    endPoint: Vector2 { X: 0.0, Y: y1 },
                };
                grad_bands.push(rt.CreateLinearGradientBrush(&gprops, None, &coll)?);
            }
        }

        // --- Windows.Graphics.Capture of the monitor behind the panel ---
        // Wrap our D3D device as a WinRT IDirect3DDevice so the frame pool
        // hands back textures on the same device our D2D context uses.
        let d3d_device: IDirect3DDevice =
            CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)?.cast()?;

        let hmonitor: HMONITOR = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
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
        // Suppress the yellow capture border where the OS build allows it.
        let _ = session.SetIsBorderRequired(false);

        // Drive redraws off real frame arrival rather than a timer: the
        // pool fires this (on its own thread) exactly when new content is
        // available, so our presents stay phase-locked to the source and
        // moving content (video, dragging) doesn't beat against a sample
        // clock. We only post a wake-up to the window thread — all the
        // D2D/D3D work happens there, single-threaded. The pending flag
        // and target HWND are captured per-window so two panels never
        // cross-signal.
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

        // Owned texture we copy each frame into (see Render::cap_tex note),
        // plus the single D2D bitmap that wraps it as the blur input.
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
                mode,
                ctx,
                swapchain,
                _dcomp: dcomp,
                gaussian,
                clip,
                grad_bands,
                framepool,
                hwnd,
                d3d_ctx,
                cap_tex,
                cap_bmp,
                has_frame: Cell::new(false),
                stddev: Cell::new(STDDEV_DEFAULT),
                frame_pending,
                frame_token,
                _session: session,
                _d3d: d3d,
                _d3d_device: d3d_device,
                _target: target,
                _visual: visual,
                width: PANEL_W as u32,
                height: PANEL_H as u32,
            });
        });
        Ok(())
    }

    fn render() {
        RENDER.with(|r| {
            if let Some(rs) = r.borrow().as_ref() {
                if let Err(e) = unsafe { draw(rs) } {
                    log(&format!("draw failed: {e:?}"));
                }
            }
        });
    }

    unsafe fn draw(rs: &Render) -> windows::core::Result<()> {
        let ctx = &rs.ctx;

        // --- Pull the freshest capture frame, if one is queued ---
        // Drain the pool: under a fast source several frames can stack up
        // between wake-ups; we only care about the newest, so copy the last
        // one in and close the rest. Keeps latency low and avoids a backlog.
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

        // Bind the swapchain back buffer as the D2D render target.
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
        // Fully transparent background -> only the panel shows.
        ctx.Clear(Some(&D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));

        let stddev = rs.stddev.get();
        let w = rs.width as f32;
        let h = rs.height as f32;

        if rs.has_frame.get() {
            // Where the panel sits relative to its monitor, so we sample
            // the matching region of the captured frame.
            let (sx, sy) = panel_offset_on_monitor(rs.hwnd);
            let source_rect = D2D_RECT_F {
                left: sx,
                top: sy,
                right: sx + w,
                bottom: sy + h,
            };

            match rs.mode {
                Mode::Uniform => {
                    rs.gaussian.SetInput(0, &rs.cap_bmp, true);
                    set_blur(&rs.gaussian, stddev);
                    push_clip_layer(ctx, &rs.clip, w, h, None)?;
                    let output = rs.gaussian.GetOutput()?;
                    ctx.DrawImage(
                        &output,
                        None,
                        Some(&source_rect),
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        D2D1_COMPOSITE_MODE_SOURCE_OVER,
                    );
                    ctx.PopLayer();
                }
                Mode::Progressive => {
                    // 1) Sharp base (blur 0), clipped to the rounded rect.
                    //    The top of the panel stays crisp; bands stack over
                    //    it from here.
                    push_clip_layer(ctx, &rs.clip, w, h, None)?;
                    ctx.DrawImage(
                        &rs.cap_bmp,
                        None,
                        Some(&source_rect),
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        D2D1_COMPOSITE_MODE_SOURCE_OVER,
                    );
                    ctx.PopLayer();

                    // 2) Stack increasingly-blurred bands back to front.
                    //    Band i uses blur stddev = peak * i/N and is masked
                    //    by its own gradient (transparent above its slice,
                    //    opaque below). Because consecutive bands differ by
                    //    only one step of blur, every feathered handoff
                    //    blends two near-identical images — a smooth ramp,
                    //    not a haze of heavy blur over crisp base.
                    rs.gaussian.SetInput(0, &rs.cap_bmp, true);
                    let n = rs.grad_bands.len() as f32;
                    for (idx, band) in rs.grad_bands.iter().enumerate() {
                        let level = stddev * (idx as f32 + 1.0) / n;
                        set_blur(&rs.gaussian, level);
                        let output = rs.gaussian.GetOutput()?;
                        let brush: ID2D1Brush = band.cast()?;
                        push_clip_layer(ctx, &rs.clip, w, h, Some(&brush))?;
                        ctx.DrawImage(
                            &output,
                            None,
                            Some(&source_rect),
                            D2D1_INTERPOLATION_MODE_LINEAR,
                            D2D1_COMPOSITE_MODE_SOURCE_OVER,
                        );
                        ctx.PopLayer();
                    }
                }
            }
        }

        let rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: 1.0,
                top: 1.0,
                right: w - 1.0,
                bottom: h - 1.0,
            },
            radiusX: CORNER_RADIUS,
            radiusY: CORNER_RADIUS,
        };

        // Crisp 1px border so the panel edge reads clearly over the blur.
        let ba = 0.85_f32;
        let border = ctx.CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: ba,
                g: ba,
                b: ba,
                a: ba,
            },
            None,
        )?;
        ctx.DrawRoundedRectangle(&rect, &border, 1.5, None);

        // Strength bar at the bottom showing where the blur sits in
        // [MIN, MAX]. Scroll the wheel to scrub it live (peak, in
        // Progressive mode).
        draw_strength_bar(ctx, w, h, stddev)?;

        ctx.EndDraw(None, None)?;

        // Present with sync interval 0: let DWM's own composition clock
        // pace us instead of blocking on a vblank wait inside Present. The
        // composited swapchain still updates on the next DWM frame, but we
        // don't stall the render thread — which is what was producing the
        // occasional hitch. No dcomp.Commit() here: the visual already
        // references the swapchain (committed once at init).
        rs.swapchain.Present(0, Default::default()).ok()?;
        Ok(())
    }

    // Set the Gaussian standard deviation. SetValue is declared on the
    // parent ID2D1Properties; reach it via the generated &child -> &parent
    // reference cast. (The caller binds input 0 to cap_bmp beforehand.)
    unsafe fn set_blur(gaussian: &ID2D1Effect, stddev: f32) {
        let props: &ID2D1Properties = gaussian.into();
        let _ = props.SetValue(
            D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
            D2D1_PROPERTY_TYPE_FLOAT,
            &stddev.to_le_bytes(),
        );
    }

    // Push a rounded-rect clip layer, optionally modulated by an opacity
    // brush (used for the progressive gradient fade). The geometry/brush
    // casts add a ref that PushLayer copies; we drop ours right after.
    unsafe fn push_clip_layer(
        ctx: &ID2D1DeviceContext,
        clip: &ID2D1RoundedRectangleGeometry,
        w: f32,
        h: f32,
        opacity_brush: Option<&ID2D1Brush>,
    ) -> windows::core::Result<()> {
        let mut layer = D2D1_LAYER_PARAMETERS1 {
            contentBounds: D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: w,
                bottom: h,
            },
            geometricMask: std::mem::ManuallyDrop::new(Some(clip.cast::<ID2D1Geometry>()?)),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: identity(),
            opacity: 1.0,
            opacityBrush: std::mem::ManuallyDrop::new(opacity_brush.cloned()),
            layerOptions: D2D1_LAYER_OPTIONS1_NONE,
        };
        ctx.PushLayer(&layer, None);
        std::mem::ManuallyDrop::drop(&mut layer.geometricMask);
        std::mem::ManuallyDrop::drop(&mut layer.opacityBrush);
        Ok(())
    }

    // Identity transform (windows-numerics doesn't expose a const ctor).
    fn identity() -> Matrix3x2 {
        Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: 0.0,
            M32: 0.0,
        }
    }

    // Top-left of the panel expressed in its monitor's local pixel space,
    // so we can sample the matching slice of the full-monitor capture.
    unsafe fn panel_offset_on_monitor(hwnd: HWND) -> (f32, f32) {
        let mut wr = RECT::default();
        if GetWindowRect(hwnd, &mut wr).is_err() {
            return (0.0, 0.0);
        }
        let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(hmon, &mut mi).as_bool() {
            (
                (wr.left - mi.rcMonitor.left) as f32,
                (wr.top - mi.rcMonitor.top) as f32,
            )
        } else {
            (0.0, 0.0)
        }
    }

    unsafe fn draw_strength_bar(
        ctx: &ID2D1DeviceContext,
        w: f32,
        h: f32,
        stddev: f32,
    ) -> windows::core::Result<()> {
        let margin = 24.0_f32;
        let bar_h = 4.0_f32;
        let track = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: margin,
                top: h - margin - bar_h,
                right: w - margin,
                bottom: h - margin,
            },
            radiusX: bar_h * 0.5,
            radiusY: bar_h * 0.5,
        };
        let track_brush = ctx.CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.35,
            },
            None,
        )?;
        ctx.FillRoundedRectangle(&track, &track_brush);

        let t = ((stddev - STDDEV_MIN) / (STDDEV_MAX - STDDEV_MIN)).clamp(0.0, 1.0);
        let fill_right = margin + (w - 2.0 * margin) * t;
        if fill_right > margin {
            let fill = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: margin,
                    top: h - margin - bar_h,
                    right: fill_right,
                    bottom: h - margin,
                },
                radiusX: bar_h * 0.5,
                radiusY: bar_h * 0.5,
            };
            let fill_brush = ctx.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.95,
                    g: 0.95,
                    b: 0.98,
                    a: 0.95,
                },
                None,
            )?;
            ctx.FillRoundedRectangle(&fill, &fill_brush);
        }
        Ok(())
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
                    // A capture frame landed: clear this window's pending
                    // flag so the next arrival re-posts, then re-blur.
                    RENDER.with(|r| {
                        if let Some(rs) = r.borrow().as_ref() {
                            rs.frame_pending.store(false, Ordering::Release);
                        }
                    });
                    render();
                    LRESULT(0)
                }
                WM_MOUSEWHEEL => {
                    // High word of wParam is the signed wheel delta
                    // (multiples of WHEEL_DELTA == 120). Scrub the blur.
                    let delta = ((wparam.0 >> 16) & 0xffff) as i16;
                    let steps = delta as f32 / 120.0;
                    RENDER.with(|r| {
                        if let Some(rs) = r.borrow().as_ref() {
                            let v = (rs.stddev.get() + steps * STDDEV_STEP)
                                .clamp(STDDEV_MIN, STDDEV_MAX);
                            rs.stddev.set(v);
                        }
                    });
                    render();
                    LRESULT(0)
                }
                WM_KEYDOWN => {
                    // Escape closes the test panel.
                    if wparam.0 == VK_ESCAPE.0 as usize {
                        let _ = DestroyWindow(hwnd);
                    }
                    LRESULT(0)
                }
                WM_DESTROY => {
                    // Stop frame callbacks before we tear down, so no
                    // WM_FRAME can be posted to a dead window.
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
            .open(std::env::temp_dir().join("monocle-blur-proto.log"))
        {
            let _ = writeln!(f, "{s}");
        }
    }
}

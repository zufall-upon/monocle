//! Shared production styles: layered visual overlays pass input through across threads.
use windows::Win32::UI::WindowsAndMessaging::*;
pub fn visual_style() -> WINDOW_STYLE { WS_POPUP }
pub fn visual_ex_style(layered: bool) -> WINDOW_EX_STYLE {
    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED
        | if layered { WINDOW_EX_STYLE(0) } else { WS_EX_NOREDIRECTIONBITMAP }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::{core::{w,Interface}, Win32::{Foundation::*,Graphics::{DirectComposition::*,Direct3D::D3D_DRIVER_TYPE_WARP,Direct3D11::*,Dxgi::{*,Common::*}},System::{LibraryLoader::GetModuleHandleW,WinRT::{RoInitialize,RO_INIT_MULTITHREADED}}}};
    struct Owned(HWND);
    impl Drop for Owned { fn drop(&mut self) { unsafe { let _=DestroyWindow(self.0); } } }
    unsafe extern "system" fn proc(hwnd:HWND,msg:u32,w:WPARAM,l:LPARAM)->LRESULT {
        DefWindowProcW(hwnd,msg,w,l)
    }
    #[test]
    fn win32_hit_target_survives_visual_layers_show_hide_and_recreation() {
        // Synthetic Windows runner fixture, not Explorer or real mouse input.
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED).unwrap();
            let instance=GetModuleHandleW(None).unwrap();
            let cls=w!("DeepLiteInputFixture");
            let wc=WNDCLASSEXW {cbSize:std::mem::size_of::<WNDCLASSEXW>() as u32,lpfnWndProc:Some(proc),hInstance:instance.into(),lpszClassName:cls,..Default::default()};
            assert_ne!(RegisterClassExW(&wc),0);
            let base=Owned(CreateWindowExW(WS_EX_TOOLWINDOW,cls,w!("fixture"),WS_POPUP,100,100,100,100,None,None,Some(instance.into()),None).unwrap());
            let _=ShowWindow(base.0,SW_SHOWNOACTIVATE);
            SetWindowPos(base.0,Some(HWND_TOP),0,0,0,0,SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE).unwrap();
            let point=POINT{x:120,y:120};
            assert_eq!(WindowFromPoint(point),base.0,"runner must expose the synthetic base");
            for _ in 0..2 { for layered in [true,false] {
                let visual=Owned(CreateWindowExW(visual_ex_style(layered),cls,w!("visual fixture"),visual_style(),100,100,100,100,None,None,Some(instance.into()),None).unwrap());
                SetLayeredWindowAttributes(visual.0,COLORREF(0),255,LWA_ALPHA).unwrap();
                let _composition = if !layered {
                    // A real opaque presented surface, not an empty visual tree.
                    let mut d3d=None;
                    let mut context=None;
                    D3D11CreateDevice(None,D3D_DRIVER_TYPE_WARP,HMODULE::default(),D3D11_CREATE_DEVICE_BGRA_SUPPORT,None,D3D11_SDK_VERSION,Some(&mut d3d),None,Some(&mut context)).unwrap();
                    let d3d=d3d.unwrap();
                    let context=context.unwrap();
                    let dxgi:IDXGIDevice=d3d.cast().unwrap();
                    let factory:IDXGIFactory2=CreateDXGIFactory2(Default::default()).unwrap();
                    let desc=DXGI_SWAP_CHAIN_DESC1 { Width:100,Height:100,Format:DXGI_FORMAT_B8G8R8A8_UNORM,SampleDesc:DXGI_SAMPLE_DESC{Count:1,Quality:0},BufferUsage:DXGI_USAGE_RENDER_TARGET_OUTPUT,BufferCount:2,SwapEffect:DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,AlphaMode:DXGI_ALPHA_MODE_PREMULTIPLIED,..Default::default() };
                    let swapchain=factory.CreateSwapChainForComposition(&d3d,&desc,None).unwrap();
                    let buffer:ID3D11Texture2D=swapchain.GetBuffer(0).unwrap();
                    let mut rtv=None;
                    d3d.CreateRenderTargetView(&buffer,None,Some(&mut rtv)).unwrap();
                    context.ClearRenderTargetView(&rtv.unwrap(),&[1.0,0.0,0.0,1.0]);
                    swapchain.Present(0,DXGI_PRESENT(0)).ok().unwrap();
                    let device:IDCompositionDevice=DCompositionCreateDevice(&dxgi).unwrap();
                    let target=device.CreateTargetForHwnd(visual.0,true).unwrap();
                    let root=device.CreateVisual().unwrap();
                    root.SetContent(&swapchain).unwrap();
                    target.SetRoot(&root).unwrap();
                    device.Commit().unwrap();
                    device.WaitForCommitCompletion().unwrap();
                    Some((device,target,root,swapchain,d3d))
                } else { None };
                let _=ShowWindow(visual.0,SW_SHOWNOACTIVATE);
                SetWindowPos(visual.0,Some(HWND_TOP),0,0,0,0,SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE).unwrap();
                assert_ne!(GetWindowLongW(visual.0,GWL_EXSTYLE) as u32 & WS_EX_LAYERED.0,0);
                assert_eq!(WindowFromPoint(point),base.0,"visual must never become the input target: layered GDI={layered} visual={:?}",visual.0);
                let _=ShowWindow(visual.0,SW_HIDE);
                assert_eq!(WindowFromPoint(point),base.0);
            }}
            // A nonactivating ignored-app lift alone must not lift an unrelated
            // background app. Foreground-selection policy is tested separately.
            let divider=Owned(CreateWindowExW(visual_ex_style(true),cls,w!("divider"),visual_style(),300,100,50,50,None,None,Some(instance.into()),None).unwrap());
            SetLayeredWindowAttributes(divider.0,COLORREF(0),180,LWA_ALPHA).unwrap();
            let ignored=Owned(CreateWindowExW(WS_EX_TOOLWINDOW,cls,w!("ignored fixture"),WS_POPUP,400,100,50,50,None,None,Some(instance.into()),None).unwrap());
            let _=ShowWindow(divider.0,SW_SHOWNOACTIVATE);
            let _=ShowWindow(ignored.0,SW_SHOWNOACTIVATE);
            let flags=SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE;
            SetWindowPos(divider.0,Some(HWND_TOP),0,0,0,0,flags).unwrap();
            SetWindowPos(base.0,Some(divider.0),0,0,0,0,flags).unwrap();
            let before=GetForegroundWindow();
            SetWindowPos(ignored.0,Some(HWND_TOP),0,0,0,0,flags).unwrap();
            assert_eq!(GetForegroundWindow(),before,"ignored raise must not activate");
            let mut cursor=divider.0;
            let mut found=false;
            for _ in 0..4096 {
                cursor=GetWindow(cursor,GW_HWNDNEXT).unwrap_or_default();
                if cursor==base.0 { found=true;break; }
                if cursor.0.is_null() { break; }
            }
            assert!(found,"ignored raise must leave unrelated base below divider");
        }
    }
}

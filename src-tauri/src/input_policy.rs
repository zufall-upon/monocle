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
    use windows::{core::w, Win32::{Foundation::*,Graphics::{DirectComposition::*,Dxgi::IDXGIDevice},System::{LibraryLoader::GetModuleHandleW,WinRT::{RoInitialize,RO_INIT_MULTITHREADED}}}};
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
                    let device:IDCompositionDevice=DCompositionCreateDevice(None::<&IDXGIDevice>).unwrap();
                    let target=device.CreateTargetForHwnd(visual.0,true).unwrap();
                    let root=device.CreateVisual().unwrap();
                    target.SetRoot(&root).unwrap();
                    device.Commit().unwrap();
                    Some((device,target,root))
                } else { None };
                let _=ShowWindow(visual.0,SW_SHOWNOACTIVATE);
                SetWindowPos(visual.0,Some(HWND_TOP),0,0,0,0,SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE).unwrap();
                assert_ne!(GetWindowLongW(visual.0,GWL_EXSTYLE) as u32 & WS_EX_LAYERED.0,0);
                assert_eq!(WindowFromPoint(point),base.0,"visual must never become the input target: layered GDI={layered} visual={:?}",visual.0);
                let _=ShowWindow(visual.0,SW_HIDE);
                assert_eq!(WindowFromPoint(point),base.0);
            }}
        }
    }
}

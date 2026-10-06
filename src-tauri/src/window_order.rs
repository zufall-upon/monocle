//! Native placement shared by production and the ownership regression fixture.
use windows::{core::Result, Win32::{Foundation::HWND, UI::WindowsAndMessaging::*}};

pub unsafe fn lower_window(target: HWND, after: HWND) -> Result<()> {
    // A hidden shared owner must not drag a different sharp sibling down.
    SetWindowPos(target, Some(after), 0, 0, 0, 0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER)
}

pub unsafe fn above(target: HWND, root: HWND) -> Option<bool> {
    if !IsWindow(Some(target)).as_bool() || !IsWindow(Some(root)).as_bool() { return None; }
    let mut cursor=target;
    for _ in 0..4096 {
        cursor=GetWindow(cursor,GW_HWNDNEXT).unwrap_or_default();
        if cursor.0.is_null() { return Some(false); }
        if cursor==root { return Some(true); }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::{core::w,Win32::{Foundation::*,System::{LibraryLoader::GetModuleHandleW,Threading::GetCurrentThreadId}}};
    unsafe extern "system" fn proc(h:HWND,m:u32,w:WPARAM,l:LPARAM)->LRESULT {
        if m==WM_WINDOWPOSCHANGING && GetWindowLongPtrW(h,GWLP_USERDATA)==1 {
            (*(l.0 as *mut WINDOWPOS)).flags |= SWP_NOZORDER;
        }
        DefWindowProcW(h,m,w,l)
    }
    unsafe fn create()->HWND {
        CreateWindowExW(WS_EX_TOOLWINDOW,w!("DeepLitePlacementResultFixture"),w!("fixture"),WS_POPUP,
            600,500,80,80,None,None,Some(GetModuleHandleW(None).unwrap().into()),None).unwrap()
    }
    #[test]
    fn successful_api_can_leave_cross_thread_window_above_divider() {
        unsafe {
            assert_ne!(RegisterClassExW(&WNDCLASSEXW{cbSize:std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc:Some(proc),hInstance:GetModuleHandleW(None).unwrap().into(),
                lpszClassName:w!("DeepLitePlacementResultFixture"),..Default::default()}),0);
            let root=create(); let _=ShowWindow(root,SW_SHOWNOACTIVATE);
            let (tx,rx)=std::sync::mpsc::channel();
            let worker=std::thread::spawn(move|| {
                let h=create(); SetWindowLongPtrW(h,GWLP_USERDATA,1);
                let _=ShowWindow(h,SW_SHOWNOACTIVATE);
                tx.send((h.0 as isize,GetCurrentThreadId())).unwrap();
                let mut msg=MSG::default();
                while GetMessageW(&mut msg,None,0,0).0>0 {let _=TranslateMessage(&msg);DispatchMessageW(&msg);}
                let _=DestroyWindow(h);
            });
            let (id,tid)=rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            let target=HWND(id as *mut _);
            assert_ne!(tid,GetCurrentThreadId());
            // Move divider instead; the target intentionally refuses z-order moves.
            SetWindowPos(root,Some(target),0,0,0,0,SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE).unwrap();
            let initial=above(target,root);
            let result=crate::placement_trace::lower(target,root,"fixture-reject-zorder");
            let unchanged=above(target,root);
            crate::placement_trace::observe_next();
            let report=crate::placement_trace::report();
            // The same target accepts the identical call when its veto is removed.
            SetWindowLongPtrW(target,GWLP_USERDATA,0);
            let accepted=lower_window(target,root);
            let moved=above(target,root);
            let _=PostThreadMessageW(tid,WM_QUIT,WPARAM(0),LPARAM(0));
            worker.join().unwrap(); let _=DestroyWindow(root);
            assert_eq!(initial,Some(true)); assert!(result.is_ok()); assert_eq!(unchanged,Some(true));
            assert!(report.contains("fixture-reject-zorder") && report.contains("result=ok"));
            assert!(report.contains("next_after_ms=") && report.contains("target_security=[integrity_rid="));
            assert!(accepted.is_ok()); assert_eq!(moved,Some(false));
            println!("cross-thread veto: SetWindowPos=Ok, position unchanged; accepting same call moves below divider");
        }
    }
}

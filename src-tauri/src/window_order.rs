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

/// If a background HWND does not accept demotion, move our divider instead.
/// Only safe when every protected window already precedes that background.
/// Never change another application's styles, owner, activation or topmost band.
pub unsafe fn raise_divider_above(target:HWND,root:HWND,visuals:&[isize],protected:&[isize])->Result<bool> {
    if above(target,root)!=Some(true) || !IsWindowVisible(target).as_bool()
        || (GetWindowLongW(target,GWL_EXSTYLE) as u32 | GetWindowLongW(root,GWL_EXSTYLE) as u32)&WS_EX_TOPMOST.0!=0
        || protected.iter().any(|&id|above(HWND(id as *mut _),target)!=Some(true)) {return Ok(false);}
    let predecessor=|h:HWND| {
        let mut prev=GetWindow(h,GW_HWNDPREV).unwrap_or_default();
        for _ in 0..4096 {
            if !visuals.contains(&(prev.0 as isize)) {break;}
            prev=GetWindow(prev,GW_HWNDPREV).unwrap_or_default();
        }
        if prev.0.is_null() || GetWindowLongW(prev,GWL_EXSTYLE) as u32&WS_EX_TOPMOST.0!=0 {HWND_TOP} else {prev}
    };
    let restore=predecessor(root);
    let insert=predecessor(target);
    lower_window(root,insert)?;
    let safe=above(root,target)==Some(true) && protected.iter().all(|&id|
        visuals.iter().all(|&visual|above(HWND(id as *mut _),HWND(visual as *mut _))==Some(true)));
    if !safe {lower_window(root,restore)?;}
    Ok(safe)
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
    unsafe fn visual(owner:Option<HWND>)->HWND {
        let h=CreateWindowExW(crate::input_policy::visual_ex_style(true),w!("DeepLitePlacementResultFixture"),w!("visual"),
            crate::input_policy::visual_style(),600,500,80,80,owner,None,Some(GetModuleHandleW(None).unwrap().into()),None).unwrap();
        SetLayeredWindowAttributes(h,COLORREF(0),255,LWA_ALPHA).unwrap();
        let _=ShowWindow(h,SW_SHOWNOACTIVATE); h
    }
    #[test]
    fn successful_api_can_leave_cross_thread_window_above_divider() {
        unsafe {
            assert_ne!(RegisterClassExW(&WNDCLASSEXW{cbSize:std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc:Some(proc),hInstance:GetModuleHandleW(None).unwrap().into(),
                lpszClassName:w!("DeepLitePlacementResultFixture"),..Default::default()}),0);
            let root=visual(None); let tint=visual(Some(root)); let grain=visual(Some(tint));
            let gpu=[visual(None),visual(None),visual(None)];
            let active=create(); let _=ShowWindow(active,SW_SHOWNOACTIVATE);
            SetWindowPos(active,Some(HWND_TOP),700,500,80,80,SWP_NOACTIVATE).unwrap();
            let (tx,rx)=std::sync::mpsc::channel();
            let worker=std::thread::spawn(move|| {
                let h=create(); SetWindowLongPtrW(h,GWLP_USERDATA,1);
                let rebound=create(); let _=ShowWindow(rebound,SW_SHOWNOACTIVATE);
                let _=ShowWindow(h,SW_SHOWNOACTIVATE);
                tx.send((h.0 as isize,rebound.0 as isize,GetCurrentThreadId())).unwrap();
                let mut msg=MSG::default();
                while GetMessageW(&mut msg,None,0,0).0>0 {let _=TranslateMessage(&msg);DispatchMessageW(&msg);}
                let _=DestroyWindow(rebound); let _=DestroyWindow(h);
            });
            let (id,rebound_id,tid)=rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            let target=HWND(id as *mut _);
            assert_ne!(tid,GetCurrentThreadId());
            // Move divider instead; the target intentionally refuses z-order moves.
            SetWindowPos(root,Some(target),0,0,0,0,SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE).unwrap();
            let initial=above(target,root);
            let result=crate::placement_trace::lower(target,root,"fixture-reject-zorder");
            let unchanged=above(target,root);
            crate::placement_trace::observe_next();
            let report=crate::placement_trace::report();
            // Repair a native cross-thread veto by moving only our overlay.
            let flags=SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE;
            SetWindowPos(active,Some(HWND_TOP),0,0,0,0,flags).unwrap();
            let fg=GetForegroundWindow();
            let visuals=[root.0 as isize,tint.0 as isize,grain.0 as isize];
            // A second protected monitor anchor below the refusing target must
            // prevent relocation; never blur that retained sharp window.
            lower_window(HWND(rebound_id as *mut _),root).unwrap();
            let declined=raise_divider_above(target,root,&visuals,&[active.0 as isize,rebound_id]).unwrap();
            let still_above=above(target,root);
            let repaired=raise_divider_above(target,root,&visuals,&[active.0 as isize]).unwrap();
            for g in gpu {lower_window(g,root).unwrap();}
            let all_below=gpu.iter().all(|g|above(target,*g)==Some(false));
            let sharp_ok=visuals.iter().all(|id|above(active,HWND(*id as *mut _))==Some(true))
                && gpu.iter().all(|g|above(active,*g)==Some(true));
            let input_ok=WindowFromPoint(POINT{x:620,y:520})==target;
            let foreground_ok=GetForegroundWindow()==fg;
            let root_normal=GetWindowLongW(root,GWL_EXSTYLE) as u32&WS_EX_TOPMOST.0==0;
            // Restore the old ordering to verify the identical foreign call
            // succeeds once its veto is removed.
            lower_window(root,target).unwrap();
            // The same target accepts the identical call when its veto is removed.
            SetWindowLongPtrW(target,GWLP_USERDATA,0);
            let accepted=lower_window(target,root);
            let moved=above(target,root);
            let rebound=HWND(rebound_id as *mut _);
            let rebound_result=crate::placement_trace::lower(rebound,root,"fixture-later-raise");
            let rebound_after=above(rebound,root);
            SetWindowPos(rebound,Some(HWND_TOP),0,0,0,0,SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE).unwrap();
            crate::placement_trace::observe_next();
            let rebound_report=crate::placement_trace::report();
            let _=PostThreadMessageW(tid,WM_QUIT,WPARAM(0),LPARAM(0));
            worker.join().unwrap();
            for g in gpu {let _=DestroyWindow(g);}
            let _=DestroyWindow(grain);let _=DestroyWindow(tint);let _=DestroyWindow(root);let _=DestroyWindow(active);
            assert!(!declined);assert_eq!(still_above,Some(true));
            assert!(repaired && all_below && sharp_ok && input_ok && foreground_ok && root_normal,"divider fallback must preserve sharp/input/activation/band");
            assert_eq!(initial,Some(true)); assert!(result.is_ok()); assert_eq!(unchanged,Some(true));
            assert!(report.contains("fixture-reject-zorder") && report.contains("result=ok"));
            assert!(report.contains("next_after_ms=") && report.contains("target_security=[integrity_rid="));
            assert!(accepted.is_ok()); assert_eq!(moved,Some(false));
            assert!(rebound_result.is_ok()); assert_eq!(rebound_after,Some(false));
            let line=rebound_report.lines().find(|line|line.contains("fixture-later-raise")).unwrap();
            assert!(line.split(" after=[").nth(1).unwrap().split(" next_after_ms=").next().unwrap().contains("above_root=Some(false)"));
            assert!(line.split(" next=[").nth(1).unwrap().contains("above_root=Some(true)"));
            println!("cross-thread veto: SetWindowPos=Ok, position unchanged; accepting same call moves below divider");
        }
    }
}

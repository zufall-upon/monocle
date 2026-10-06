//! Native investigation of the Preview 5 ordering sequence. No user applications.
//! These HWNDs model roles, not Firefox/Tablacus internals or physical monitors.
use windows::{core::w, Win32::{Foundation::*, System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::*}};

struct Window(HWND);
impl Drop for Window { fn drop(&mut self) { unsafe { let _ = DestroyWindow(self.0); } } }
unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(h,m,w,l)
}
unsafe fn place(h: HWND, after: HWND) {
    SetWindowPos(h,Some(after),0,0,0,0,SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE).unwrap();
}
unsafe fn above(h: HWND, other: HWND) -> bool {
    let mut cur=h;
    for _ in 0..4096 {
        cur=GetWindow(cur,GW_HWNDNEXT).unwrap_or_default();
        if cur.0.is_null() { return false; }
        if cur==other { return true; }
    }
    panic!("unexpected z-order cycle");
}
unsafe fn pump() {
    let mut msg=MSG::default();
    while PeekMessageW(&mut msg,None,0,0,PM_REMOVE).as_bool() {
        let _=TranslateMessage(&msg); DispatchMessageW(&msg);
    }
}
unsafe fn focus(h: HWND) {
    assert!(SetForegroundWindow(h).as_bool(),"fixture cannot activate its own window");
    pump();
    assert_eq!(GetForegroundWindow(),h);
}
unsafe fn create(owner: Option<HWND>, visual: bool, x: i32) -> Window {
    let instance=GetModuleHandleW(None).unwrap();
    let ex=if visual { crate::input_policy::visual_ex_style(false) } else { WS_EX_TOOLWINDOW };
    let window=Window(CreateWindowExW(ex,w!("DeepLiteZFixture"),w!("fixture"),WS_POPUP,
        x,300,100,100,owner,None,Some(instance.into()),None).unwrap());
    if visual { SetLayeredWindowAttributes(window.0,COLORREF(0),255,LWA_ALPHA).unwrap(); }
    let _=ShowWindow(window.0,SW_SHOWNOACTIVATE);
    window
}
unsafe fn dump(stage: &str, named: &[(&str,HWND)]) {
    let mut order=named.to_vec();
    order.sort_by(|a,b| if a.1==b.1 {std::cmp::Ordering::Equal} else if above(a.1,b.1) {std::cmp::Ordering::Less} else {std::cmp::Ordering::Greater});
    println!("{stage}: {}",order.iter().map(|(n,_)|*n).collect::<Vec<_>>().join(" > "));
}

#[test]
fn native_three_gpu_windows_with_ignored_owner_and_settings_transitions() {
    unsafe {
        let instance=GetModuleHandleW(None).unwrap();
        assert_ne!(RegisterClassExW(&WNDCLASSEXW{cbSize:std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc:Some(proc),hInstance:instance.into(),lpszClassName:w!("DeepLiteZFixture"),..Default::default()}),0);
        for shared_hidden_owner in [false,true] {
        println!("scenario shared_hidden_owner={shared_hidden_owner}");
        let helper=create(None,false,820);
        let _=ShowWindow(helper.0,SW_HIDE);
        let common=if shared_hidden_owner {Some(helper.0)} else {None};
        let root=create(None,true,100);
        let tint=create(Some(root.0),true,100);
        let grain=create(Some(tint.0),true,100);
        let gpu=[create(None,true,100),create(None,true,220),create(None,true,340)];
        let ignored=create(None,false,460);
        let background=create(common,false,100);
        let popup=create(Some(background.0),false,220);
        let active=create(common,false,580);
        let settings=create(None,false,700);
        place(settings.0,HWND_TOPMOST);
        let named=[("root",root.0),("tint",tint.0),("grain",grain.0),
            ("gpu0",gpu[0].0),("gpu1",gpu[1].0),("gpu2",gpu[2].0),
            ("ignored",ignored.0),("background",background.0),("popup",popup.0),
            ("active",active.0),("settings",settings.0)];
        let pin_gpu=|| { for g in &gpu { place(g.0,root.0); } };
        // Preview 5 setup: sharp block, root, background block, then GPU pin.
        focus(active.0);
        place(ignored.0,HWND_TOP); place(active.0,ignored.0); place(root.0,active.0);
        place(popup.0,root.0); place(background.0,popup.0); pin_gpu();
        dump("setup",&named);
        for g in &gpu {
            assert!(!above(background.0,g.0),"background above GPU after setup");
            assert!(!above(popup.0,g.0),"popup above GPU after setup");
            assert!(above(active.0,g.0)); assert!(above(ignored.0,g.0));
        }
        // Actual settings activation, retain active anchor, then return.
        focus(settings.0); pin_gpu(); focus(active.0); pin_gpu();
        dump("settings-return",&named);
        for g in &gpu { assert!(!above(background.0,g.0)); }
        // Activate the owner family, then another app: same demotion sequence
        // as the foreground tracker (owner first in the focused group).
        focus(background.0); place(popup.0,background.0); pin_gpu();
        focus(active.0); place(background.0,root.0); place(popup.0,root.0);
        dump("immediately-after-demotion",&named);
        println!("root boundary says background above={} while GPU0 above={}",
            above(background.0,root.0),above(background.0,gpu[0].0));
        // Model two complete tracker ticks, not just the immediate transient.
        for _ in 0..2 {
            pin_gpu();
            if !above(ignored.0,root.0) { place(ignored.0,HWND_TOP); }
            for h in [popup.0,background.0] { if above(h,root.0) { place(h,root.0); } }
            pump();
        }
        dump("settled",&named);
        for g in &gpu {
            assert!(!above(background.0,g.0),"inactive owner remains above GPU");
            assert!(!above(popup.0,g.0),"inactive popup remains above GPU");
            assert!(above(active.0,g.0)); assert!(above(ignored.0,g.0));
        }
        // The visual stack must not become an input target at the background.
        assert_eq!(WindowFromPoint(POINT{x:120,y:320}),background.0);
        }
    }
}

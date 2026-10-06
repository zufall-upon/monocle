//! Native investigation of the Preview 5 ordering sequence. No user applications.
//! These HWNDs model roles, not Firefox/Tablacus internals or physical monitors.
use windows::{core::w, Win32::{Foundation::*, System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::*}};

static OWNER_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
struct Window(HWND);
impl Drop for Window { fn drop(&mut self) { unsafe { let _ = DestroyWindow(self.0); } } }
unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(h,m,w,l)
}
unsafe fn place(h: HWND, after: HWND) {
    SetWindowPos(h,Some(after),0,0,0,0,SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE).unwrap();
}
unsafe fn app_place(h: HWND, after: HWND, lowering: bool) {
    let bit=if lowering {1} else {2};
    let preserve=OWNER_MODE.load(std::sync::atomic::Ordering::Relaxed)&bit!=0;
    if lowering && preserve { crate::window_order::lower_window(h,after).unwrap(); return; }
    let flags=SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE;
    let flags=if preserve {flags|SWP_NOOWNERZORDER} else {flags};
    SetWindowPos(h,Some(after),0,0,0,0,flags).unwrap();
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

unsafe fn reconcile_model(root: HWND, sharp: HWND, targets: &[HWND], gpu: &[Window], stage: &str, named: &[(&str,HWND)]) {
    for tick in 0..4 {
        for g in gpu { place(g.0,root); }
        // EnumWindows-style snapshot; production iterates this original order
        // even when earlier moves change later HWND positions.
        let mut ordered=targets.to_vec();
        ordered.sort_by(|a,b|if a==b {std::cmp::Ordering::Equal} else if above(*a,*b) {std::cmp::Ordering::Less} else {std::cmp::Ordering::Greater});
        for h in ordered {
            let is_above=above(h,root);
            if crate::focus_policy::must_lower(h.0 as isize,&[sharp.0 as isize],&[],is_above) {
                app_place(h,root,true);
            } else if h==sharp && !is_above {
                app_place(h,HWND_TOP,false);
            }
        }
        pump();
        dump(&format!("{stage}-reconcile-{tick}"),named);
    }
    // Re-pin on the subsequent 16 ms tracker tick (reconcile is 250 ms).
    for g in gpu { place(g.0,root); }
}

#[test]
fn native_three_gpu_windows_with_ignored_owner_and_settings_transitions() {
    unsafe {
        let instance=GetModuleHandleW(None).unwrap();
        assert_ne!(RegisterClassExW(&WNDCLASSEXW{cbSize:std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc:Some(proc),hInstance:instance.into(),lpszClassName:w!("DeepLiteZFixture"),..Default::default()}),0);
        for owner_mode in 0..4 {
        OWNER_MODE.store(owner_mode,std::sync::atomic::Ordering::Relaxed);
        let mut violations=Vec::<String>::new();
        macro_rules! check { ($condition:expr, $message:expr) => { if !$condition { violations.push($message.into()); } }; }
        for shared_hidden_owner in [false,true] {
        println!("scenario owner_mode={owner_mode} shared_hidden_owner={shared_hidden_owner}");
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
        let named=[("hidden-helper",helper.0),("root",root.0),("tint",tint.0),("grain",grain.0),
            ("gpu0",gpu[0].0),("gpu1",gpu[1].0),("gpu2",gpu[2].0),
            ("ignored",ignored.0),("background",background.0),("popup",popup.0),
            ("active",active.0),("settings",settings.0)];
        let pin_gpu=|| { for g in &gpu { place(g.0,root.0); } };
        // Preview 5 setup: sharp block, root, background block, then GPU pin.
        focus(active.0);
        app_place(ignored.0,HWND_TOP,false); dump("setup-raise-ignored",&named);
        app_place(active.0,ignored.0,false); dump("setup-raise-active",&named);
        place(root.0,active.0); dump("setup-move-root",&named);
        app_place(popup.0,root.0,true); dump("setup-lower-popup",&named);
        app_place(background.0,popup.0,true); dump("setup-lower-background",&named);
        pin_gpu();
        dump("setup",&named);
        for g in &gpu {
            check!(!above(background.0,g.0),"background above GPU after setup");
            check!(!above(popup.0,g.0),"popup above GPU after setup");
            check!(above(active.0,g.0),"active below GPU"); check!(above(ignored.0,g.0),"ignored below GPU");
        }
        // Actual settings activation, retain active anchor, then return.
        focus(settings.0); pin_gpu(); focus(active.0); pin_gpu();
        dump("settings-return-immediate",&named);
        reconcile_model(root.0,active.0,&[active.0,popup.0,background.0],&gpu,"settings-return",&named);
        for g in &gpu { check!(!above(background.0,g.0),"background above GPU after settings reconciliation"); }
        // Activate the owner family, then another app: same demotion sequence
        // as the foreground tracker (owner first in the focused group).
        focus(background.0); app_place(popup.0,background.0,false); pin_gpu();
        focus(active.0); app_place(background.0,root.0,true); app_place(popup.0,root.0,true);
        dump("immediately-after-demotion",&named);
        println!("root boundary says background above={} while GPU0 above={}",
            above(background.0,root.0),above(background.0,gpu[0].0));
        reconcile_model(root.0,active.0,&[active.0,popup.0,background.0],&gpu,"demotion",&named);
        dump("settled",&named);
        check!(!above(background.0,root.0),"inactive owner remains above root");
        check!(!above(popup.0,root.0),"inactive popup remains above root");
        for g in &gpu {
            check!(!above(background.0,g.0),"inactive owner remains above GPU");
            check!(!above(popup.0,g.0),"inactive popup remains above GPU");
            check!(above(active.0,g.0),"active below GPU"); check!(above(ignored.0,g.0),"ignored below GPU");
        }
        // Reported interaction: old normal app -> ignored app -> own settings
        // -> ignored app. Ignored is still a real foreground anchor, not just
        // an independently raised window. Reuse production anchor policy.
        focus(background.0); pin_gpu(); focus(ignored.0);
        let windows=[ignored.0,background.0,active.0].map(|h|crate::focus_policy::Window{id:h.0 as isize,monitor:1});
        let (_,anchors)=crate::focus_policy::setup_anchors(&[(1,background.0.0 as isize)],&windows,
            GetForegroundWindow().0 as isize,background.0.0 as isize,false);
        assert_eq!(anchors,vec![(1,ignored.0.0 as isize)]);
        for h in [background.0,popup.0,active.0] { app_place(h,root.0,true); }
        focus(settings.0); pin_gpu(); focus(ignored.0);
        reconcile_model(root.0,ignored.0,&[ignored.0,active.0,popup.0,background.0],&gpu,"ignored-return",&named);
        dump("ignored-settings-return-settled",&named);
        for h in [background.0,popup.0,active.0] {
            check!(!above(h,root.0),"old normal app remains above root after ignored focus");
            for g in &gpu { check!(!above(h,g.0),"old normal app remains above GPU after ignored focus"); }
        }
        assert_eq!(GetForegroundWindow(),ignored.0);
        for g in &gpu { check!(above(ignored.0,g.0),"ignored below GPU after settings return"); }
        // The visual stack must not become an input target at the background.
        check!(WindowFromPoint(POINT{x:120,y:320})==background.0,"visual input target changed");
        }
        println!("owner_mode={owner_mode} violations={violations:?}");
        if owner_mode==1 || owner_mode==3 { assert!(violations.is_empty(),"owner-preserving candidate did not satisfy ordering"); }
        else if owner_mode==0 { assert!(!violations.is_empty(),"baseline owner coupling was not reproduced"); }
        }
    }
}

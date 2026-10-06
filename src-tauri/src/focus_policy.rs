//! Focus reconciliation decisions, independent of Win32 z-order calls.
#[derive(Clone, Copy, Debug)]
pub struct Window {
    pub id: isize,
    pub monitor: isize,
}

// Input windows are visible, non-minimized, eligible windows in z-order.
// Preserve a monitor's actual anchor if it is still available on that monitor;
// otherwise promote its next real window. A current foreground always wins.
pub fn anchors(previous: &[(isize,isize)], windows: &[Window], foreground: Option<isize>, per_monitor: bool) -> Vec<(isize,isize)> {
    let foreground = foreground.and_then(|id| windows.iter().find(|w| w.id == id));
    if !per_monitor {
        return foreground.or_else(|| previous.iter().find_map(|(_,id)| windows.iter().find(|w| w.id == *id)))
            .or_else(|| windows.first()).map(|w| vec![(w.monitor,w.id)]).unwrap_or_default();
    }
    let mut result = Vec::new();
    for window in windows {
        if result.iter().any(|(monitor,_)| *monitor == window.monitor) { continue; }
        let selected = foreground.filter(|w| w.monitor == window.monitor)
            .or_else(|| previous.iter().filter(|(monitor,_)| *monitor == window.monitor)
                .find_map(|(_,id)| windows.iter().find(|w| w.id == *id && w.monitor == window.monitor)))
            .unwrap_or(window);
        result.push((window.monitor,selected.id));
    }
    result
}

pub fn anchor_first(anchor: isize, mut group: Vec<isize>) -> Vec<isize> {
    group.retain(|id| *id != anchor);
    group.insert(0,anchor);
    group
}

pub fn must_lower(id: isize, sharp: &[isize], ignored: &[isize], above: bool) -> bool {
    above && !sharp.contains(&id) && !ignored.contains(&id)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn window(id:isize,monitor:isize)->Window { Window{id,monitor} }
    #[test]
    fn same_monitor_browser_to_editor_and_alt_tab_replace_anchor() {
        let windows=[window(2,10),window(1,10)];
        assert_eq!(anchors(&[(10,1)],&windows,Some(2),true),vec![(10,2)]);
        assert!(must_lower(1,&[2],&[],true));
        assert_eq!(anchors(&[(10,2)],&windows,Some(1),true),vec![(10,1)]);
    }
    #[test]
    fn per_monitor_retention_is_intentional_and_global_focus_demotes_browser() {
        let windows=[window(2,20),window(1,10)];
        let retained=anchors(&[(10,1)],&windows,Some(2),true);
        assert!(retained.contains(&(10,1))); assert!(retained.contains(&(20,2)));
        assert_eq!(anchors(&[(10,1)],&windows,Some(2),false),vec![(20,2)]);
        assert!(must_lower(1,&[2],&[],true));
    }
    #[test]
    fn close_hide_or_minimize_on_other_monitor_promotes_available_window() {
        // Window 1 has left the eligible snapshot; foreground 3 did not change.
        let windows=[window(3,20),window(2,10)];
        assert!(anchors(&[(10,1),(20,3)],&windows,Some(3),true).contains(&(10,2)));
        // Restoring 1 without activation must not make it sharp again.
        let restored=[window(1,10),window(3,20),window(2,10)];
        assert!(anchors(&[(10,2),(20,3)],&restored,Some(3),true).contains(&(10,2)));
        assert!(must_lower(1,&[2,3],&[],true));
    }
    #[test]
    fn background_self_raise_is_repaired_without_foreground_change() {
        assert!(must_lower(1,&[2],&[],true));
        assert!(!must_lower(1,&[2],&[],false));
        assert!(!must_lower(1,&[2],&[1],true));
        assert!(!must_lower(1,&[1,2],&[],true)); // explicit app-wide group
    }
    #[test]
    fn actual_anchor_survives_popup_enumeration_and_monitor_move() {
        assert_eq!(anchor_first(1,vec![9,1,8]),vec![1,9,8]);
        let windows=[window(1,20),window(3,10),window(2,20)];
        let selected=anchors(&[(10,1),(20,2)],&windows,Some(1),true);
        assert!(selected.contains(&(20,1))); assert!(selected.contains(&(10,3)));
        assert!(!selected.contains(&(20,2)));
    }
    #[test]
    fn closed_popup_keeps_owner_and_empty_desktop_has_no_stale_anchor() {
        assert_eq!(anchors(&[(10,9)],&[window(1,10)],Some(1),false),vec![(10,1)]);
        assert!(anchors(&[(10,9)],&[],None,true).is_empty());
    }
}

/// Classification is shared by foreground tracking and reconciliation. A tool
/// style alone says nothing about whether this is a real focusable app window.
#[derive(Clone, Copy, Debug, Default)]
pub struct Traits {
    pub own_process: bool, pub shell: bool, pub visible: bool, pub minimized: bool,
    pub cloaked: bool, pub topmost: bool, pub no_activate: bool,
    pub tool: bool, pub width: i32, pub height: i32,
}
pub fn exclusion(t: Traits) -> Option<&'static str> {
    if t.own_process { Some("own-process") }
    else if t.shell { Some("shell/menu") }
    else if !t.visible { Some("hidden") }
    else if t.minimized { Some("minimized") }
    else if t.cloaked { Some("cloaked") }
    else if t.topmost { Some("topmost-band") }
    else if t.width <= 0 || t.height <= 0 { Some("empty") }
    else { None }
}
pub fn can_anchor(t: Traits) -> bool {
    exclusion(t).is_none() && !t.no_activate && t.width >= 50 && t.height >= 50
}
pub fn receives_effect(t: Traits) -> bool { exclusion(t).is_none() }

/// Stop at an invisible helper, not the ultimate Win32 root owner. Independent
/// app windows sharing a hidden helper must not form one sharp group.
pub fn visible_owner_root(start: isize, chain: &[(isize, isize, bool)]) -> isize {
    let mut current = start;
    let mut seen = vec![start];
    while let Some((_, owner, usable)) = chain.iter().find(|(id,_,_)| *id == current) {
        if !usable || *owner == 0 || seen.contains(owner) { break; }
        current = *owner;
        seen.push(current);
    }
    current
}

#[cfg(test)]
mod classification_tests {
    use super::*;
    fn normal() -> Traits { Traits { visible:true, width:800, height:600, ..Default::default() } }
    #[test] fn visible_tool_window_is_an_anchor_and_background_target() {
        let tool=Traits { tool:true, ..normal() };
        assert!(can_anchor(tool)); assert!(receives_effect(tool));
        assert_eq!(anchors(&[(1,10)], &[Window{id:20,monitor:1},Window{id:10,monitor:1}],Some(20),false),vec![(1,20)]);
        assert!(must_lower(10,&[20],&[],true));
    }
    #[test] fn nonactivating_popup_gets_effect_but_cannot_replace_foreground() {
        let popup=Traits { tool:true, no_activate:true, width:30,height:30,..normal() };
        assert!(!can_anchor(popup)); assert!(receives_effect(popup));
    }
    #[test] fn topmost_is_reported_without_mutating_its_band() {
        let top=Traits {topmost:true,..normal()};
        assert_eq!(exclusion(top),Some("topmost-band"));
        assert!(!receives_effect(top)); assert!(!can_anchor(top));
    }
    #[test] fn self_shell_hidden_minimized_and_cloaked_remain_excluded() {
        for t in [Traits{own_process:true,..normal()},Traits{shell:true,..normal()},Traits{visible:false,..normal()},Traits{minimized:true,..normal()},Traits{cloaked:true,..normal()}] {
            assert!(!receives_effect(t)); assert!(!can_anchor(t));
        }
    }
    #[test] fn shared_hidden_owner_does_not_expand_global_focus() {
        let edges=[(10,1,false),(20,1,false)];
        assert_eq!(visible_owner_root(10,&edges),10);
        assert_eq!(visible_owner_root(20,&edges),20);
        assert!(must_lower(20,&[10],&[],true));
    }
    #[test] fn modal_and_nested_popups_keep_their_visible_owner() {
        let edges=[(30,20,true),(20,10,true),(10,1,false)];
        assert_eq!(visible_owner_root(30,&edges),10);
        assert_eq!(visible_owner_root(20,&edges),10);
        assert_eq!(visible_owner_root(10,&edges),10);
    }
    #[test] fn malformed_owner_cycles_terminate() {
        assert_eq!(visible_owner_root(1,&[(1,2,true),(2,1,true)]),2);
    }
}

/// Initial setup must use focus, not a z-order changed by ignored-app lifting.
pub fn setup_anchors(previous: &[(isize,isize)], windows: &[Window], current: isize, last_real: isize, per_monitor: bool) -> (Option<isize>,Vec<(isize,isize)>) {
    let preferred=[current,last_real].into_iter().find(|id| windows.iter().any(|w| w.id==*id));
    (preferred,anchors(previous,windows,preferred,per_monitor))
}

#[derive(Clone,Debug)]
pub struct IgnoredConfig { pub revision:u64, pub exes:Vec<String> }
impl IgnoredConfig {
    pub const fn new()->Self { Self { revision:0, exes:Vec::new() } }
    pub fn replace(&mut self, exes:Vec<String>)->bool {
        if self.exes==exes { return false; }
        self.exes=exes; self.revision=self.revision.wrapping_add(1); true
    }
    pub fn needs_refresh(&self, acknowledged:Option<u64>, periodic:bool, setup:bool)->bool {
        acknowledged!=Some(self.revision) || periodic || setup
    }
}

pub fn sharp_reason(id:isize,group:&[isize],ignored:bool,excluded:Option<&'static str>)->&'static str {
    if ignored { "settings.ignored_apps" }
    else if group.contains(&id) { "retained-focus-group" }
    else if excluded.is_some() { "excluded-window-category" }
    else { "background-effect-expected" }
}

#[cfg(test)]
mod setup_tests {
    use super::*;
    fn w(id:isize,monitor:isize)->Window { Window{id,monitor} }
    #[test] fn ignored_raise_cannot_replace_actual_global_foreground() {
        let z=[w(10,1),w(20,1),w(30,2)]; // ignored Firefox was lifted above TE
        assert_eq!(setup_anchors(&[],&z,20,30,false).1,vec![(1,20)]);
    }
    #[test] fn ignore_edit_with_settings_foreground_keeps_last_real_app() {
        let z=[w(10,1),w(20,1),w(30,2)];
        let (_,selected)=setup_anchors(&[(1,20)],&z,999,20,false);
        assert_eq!(selected,vec![(1,20)]);
        assert_eq!(sharp_reason(10,&[20],true,None),"settings.ignored_apps");
        assert_eq!(sharp_reason(10,&[20],false,None),"background-effect-expected");
        assert_eq!(sharp_reason(20,&[20],false,None),"retained-focus-group");
    }
    #[test] fn display_change_and_monitor_transfer_preserve_valid_anchors() {
        let z=[w(10,1),w(20,2),w(30,1)];
        let (_,chosen)=setup_anchors(&[(1,30),(2,20)],&z,999,20,true);
        assert!(chosen.contains(&(1,30))); assert!(chosen.contains(&(2,20)));
    }
    #[test] fn stale_last_real_is_revalidated_after_close_or_minimize() {
        let z=[w(10,1),w(30,1)];
        assert_eq!(setup_anchors(&[(1,30)],&z,999,20,false).1,vec![(1,30)]);
        assert_eq!(setup_anchors(&[(1,20)],&z,999,20,false).1,vec![(1,10)]);
        assert!(setup_anchors(&[(1,20)],&[],999,20,false).1.is_empty());
    }
    #[test] fn enumeration_acknowledges_snapshot_not_a_concurrent_update() {
        let mut config=IgnoredConfig::new();
        assert!(config.replace(vec!["firefox.exe".into()]));
        let snapshot=config.clone(); // released configuration lock, enumerating
        assert!(config.replace(vec![])); // another settings update meanwhile
        let acknowledged=Some(snapshot.revision);
        assert!(config.needs_refresh(acknowledged,false,false));
        let next=config.clone();
        assert!(next.exes.is_empty());
        assert!(!config.needs_refresh(Some(next.revision),false,false));
        assert!(config.needs_refresh(Some(next.revision),false,true));
        assert!(!config.replace(vec![]));
    }
}

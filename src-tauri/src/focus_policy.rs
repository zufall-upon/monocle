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

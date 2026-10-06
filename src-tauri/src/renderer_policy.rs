//! Capture-free mode never creates the GPU worker/resource graph.
pub fn is_live(renderer: &str) -> bool { renderer == "blur" }
pub fn needs_workers(live: bool, active: bool, fade: f64, capture_allowed: bool) -> bool {
    live && capture_allowed && (active || fade > 0.0)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn static_never_needs_workers() {
        for active in [false, true] { for fade in [0.0, 0.5, 1.0] {
            assert!(!needs_workers(false, active, fade, true));
        }}
    }
    #[test] fn live_starts_fades_stops_and_resumes() {
        assert!(needs_workers(true,true,0.0,true));
        assert!(needs_workers(true,false,0.5,true));
        assert!(!needs_workers(true,false,0.0,true));
        assert!(needs_workers(true,true,0.0,true));
    }
    #[test] fn switching_to_static_releases_even_during_fade() {
        assert!(needs_workers(true,false,0.8,true));
        assert!(!needs_workers(false,false,0.8,true));
        assert!(needs_workers(true,false,0.8,true));
    }
    #[test] fn unknown_renderer_and_failed_exclusion_are_safe() {
        for name in ["", "mask", "unknown"] { assert!(!is_live(name)); }
        assert!(is_live("blur"));
        assert!(!needs_workers(true,true,1.0,false));
    }
}

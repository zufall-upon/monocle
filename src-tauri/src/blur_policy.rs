//! Platform-independent decisions used by the Windows blur renderer.
//! These tests exercise policy, not WGC, Direct2D, visual quality or GPU timing.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Extent {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct CaptureDemand {
    pub active: bool,
    pub fade: f64,
    pub stddev: f32,
    pub desaturate: bool,
    pub tint_strength: f32,
}

impl CaptureDemand {
    pub fn wanted(self) -> bool {
        (self.active || self.fade > 0.0)
            && (self.stddev > 0.0 || self.desaturate || self.tint_strength > 0.001)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionAction {
    Start,
    Stop,
    Keep,
}

#[derive(Debug)]
pub struct UpdateState {
    extent: Extent,
    capturing: bool,
    has_frame: bool,
    frame_changed: bool,
    presented_revision: Option<u64>,
}

impl UpdateState {
    pub fn new(extent: Extent) -> Self {
        Self {
            extent,
            capturing: false,
            has_frame: false,
            frame_changed: false,
            presented_revision: None,
        }
    }

    // Start failures cause the Windows worker to be discarded and recreated.
    pub fn set_capture(&mut self, wanted: bool) -> SessionAction {
        match (self.capturing, wanted) {
            (false, true) => {
                self.capturing = true;
                SessionAction::Start
            }
            (true, false) => {
                self.capturing = false;
                self.invalidate();
                SessionAction::Stop
            }
            _ => SessionAction::Keep,
        }
    }

    fn invalidate(&mut self) {
        self.has_frame = false;
        self.frame_changed = false;
        self.presented_revision = None;
    }

    // A rejected frame also invalidates the old image. Never feed CopyResource
    // a resized texture or repaint a stale cached capture during reconciliation.
    pub fn accept_frame(&mut self, content: Extent, texture: Extent) -> bool {
        if !self.capturing || self.extent.width == 0 || self.extent.height == 0
            || content != self.extent || texture != self.extent
        {
            self.invalidate();
            return false;
        }
        self.has_frame = true;
        self.frame_changed = true;
        true
    }

    pub fn has_frame(&self) -> bool {
        self.has_frame
    }

    pub fn needs_draw(&self, revision: u64) -> bool {
        self.has_frame && (self.frame_changed || self.presented_revision != Some(revision))
    }

    // Called only after both EndDraw and Present succeed, so a failed draw is
    // never considered up to date.
    pub fn presented(&mut self, revision: u64) {
        self.presented_revision = Some(revision);
        self.frame_changed = false;
    }
}

// Exact physical-pixel ratios only: odd dimensions retain native quality.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcessingPlan { pub extent: Extent, pub divisor: u32, pub stddev: f32 }
pub fn processing_plan(extent: Extent, sigma: f32) -> ProcessingPlan {
    let divisor = if sigma.is_finite() && sigma >= 8.0 && extent.width >= 4 && extent.height >= 4 {
        if extent.width % 4 == 0 && extent.height % 4 == 0 { 4 }
        else if extent.width % 2 == 0 && extent.height % 2 == 0 { 2 } else { 1 }
    } else { 1 };
    ProcessingPlan { extent: Extent { width: extent.width / divisor, height: extent.height / divisor },
        divisor, stddev: sigma.max(0.0) / divisor as f32 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectKey { pub frame: u64, pub sigma: u32, pub desaturate: bool, pub tint: [u32; 4] }

#[derive(Default)]
pub struct EffectValidity { key: Option<EffectKey>, valid: [bool; 3], sharp: bool }
impl EffectValidity {
    pub fn update(&mut self, key: EffectKey) {
        if self.key != Some(key) { self.key = Some(key); self.valid = [false; 3]; self.sharp = false; }
    }
    pub fn needs_band(&self, index: usize) -> bool { !self.valid[index] }
    pub fn band_done(&mut self, index: usize) { self.valid[index] = true; }
    pub fn needs_sharp(&self) -> bool { !self.sharp }
    pub fn sharp_done(&mut self) { self.sharp = true; }
}

pub fn required_bands(mix: f64, sigma: f32) -> [bool; 3] {
    if sigma <= 0.0 { [false; 3] } else if mix > 0.001 { [true; 3] } else { [false, false, true] }
}

pub fn needs_source_frame(mix: f64, sigma: f32, input_current: bool, sharp_current: bool) -> bool {
    (required_bands(mix, sigma).iter().any(|&v| v) && !input_current)
        || ((mix > 0.001 || sigma <= 0.0) && !sharp_current)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HD: Extent = Extent { width: 1920, height: 1080 };
    const UHD: Extent = Extent { width: 3840, height: 2160 };

    fn blur(active: bool, fade: f64) -> CaptureDemand {
        CaptureDemand { active, fade, stddev: 24.0, desaturate: false, tint_strength: 0.0 }
    }

    #[test]
    fn static_frame_is_reused_but_mode_or_settings_changes_redraw() {
        let mut state = UpdateState::new(HD);
        assert_eq!(state.set_capture(blur(true, 1.0).wanted()), SessionAction::Start);
        assert!(!state.needs_draw(1)); // no first frame yet
        assert!(state.accept_frame(HD, HD));
        assert!(state.needs_draw(1));
        state.presented(1);
        assert!(!state.needs_draw(1));
        assert!(state.needs_draw(2)); // same desktop, changed mode/settings/fade
        state.presented(2);
        assert!(!state.needs_draw(2));
        assert!(state.accept_frame(HD, HD));
        assert!(state.needs_draw(2)); // moving content, unchanged settings
    }

    #[test]
    fn failed_or_overtaken_present_does_not_consume_pending_update() {
        let mut state = UpdateState::new(HD);
        state.set_capture(true);
        state.accept_frame(HD, HD);
        assert!(state.needs_draw(4));
        assert!(state.needs_draw(4)); // failed draw: no presented() acknowledgement
        state.presented(4);
        assert!(state.needs_draw(5)); // revision advanced during rendering
    }

    #[test]
    fn fade_out_finishes_before_stop_and_resume_waits_for_fresh_frame() {
        let mut state = UpdateState::new(HD);
        assert_eq!(state.set_capture(blur(true, 1.0).wanted()), SessionAction::Start);
        state.accept_frame(HD, HD);
        state.presented(1);
        assert_eq!(state.set_capture(blur(false, 0.2).wanted()), SessionAction::Keep);
        assert!(state.has_frame());
        assert_eq!(state.set_capture(blur(false, 0.0).wanted()), SessionAction::Stop);
        assert!(!state.has_frame());
        assert!(!state.needs_draw(2));
        assert!(!state.accept_frame(HD, HD)); // stale queued frame after stop
        assert_eq!(state.set_capture(blur(false, 0.0).wanted()), SessionAction::Keep);
        assert_eq!(state.set_capture(blur(true, 0.0).wanted()), SessionAction::Start);
        assert!(!state.needs_draw(3));
        assert!(state.accept_frame(HD, HD));
        assert!(state.needs_draw(3));
    }

    #[test]
    fn rapid_toggle_during_fade_keeps_session_and_cached_frame() {
        let mut state = UpdateState::new(HD);
        state.set_capture(true);
        state.accept_frame(HD, HD);
        state.presented(1);
        assert_eq!(state.set_capture(blur(false, 0.5).wanted()), SessionAction::Keep);
        assert_eq!(state.set_capture(blur(true, 0.4).wanted()), SessionAction::Keep);
        assert!(state.has_frame());
        assert!(state.needs_draw(2));
    }

    #[test]
    fn zero_blur_still_captures_for_tint_or_desaturation() {
        let mut demand = blur(true, 1.0);
        demand.stddev = 0.0;
        let mut state = UpdateState::new(HD);
        state.set_capture(true);
        assert_eq!(state.set_capture(demand.wanted()), SessionAction::Stop);
        demand.desaturate = true;
        assert_eq!(state.set_capture(demand.wanted()), SessionAction::Start);
        demand.desaturate = false;
        demand.tint_strength = 0.5;
        assert_eq!(state.set_capture(demand.wanted()), SessionAction::Keep);
        demand.active = false;
        demand.fade = 0.0;
        assert_eq!(state.set_capture(demand.wanted()), SessionAction::Stop);
    }

    #[test]
    fn resized_content_or_texture_invalidates_capture_before_copy() {
        for (content, texture) in [(UHD, HD), (HD, UHD), (UHD, UHD)] {
            let mut state = UpdateState::new(HD);
            state.set_capture(true);
            state.accept_frame(HD, HD);
            state.presented(1);
            assert!(!state.accept_frame(content, texture));
            assert!(!state.has_frame());
            assert!(!state.needs_draw(2));
        }
        // The monitor supervisor replaces all resources, including this state.
        let mut replacement = UpdateState::new(UHD);
        replacement.set_capture(true);
        assert!(!replacement.needs_draw(2));
        assert!(replacement.accept_frame(UHD, UHD));
        assert!(replacement.needs_draw(2));
    }

    #[test]
    fn monitors_keep_independent_pending_frames() {
        let mut monitors = [UpdateState::new(HD), UpdateState::new(UHD), UpdateState::new(HD)];
        for state in &mut monitors { state.set_capture(true); }
        monitors[1].accept_frame(UHD, UHD);
        assert!(!monitors[0].needs_draw(1));
        assert!(monitors[1].needs_draw(1));
        assert!(!monitors[2].needs_draw(1));
        monitors[1].set_capture(false);
        assert!(!monitors[1].needs_draw(1));
    }

    #[test]
    fn processing_keeps_exact_physical_scale_at_edges_and_rotation() {
        for extent in [HD, UHD, Extent { width: 1080, height: 1920 }, Extent { width: 1918, height: 1078 }] {
            let p = processing_plan(extent, 24.0);
            assert_eq!(p.extent.width * p.divisor, extent.width);
            assert_eq!(p.extent.height * p.divisor, extent.height);
            assert_eq!(p.stddev * p.divisor as f32, 24.0);
        }
    }
    #[test]
    fn weak_and_odd_blur_stay_native() {
        assert_eq!(processing_plan(HD, 7.99).divisor, 1);
        assert_eq!(processing_plan(HD, 8.0).divisor, 4);
        assert_eq!(processing_plan(Extent { width: 1919, height: 1079 }, 24.0).divisor, 1);
        assert_eq!(processing_plan(Extent { width: 2, height: 2 }, 24.0).divisor, 1);
    }
    fn key(frame: u64) -> EffectKey { EffectKey { frame, sigma: 24f32.to_bits(), desaturate: false, tint: [0; 4] } }
    #[test]
    fn fade_and_focus_updates_do_not_invalidate_effects() {
        let mut cache = EffectValidity::default(); cache.update(key(1)); cache.band_done(2);
        for _ in 0..100 { cache.update(key(1)); assert!(!cache.needs_band(2)); }
        cache.update(key(2)); assert!(cache.needs_band(2));
    }
    #[test]
    fn mode_transition_builds_only_missing_bands() {
        let mut cache = EffectValidity::default(); cache.update(key(1));
        assert_eq!(required_bands(0.0, 24.0), [false, false, true]);
        cache.band_done(2);
        let missing: Vec<_> = required_bands(0.5, 24.0).iter().enumerate().filter(|(i,v)| **v && cache.needs_band(*i)).map(|(i,_)| i).collect();
        assert_eq!(missing, vec![0,1]);
        cache.band_done(0); cache.band_done(1);
        assert!((0..3).all(|i| !cache.needs_band(i)));
        assert_eq!(required_bands(1.0, 0.0), [false; 3]);
    }
    #[test]
    fn parameter_changes_invalidate_sharp_and_blur() {
        let mut cache = EffectValidity::default(); cache.update(key(1)); cache.band_done(2); cache.sharp_done();
        let mut changed = key(1); changed.tint[3] = 0.5f32.to_bits(); cache.update(changed);
        assert!(cache.needs_sharp()); assert!(cache.needs_band(2));
    }
    #[test]
    fn three_monitor_work_counts_exclude_idle_and_fade_only_blur() {
        let mut caches: Vec<_> = (0..3).map(|_| EffectValidity::default()).collect();
        let mut blur = 0;
        for _tick in 0..200 { for cache in &mut caches { cache.update(key(1)); for i in 0..3 {
            if required_bands(1.0,24.0)[i] && cache.needs_band(i) { blur += 1; cache.band_done(i); }
        } } }
        assert_eq!(blur, 9); // three levels once per monitor, not 4800 old draws
    }
    #[test]
    fn zero_to_weak_blur_on_static_frame_requires_ingestion() {
        assert!(!needs_source_frame(0.0,0.0,false,true));
        assert!(needs_source_frame(0.0,4.0,false,true));
        assert!(!needs_source_frame(0.0,4.0,true,true));
        assert!(needs_source_frame(1.0,24.0,true,false));
        assert!(!needs_source_frame(0.0,24.0,true,false));
    }

}

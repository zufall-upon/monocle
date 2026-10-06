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

// Physical pixels: every bitmap and the D2D context explicitly use 96 DPI.
// This conservative boundary is a starting quality policy, not a measured
// perceptual guarantee. Weak blur and odd dimensions retain the native path.
pub const HALF_BLUR_MIN_STDDEV: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HalfDeepPlan {
    pub extent: Extent,
    pub stddev: f32,
}

pub fn half_deep_plan(extent: Extent, stddev: f32, mode_mix: f64) -> Option<HalfDeepPlan> {
    if !stddev.is_finite() || stddev < HALF_BLUR_MIN_STDDEV
        || !mode_mix.is_finite() || mode_mix >= 0.999
        || extent.width < 2 || extent.height < 2
        || extent.width % 2 != 0 || extent.height % 2 != 0
    {
        return None;
    }
    Some(HalfDeepPlan {
        extent: Extent { width: extent.width / 2, height: extent.height / 2 },
        stddev: stddev / 2.0,
    })
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
    fn half_plan_preserves_physical_sigma_and_exact_two_to_one_extent() {
        for extent in [HD, UHD, Extent { width: 1080, height: 1920 }] {
            let plan = half_deep_plan(extent, 24.0, 0.0).unwrap();
            assert_eq!(plan.extent.width * 2, extent.width);
            assert_eq!(plan.extent.height * 2, extent.height);
            assert_eq!(plan.stddev * 2.0, 24.0);
        }
    }

    #[test]
    fn weak_blur_and_odd_or_invalid_extents_keep_native_path() {
        for sigma in [0.0, 7.99, -1.0, f32::NAN, f32::INFINITY] {
            assert!(half_deep_plan(HD, sigma, 0.0).is_none());
        }
        assert!(half_deep_plan(HD, HALF_BLUR_MIN_STDDEV, 0.0).is_some());
        for extent in [Extent { width: 1919, height: 1080 },
            Extent { width: 1920, height: 1079 }, Extent { width: 0, height: 1080 },
            Extent { width: 1, height: 1 }]
        {
            assert!(half_deep_plan(extent, 24.0, 0.0).is_none());
        }
    }

    #[test]
    fn ambient_is_never_replaced_by_the_half_deep_path() {
        assert!(half_deep_plan(HD, 24.0, 1.0).is_none());
        assert!(half_deep_plan(HD, 24.0, 0.999).is_none());
        assert!(half_deep_plan(HD, 24.0, f64::NAN).is_none());
        // During a crossfade only the separate Deep branch may use this plan.
        assert!(half_deep_plan(HD, 24.0, 0.5).is_some());
    }
}

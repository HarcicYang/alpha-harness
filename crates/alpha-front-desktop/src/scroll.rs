use super::*;
use std::cell::Cell;

use crate::ui::{SCROLL_EXTERNAL_DISTANCE, SCROLL_MAX_FRAME_TIME, SCROLL_TIME_CONSTANT};

impl AlphaDesktop {
    pub(crate) fn scroll_handle(&self, region: ScrollRegion) -> &ScrollHandle {
        match region {
            ScrollRegion::Transcript => {
                unreachable!("the transcript uses GPUI list scrolling")
            }
            ScrollRegion::ModelPicker => &self.model_scroll,
            ScrollRegion::Sessions => &self.session_scroll,
            ScrollRegion::ProviderKinds => &self.provider_kind_scroll,
            ScrollRegion::Providers => &self.provider_scroll,
            ScrollRegion::Context => &self.context_scroll,
            ScrollRegion::Commands => &self.command_scroll,
            ScrollRegion::SlashCommands => &self.slash_scroll,
            ScrollRegion::ChangeFiles => &self.change_file_scroll,
            ScrollRegion::ChangeDiff => &self.change_diff_scroll,
            ScrollRegion::ComposerMenu => &self.composer_menu_scroll,
            ScrollRegion::WorkspaceList => &self.workspace_list_scroll,
            ScrollRegion::WorkspaceSessions => &self.workspace_session_scroll,
        }
    }

    pub(crate) fn prepare_scroll_motion(
        &self,
        region: ScrollRegion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if region == ScrollRegion::Transcript {
            self.transcript_motion
                .prepare(&self.transcript_list, window, cx);
            return;
        }
        let handle = self.scroll_handle(region).clone();
        self.scroll_motion[region.index()].prepare(&handle, window, cx);
    }

    pub(crate) fn prepare_tool_scroll_motion(&self, window: &mut Window, cx: &mut Context<Self>) {
        for state in self.tool_outputs.values().filter(|state| state.expanded) {
            state.scroll_motion.prepare(&state.scroll, window, cx);
        }
    }

    pub(crate) fn reset_scroll(&mut self, region: ScrollRegion) {
        if region == ScrollRegion::Transcript {
            self.transcript_list.scroll_to(ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            });
            self.transcript_motion.reset(0.);
            return;
        }
        let handle = self.scroll_handle(region).clone();
        handle.set_offset(point(px(0.), px(0.)));
        self.scroll_motion[region.index()].reset(f32::from(handle.offset().y));
    }

    pub(crate) fn scroll_to_bottom(&mut self, region: ScrollRegion) {
        if region == ScrollRegion::Transcript {
            self.transcript_list.set_follow_mode(FollowMode::Tail);
            self.transcript_list.scroll_to_end();
            self.transcript_motion.reset(-f32::from(
                self.transcript_list.max_offset_for_scrollbar().y,
            ));
            return;
        }
        let handle = self.scroll_handle(region).clone();
        self.scroll_motion[region.index()].reset(f32::from(handle.offset().y));
        handle.scroll_to_bottom();
    }

    pub(crate) fn handle_smooth_scroll_wheel(
        &mut self,
        region: ScrollRegion,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
        if delta == 0. {
            return;
        }

        if region == ScrollRegion::Transcript {
            self.transcript_motion.push(&self.transcript_list, delta);
            window.prevent_default();
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let handle = self.scroll_handle(region).clone();
        self.scroll_motion[region.index()].push(&handle, delta);
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn handle_tool_scroll_wheel(
        &mut self,
        tool_id: ToolCallId,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
        if delta == 0. {
            return;
        }

        let Some(state) = self.tool_outputs.get_mut(&tool_id) else {
            return;
        };
        state.scroll_motion.push(&state.scroll, delta);
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn reset_tool_scroll(&mut self, tool_id: ToolCallId) {
        let Some(state) = self.tool_outputs.get_mut(&tool_id) else {
            return;
        };
        state.scroll.set_offset(point(px(0.), px(0.)));
        state.scroll_motion.reset(0.);
    }
}

/// Whether content is hidden above and below a virtualized list.
pub(crate) fn list_scroll_edges(list: &ListState) -> (bool, bool) {
    let offset = -f32::from(list.scroll_px_offset_for_scrollbar().y);
    let maximum = f32::from(list.max_offset_for_scrollbar().y).max(0.);
    (offset > 0.5, offset < maximum - 0.5)
}

/// Whether content is hidden above and below a scroll handle.
pub(crate) fn panel_scroll_edges(handle: &ScrollHandle) -> (bool, bool) {
    let offset = -f32::from(handle.offset().y);
    let maximum = f32::from(handle.max_offset().y).max(0.);
    (offset > 0.5, offset < maximum - 0.5)
}

#[derive(Debug, Default)]
pub(crate) struct SmoothListScroll {
    applied_y: Cell<f32>,
    remaining_y: Cell<f32>,
    last_frame: Cell<Option<Instant>>,
    /// Sub-pixel motion already consumed but not applied yet.
    pending_px: Cell<f32>,
}

impl SmoothListScroll {
    pub(crate) fn reset(&self, offset_y: f32) {
        self.applied_y.set(offset_y);
        self.remaining_y.set(0.);
        self.last_frame.set(Some(Instant::now()));
        self.pending_px.set(0.);
    }

    pub(crate) fn push(&self, list: &ListState, delta_y: f32) {
        let actual_y = -f32::from(list.scroll_px_offset_for_scrollbar().y);
        if (actual_y - self.applied_y.get()).abs() > SCROLL_EXTERNAL_DISTANCE {
            self.reset(actual_y);
        }
        let maximum = f32::from(list.max_offset_for_scrollbar().y).max(0.);
        let applied_y = self.applied_y.get();
        let target_y = (applied_y + self.remaining_y.get() - delta_y).clamp(0., maximum);
        self.remaining_y.set(target_y - applied_y);
        // Do not restart the frame clock here: a burst of wheel events would reset
        // the easing every time and the motion would stutter.
    }

    pub(crate) fn prepare(
        &self,
        list: &ListState,
        window: &mut Window,
        cx: &mut Context<AlphaDesktop>,
    ) {
        let now = Instant::now();
        let actual_y = -f32::from(list.scroll_px_offset_for_scrollbar().y);
        if (actual_y - self.applied_y.get()).abs() > SCROLL_EXTERNAL_DISTANCE {
            self.reset(actual_y);
            return;
        }
        let remaining_y = self.remaining_y.get();
        if remaining_y == 0. {
            self.last_frame.set(Some(now));
            return;
        }
        let maximum = f32::from(list.max_offset_for_scrollbar().y).max(0.);
        let elapsed = self.last_frame.get().map_or(Duration::ZERO, |last_frame| {
            now.saturating_duration_since(last_frame)
                .min(SCROLL_MAX_FRAME_TIME)
        });
        let progress = 1. - (-(elapsed.as_secs_f32()) / SCROLL_TIME_CONSTANT.as_secs_f32()).exp();
        let (consumed_y, next_remaining) =
            smooth_list_scroll_step(actual_y, maximum, remaining_y, progress, cx.reduce_motion());
        // A virtualized list re-measures whatever the scroll reveals, so moving
        // it for less than a pixel buys a relayout nobody can see. Accumulate
        // the fraction and scroll one whole pixel at a time instead.
        let (whole_px, pending_px) = quantize_list_scroll(consumed_y, self.pending_px.get());
        self.pending_px.set(pending_px);
        let next_y = (actual_y + consumed_y).clamp(0., maximum);
        let finished = next_remaining.abs() <= 0.05 || consumed_y.abs() <= 0.001;
        if whole_px != 0. {
            list.scroll_by(px(whole_px));
        }
        if finished {
            // Land on the intended offset with a single final step.
            let rest = pending_px;
            if rest != 0. {
                list.scroll_by(px(rest));
                self.pending_px.set(0.);
            }
        }
        self.remaining_y
            .set(if finished { 0. } else { next_remaining });
        self.applied_y.set(next_y);
        self.last_frame.set(Some(now));
        if self.remaining_y.get() != 0. {
            window.request_animation_frame();
        }
    }
}

/// Split one frame of motion into whole pixels and a carried remainder.
pub(crate) fn quantize_list_scroll(consumed_y: f32, pending_px: f32) -> (f32, f32) {
    let total = pending_px + consumed_y;
    let whole = total.trunc();
    (whole, total - whole)
}

pub(crate) fn smooth_list_scroll_step(
    actual_y: f32,
    maximum_y: f32,
    remaining_y: f32,
    progress: f32,
    reduce_motion: bool,
) -> (f32, f32) {
    let actual_y = actual_y.clamp(0., maximum_y.max(0.));
    let maximum_y = maximum_y.max(0.);
    let bounded_remaining = remaining_y.clamp(-actual_y, maximum_y - actual_y);
    let consumed_y = if reduce_motion {
        bounded_remaining
    } else {
        bounded_remaining * progress.clamp(0., 1.)
    };
    (consumed_y, remaining_y - consumed_y)
}

#[cfg(test)]
mod tests {
    use super::smooth_list_scroll_step;

    #[test]
    fn smooth_list_scroll_handles_both_directions_and_boundaries() {
        let (down, down_remaining) = smooth_list_scroll_step(100., 1_000., 80., 0.25, false);
        let (up, up_remaining) = smooth_list_scroll_step(100., 1_000., -80., 0.25, false);
        let (top, remaining_at_top) = smooth_list_scroll_step(10., 1_000., -80., 1., false);
        let (reduced, reduced_remaining) = smooth_list_scroll_step(100., 1_000., -80., 0.25, true);

        assert_eq!(down, 20.);
        assert_eq!(down_remaining, 60.);
        assert_eq!(up, -20.);
        assert_eq!(up_remaining, -60.);
        assert_eq!(top, -10.);
        assert_eq!(remaining_at_top, -70.);
        assert_eq!(reduced, -80.);
        assert_eq!(reduced_remaining, 0.);
    }

    #[test]
    fn list_scroll_quantizes_to_whole_pixels() {
        let (first, pending) = super::quantize_list_scroll(0.4, 0.);
        assert_eq!(first, 0.);
        let (second, pending) = super::quantize_list_scroll(0.4, pending);
        assert_eq!(second, 0.);
        let (third, pending) = super::quantize_list_scroll(0.4, pending);
        assert_eq!(third, 1.);
        assert!((pending - 0.2).abs() < 0.001);
    }
}

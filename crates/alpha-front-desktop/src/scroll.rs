use super::*;

impl AlphaDesktop {
    pub(crate) fn scroll_handle(&self, region: ScrollRegion) -> &ScrollHandle {
        match region {
            ScrollRegion::Transcript => &self.transcript_scroll,
            ScrollRegion::ModelPicker => &self.model_scroll,
            ScrollRegion::Sessions => &self.session_scroll,
            ScrollRegion::ProviderKinds => &self.provider_kind_scroll,
            ScrollRegion::Providers => &self.provider_scroll,
            ScrollRegion::Context => &self.context_scroll,
            ScrollRegion::Commands => &self.command_scroll,
            ScrollRegion::SlashCommands => &self.slash_scroll,
            ScrollRegion::ChangeFiles => &self.change_file_scroll,
            ScrollRegion::ChangeDiff => &self.change_diff_scroll,
            ScrollRegion::ToolOutput => &self.tool_scroll,
            ScrollRegion::ComposerMenu => &self.composer_menu_scroll,
        }
    }

    pub(crate) fn prepare_scroll_motion(
        &self,
        region: ScrollRegion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handle = self.scroll_handle(region).clone();
        self.scroll_motion[region.index()].prepare(&handle, window, cx);
    }

    pub(crate) fn reset_scroll(&mut self, region: ScrollRegion) {
        let handle = self.scroll_handle(region).clone();
        handle.set_offset(point(px(0.), px(0.)));
        self.scroll_motion[region.index()].reset(f32::from(handle.offset().y));
    }

    pub(crate) fn scroll_to_bottom(&mut self, region: ScrollRegion) {
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

        let handle = self.scroll_handle(region).clone();
        self.scroll_motion[region.index()].push(&handle, delta);
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }
}

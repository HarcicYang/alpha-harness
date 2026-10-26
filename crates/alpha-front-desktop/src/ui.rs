use super::*;
use std::cell::Cell;

use gpui_kit::component::animation::{EffectTransition, cubic_bezier};

pub(crate) const SCROLL_REGION_COUNT: usize = 12;
const SCROLL_RESPONSE: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScrollRegion {
    Transcript,
    ModelPicker,
    Sessions,
    ProviderKinds,
    Providers,
    Context,
    Commands,
    SlashCommands,
    ChangeFiles,
    ChangeDiff,
    ToolOutput,
    ComposerMenu,
}

impl ScrollRegion {
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Transcript => 0,
            Self::ModelPicker => 1,
            Self::Sessions => 2,
            Self::ProviderKinds => 3,
            Self::Providers => 4,
            Self::Context => 5,
            Self::Commands => 6,
            Self::SlashCommands => 7,
            Self::ChangeFiles => 8,
            Self::ChangeDiff => 9,
            Self::ToolOutput => 10,
            Self::ComposerMenu => 11,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct SmoothScroll {
    target_y: Cell<f32>,
    applied_y: Cell<f32>,
    generation: Cell<u64>,
}

impl SmoothScroll {
    pub(crate) fn reset(&mut self, offset_y: f32) {
        self.target_y.set(offset_y);
        self.applied_y.set(offset_y);
        self.generation.set(self.generation.get().wrapping_add(1));
    }

    pub(crate) fn push(&mut self, handle: &ScrollHandle, delta_y: f32) {
        let actual_y = f32::from(handle.offset().y);
        if (actual_y - self.applied_y.get()).abs() > f32::EPSILON {
            self.reset(actual_y);
        }
        let maximum = f32::from(handle.max_offset().y).max(0.);
        let target = (self.target_y.get() + delta_y).clamp(-maximum, 0.);
        self.target_y.set(target);
        set_vertical_offset(handle, self.applied_y.get());
    }

    pub(crate) fn prepare(
        &self,
        region: ScrollRegion,
        handle: &ScrollHandle,
        window: &mut Window,
        cx: &mut Context<AlphaDesktop>,
    ) {
        let actual_y = f32::from(handle.offset().y);
        if (actual_y - self.applied_y.get()).abs() > f32::EPSILON {
            self.target_y.set(actual_y);
            self.applied_y.set(actual_y);
            self.generation.set(self.generation.get().wrapping_add(1));
        }

        let target = px(self.target_y.get());
        let animated = gpui_kit::base::spring(
            format!("scroll-spring:{}:{}", region.index(), self.generation.get()),
            target,
            gpui_kit::base::Spring::new(SCROLL_RESPONSE).with_epsilon(0.25),
            window,
            cx,
        );
        let animated_y = f32::from(animated);
        set_vertical_offset(handle, animated_y);
        self.applied_y.set(animated_y);
    }
}

fn set_vertical_offset(handle: &ScrollHandle, offset_y: f32) {
    let offset = handle.offset();
    handle.set_offset(point(offset.x, px(offset_y)));
}

pub(crate) trait SmoothScrollShellExt: InteractiveElement + Sized {
    fn smooth_scroll_shell(self, region: ScrollRegion, cx: &mut Context<AlphaDesktop>) -> Self {
        let mut element = self;
        element
            .interactivity()
            .on_scroll_wheel(cx.listener(move |this, event, window, cx| {
                this.handle_smooth_scroll_wheel(region, event, window, cx);
            }));
        element
    }
}

impl<T: InteractiveElement> SmoothScrollShellExt for T {}

pub(crate) fn icon_button(
    id: impl Into<ElementId>,
    icon: IconName,
    tooltip: &'static str,
) -> Button {
    Button::new(id)
        .icon(Icon::new(icon))
        .ghost()
        .compact()
        .size(px(29.))
        .rounded_full()
        .tooltip(tooltip)
        .tooltip_show_delay(Duration::from_millis(320))
}

pub(crate) trait ButtonStyleExt: Sized {
    fn accent(self) -> Self;
    fn destructive(self) -> Self;
}

impl ButtonStyleExt for Button {
    fn accent(self) -> Self {
        self.ghost()
            .bg(rgb(ACCENT))
            .border_1()
            .border_color(rgb(ACCENT_STRONG))
            .text_color(rgb(0x061014))
    }

    fn destructive(self) -> Self {
        self.ghost().bg(rgba(DANGER_SOFT)).text_color(rgb(DANGER))
    }
}

pub(crate) fn enter_from_below<E>(element: E, id: impl Into<ElementId>) -> AnyElement
where
    E: IntoElement + Styled + 'static,
{
    EffectTransition::new(MOTION_SLOW)
        .ease(cubic_bezier(0.16, 1., 0.3, 1.))
        .fade(0., 1.)
        .slide_y(px(8.), px(0.))
        .apply(div().w_full().child(element), id)
        .into_any_element()
}

pub(crate) fn popover_enter<E>(element: E, id: impl Into<ElementId>) -> AnyElement
where
    E: IntoElement + Styled + 'static,
{
    EffectTransition::new(MOTION_BASE)
        .ease(cubic_bezier(0.16, 1., 0.3, 1.))
        .fade(0., 1.)
        .slide_y(px(-5.), px(0.))
        .apply(element, id)
        .into_any_element()
}

pub(crate) fn backdrop_enter<E>(element: E, id: impl Into<ElementId>) -> AnyElement
where
    E: IntoElement + Styled + 'static,
{
    EffectTransition::new(MOTION_BASE)
        .ease(cubic_bezier(0.2, 0., 0., 1.))
        .fade(0., 1.)
        .apply(element, id)
        .into_any_element()
}

pub(crate) fn modal_enter<E>(element: E, id: impl Into<ElementId>) -> AnyElement
where
    E: IntoElement + Styled + 'static,
{
    EffectTransition::new(MOTION_BASE)
        .ease(cubic_bezier(0.16, 1., 0.3, 1.))
        .fade(0., 1.)
        .slide_y(px(8.), px(0.))
        .apply(element, id)
        .into_any_element()
}

pub(crate) fn icon_badge(icon: IconName, color: u32, background: u32, size: f32) -> Div {
    let background: Hsla = if background > 0x00ff_ffff {
        rgba(background).into()
    } else {
        rgb(background).into()
    };
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .bg(background)
        .text_color(rgb(color))
        .flex()
        .items_center()
        .justify_center()
        .child(Icon::new(icon).size(px(size * 0.52)))
}

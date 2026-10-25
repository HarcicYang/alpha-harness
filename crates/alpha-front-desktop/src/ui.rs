use super::*;
use gpui_kit::component::animation::{EffectTransition, cubic_bezier};

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

pub(crate) fn enter_from_below(element: AnyElement, id: impl Into<ElementId>) -> AnyElement {
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

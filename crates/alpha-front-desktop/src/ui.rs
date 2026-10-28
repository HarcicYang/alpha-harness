use super::*;
use std::cell::Cell;

pub(crate) const SCROLL_REGION_COUNT: usize = 12;
pub(crate) const SCROLLBAR_GUTTER: f32 = 14.;
const SCROLL_RESPONSE: Duration = Duration::from_millis(110);
const SCROLL_EXTERNAL_DISTANCE: f32 = 0.25;

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
        if (actual_y - self.applied_y.get()).abs() > SCROLL_EXTERNAL_DISTANCE {
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
        if (actual_y - self.applied_y.get()).abs() > SCROLL_EXTERNAL_DISTANCE {
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

pub(crate) fn scrollbar_rail(id: String, handle: &ScrollHandle) -> AnyElement {
    div()
        .id(format!("scrollbar-rail:{id}"))
        .absolute()
        .top(px(6.))
        .right(px(2.))
        .bottom(px(6.))
        .w(px(10.))
        .child(
            Scrollbar::vertical(handle)
                .id(format!("scrollbar:{id}"))
                .viewport_from_layout()
                .mode(ScrollbarMode::Always)
                .styles(|styles| {
                    styles
                        .track(|style| style.bg(transparent_black()).width(px(10.)))
                        .track_hover(|style| style.bg(rgba(0xffffff05).into()).width(px(10.)))
                        .track_active(|style| style.bg(rgba(0xffffff08).into()).width(px(10.)))
                        .thumb(|style| {
                            style
                                .bg(rgba(0x7077808a))
                                .width(px(4.))
                                .inset(px(3.))
                                .radius(px(999.))
                                .min_length(px(28.))
                        })
                        .thumb_hover(|style| {
                            style
                                .bg(rgba(0xa7adb5c7))
                                .width(px(5.))
                                .inset(px(2.5))
                                .radius(px(999.))
                        })
                        .thumb_active(|style| {
                            style
                                .bg(rgba(0x5cc8e6d9))
                                .width(px(5.))
                                .inset(px(2.5))
                                .radius(px(999.))
                        })
                }),
        )
        .into_any_element()
}

pub(crate) fn pulse_dot() -> AnyElement {
    div()
        .relative()
        .size(px(6.))
        .flex_none()
        .rounded_full()
        .bg(rgb(ACCENT))
        .child(
            div()
                .absolute()
                .size(px(6.))
                .rounded_full()
                .border_1()
                .border_color(rgba(ACCENT_BORDER))
                .with_animation(
                    "connection-pulse",
                    Animation::new(Duration::from_millis(3200)).repeat(),
                    |ring, progress| {
                        let spread = 3. * progress;
                        ring.top(px(-spread))
                            .left(px(-spread))
                            .size(px(6. + spread * 2.))
                            .opacity((1. - progress) * 0.65)
                    },
                ),
        )
        .into_any_element()
}

pub(crate) fn streaming_dots() -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(3.))
        .children((0..3).map(|index| {
            div()
                .size(px(4.))
                .rounded_full()
                .bg(rgb(ACCENT))
                .with_animation(
                    format!("streaming-dot:{index}"),
                    Animation::new(Duration::from_millis(1100)).repeat(),
                    move |dot, progress| {
                        let phase = (progress - index as f32 * 0.12).rem_euclid(1.);
                        let wave = (phase * std::f32::consts::TAU).sin().max(0.);
                        dot.top(px(-3. * wave)).opacity(0.35 + wave * 0.65)
                    },
                )
                .into_any_element()
        }))
        .into_any_element()
}

pub(crate) fn stream_cursor(id: impl Into<ElementId>) -> AnyElement {
    div()
        .h(px(2.))
        .w(px(12.))
        .rounded_full()
        .bg(rgb(ACCENT))
        .with_animation(
            id,
            Animation::new(Duration::from_millis(900)).repeat(),
            |cursor, progress| cursor.opacity(if progress < 0.5 { 1. } else { 0.15 }),
        )
        .into_any_element()
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
    spring_enter(div().w_full().child(element), id, px(8.), 49.)
}

pub(crate) fn popover_enter<E>(element: E, id: impl Into<ElementId>) -> AnyElement
where
    E: IntoElement + Styled + 'static,
{
    spring_enter(element, id, px(-5.), 47.)
}

pub(crate) fn backdrop_enter<E>(element: E, id: impl Into<ElementId>) -> AnyElement
where
    E: IntoElement + Styled + 'static,
{
    spring_enter(element, id, px(0.), 54.)
}

pub(crate) fn modal_enter<E>(element: E, id: impl Into<ElementId>) -> AnyElement
where
    E: IntoElement + Styled + 'static,
{
    spring_enter(element, id, px(12.), 54.)
}

fn spring_enter<E>(
    element: E,
    id: impl Into<ElementId>,
    distance: Pixels,
    damping: f32,
) -> AnyElement
where
    E: IntoElement + Styled + 'static,
{
    element
        .with_spring(
            id,
            SpringAnimation::new(SpringConfig::new(720., damping, 1.))
                .to(true)
                .from(false)
                .with_epsilon(0.002),
            move |mut element, phase| {
                let progress = phase.0;
                element = element
                    .opacity(progress.clamp(0., 1.))
                    .top(px(distance.as_f32() * (1. - progress)));
                element
            },
        )
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

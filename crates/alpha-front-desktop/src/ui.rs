use super::*;
use std::cell::Cell;

pub(crate) const SCROLL_REGION_COUNT: usize = 13;
pub(crate) const SCROLLBAR_GUTTER: f32 = 14.;
pub(crate) const SCROLL_TIME_CONSTANT: Duration = Duration::from_millis(32);
pub(crate) const SCROLL_MAX_FRAME_TIME: Duration = Duration::from_millis(16);
pub(crate) const SCROLL_EXTERNAL_DISTANCE: f32 = 0.25;

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
    ComposerMenu,
    WorkspaceList,
    WorkspaceSessions,
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
            Self::ComposerMenu => 10,
            Self::WorkspaceList => 11,
            Self::WorkspaceSessions => 12,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct SmoothScroll {
    applied_y: Cell<f32>,
    remaining_y: Cell<f32>,
    last_frame: Cell<Option<Instant>>,
}

impl SmoothScroll {
    pub(crate) fn reset(&self, offset_y: f32) {
        self.applied_y.set(offset_y);
        self.remaining_y.set(0.);
        self.last_frame.set(Some(Instant::now()));
    }

    pub(crate) fn push(&mut self, handle: &ScrollHandle, delta_y: f32) {
        let actual_y = f32::from(handle.offset().y);
        if (actual_y - self.applied_y.get()).abs() > SCROLL_EXTERNAL_DISTANCE {
            self.reset(actual_y);
        }
        let maximum = f32::from(handle.max_offset().y).max(0.);
        let applied_y = self.applied_y.get();
        let target_y =
            (self.applied_y.get() + self.remaining_y.get() + delta_y).clamp(-maximum, 0.);
        self.remaining_y.set(target_y - applied_y);
        self.last_frame.set(Some(Instant::now()));
        set_vertical_offset(handle, applied_y);
    }

    pub(crate) fn prepare(
        &self,
        handle: &ScrollHandle,
        window: &mut Window,
        cx: &mut Context<AlphaDesktop>,
    ) {
        let now = Instant::now();
        let actual_y = f32::from(handle.offset().y);
        if (actual_y - self.applied_y.get()).abs() > SCROLL_EXTERNAL_DISTANCE {
            self.reset(actual_y);
            return;
        }

        let remaining_y = self.remaining_y.get();
        if remaining_y == 0. {
            self.last_frame.set(Some(now));
            return;
        }

        let maximum = f32::from(handle.max_offset().y).max(0.);
        let next_y = if cx.reduce_motion() {
            (actual_y + remaining_y).clamp(-maximum, 0.)
        } else {
            let elapsed = self.last_frame.get().map_or(Duration::ZERO, |last_frame| {
                now.saturating_duration_since(last_frame)
                    .min(SCROLL_MAX_FRAME_TIME)
            });
            let progress =
                1. - (-(elapsed.as_secs_f32()) / SCROLL_TIME_CONSTANT.as_secs_f32()).exp();
            (actual_y + remaining_y * progress).clamp(-maximum, 0.)
        };

        let consumed_y = next_y - actual_y;
        let next_remaining = remaining_y - consumed_y;
        if next_remaining.abs() <= 0.05 || consumed_y.abs() <= 0.001 {
            self.remaining_y.set(0.);
        } else {
            self.remaining_y.set(next_remaining);
        }
        set_vertical_offset(handle, next_y);
        self.applied_y.set(next_y);
        self.last_frame.set(Some(now));
        if self.remaining_y.get() != 0. {
            window.request_animation_frame();
        }
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
                .max_fps(120)
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

pub(crate) fn list_scrollbar_rail(id: String, list: &ListState) -> AnyElement {
    div()
        .id(format!("scrollbar-rail:{id}"))
        .absolute()
        .top(px(6.))
        .right(px(2.))
        .bottom(px(6.))
        .w(px(10.))
        .child(
            Scrollbar::vertical(list)
                .id(format!("scrollbar:{id}"))
                .viewport_from_layout()
                .mode(ScrollbarMode::Always)
                .max_fps(120)
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
                .opacity(0.45),
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
                .opacity(0.45 + index as f32 * 0.2)
                .into_any_element()
        }))
        .into_any_element()
}

pub(crate) fn stream_cursor(id: impl Into<ElementId>) -> AnyElement {
    let _ = id.into();
    div()
        .h(px(2.))
        .w(px(12.))
        .rounded_full()
        .bg(rgb(ACCENT))
        .opacity(0.72)
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

    fn tool_scroll_shell(self, tool_id: ToolCallId, cx: &mut Context<AlphaDesktop>) -> Self {
        let mut element = self;
        element
            .interactivity()
            .on_scroll_wheel(cx.listener(move |this, event, window, cx| {
                this.handle_tool_scroll_wheel(tool_id, event, window, cx);
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
    E: IntoElement + InteractiveElement + Styled + 'static,
{
    spring_enter(element.occlude(), id, px(12.), 54.)
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

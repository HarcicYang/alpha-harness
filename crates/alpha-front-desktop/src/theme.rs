use super::*;

pub(crate) const CANVAS: u32 = 0x08090a;
pub(crate) const SURFACE: u32 = 0x101215;
pub(crate) const SURFACE_RAISED: u32 = 0x14171b;
pub(crate) const SURFACE_ACTIVE: u32 = 0x20252b;
pub(crate) const SURFACE_INSET: u32 = 0x0b0d0f;
pub(crate) const BORDER: u32 = 0x24282e;
pub(crate) const BORDER_STRONG: u32 = 0x343a43;
pub(crate) const BORDER_SUBTLE: u32 = 0xb4bcc61a;
pub(crate) const TEXT: u32 = 0xf0f2f4;
pub(crate) const TEXT_SECONDARY: u32 = 0xa7adb5;
pub(crate) const MUTED: u32 = 0x707780;
pub(crate) const FAINT: u32 = 0x484e56;
pub(crate) const ACCENT: u32 = 0x5cc8e6;
pub(crate) const ACCENT_STRONG: u32 = 0x84dcf3;
pub(crate) const ACCENT_SOFT: u32 = 0x5cc8e61a;
pub(crate) const ACCENT_BORDER: u32 = 0x5cc8e652;
pub(crate) const WARNING: u32 = 0x8eb9c8;
pub(crate) const WARNING_SOFT: u32 = 0x5cc8e613;
pub(crate) const DANGER: u32 = 0xc9808b;
pub(crate) const DANGER_SOFT: u32 = 0xc9808b17;
pub(crate) const USER_BAND: u32 = 0x15181c;
pub(crate) const SCRIM: u32 = 0x000000b8;

pub(crate) const RADIUS_XS: f32 = 8.;
pub(crate) const RADIUS_SM: f32 = 14.;
pub(crate) const RADIUS_MD: f32 = 24.;
pub(crate) const CONTENT_WIDTH: f32 = 780.;
pub(crate) const HEADER_HEIGHT: f32 = 56.;

pub(crate) const MOTION_BASE: Duration = Duration::from_millis(220);
pub(crate) const MOTION_SLOW: Duration = Duration::from_millis(320);

pub(crate) fn apply_theme(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::update(cx, |theme| {
        theme.font_size = px(13.);
        theme.mono_font_size = px(10.);
        theme.radius = px(RADIUS_SM);
        theme.radius_lg = px(RADIUS_MD);
    });
}

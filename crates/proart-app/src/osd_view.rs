//! On-screen indicator shown when the keyboard's brightness or volume keys
//! change a monitor, like macOS's own.

use gpui::{div, prelude::*, px, size, Bounds, Context, FontWeight, Pixels, Render, SharedString, Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions};

use crate::icons;
use crate::state::Shared;
use crate::theme::Theme;

pub const OSD_W: f32 = 280.0;
pub const OSD_H: f32 = 64.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsdKind {
    Brightness,
    Volume,
}

pub struct OsdView {
    pub kind: OsdKind,
    pub frac: f32,
    /// Shown instead of the percentage (e.g. "Mute").
    pub note: Option<SharedString>,
}

pub fn window_options(origin: gpui::Point<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin, size: size(px(OSD_W), px(OSD_H)) })),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("proart".into()),
        ..Default::default()
    }
}

impl Render for OsdView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let setting = cx.global::<Shared>().mgr.with_settings(|s| s.ui.theme.clone());
        let t = Theme::resolve(&setting, window.appearance());
        let icon = match self.kind {
            OsdKind::Brightness => "ui/brightness.svg",
            OsdKind::Volume => "ui/sound.svg",
        };
        let frac = self.frac.clamp(0.0, 1.0);
        let text = self.note.clone().unwrap_or_else(|| format!("{}", (frac * 100.0).round() as u32).into());
        let bar_w = OSD_W - 32.0 - 24.0 - 12.0 - 44.0;
        div()
            .size_full()
            .rounded(px(16.))
            .bg(t.card)
            .border_1()
            .border_color(t.border)
            .px(px(16.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .child(icons::icon(icon, 24., t.text))
            .child(div().w(px(bar_w)).h(px(6.)).rounded_full().bg(t.field).child(div().h_full().w(px(bar_w * frac)).rounded_full().bg(t.accent)))
            .child(div().w(px(44.)).flex().justify_end().text_size(px(14.)).font_weight(FontWeight::BOLD).text_color(t.text).child(text))
    }
}

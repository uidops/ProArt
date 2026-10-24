//! The menu-bar / tray popover: quick controls for the current display.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    div, img, point, prelude::*, px, size, AnyElement, App, Bounds, Context, FontWeight, ImageSource, IntoElement, Pixels, Render,
    SharedString, Subscription, Task, Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
};
use proart_core::features::{self, Feature};
use proart_core::manager::{DisplayState, Status};

use crate::actions;
use crate::controls::{confirm, feature_row, Ctl};
use crate::main_view::toast_stack;
use crate::state::Shared;
use crate::theme::Theme;
use crate::ui::{pill, spectrum_bar, DropItem, Dropdown, PillKind};

pub const PANEL_W: f32 = 380.0;
const BASE_H: f32 = 92.0;
const ROW_H: f32 = 45.0;
const FOOT_H: f32 = 44.0;

pub fn window_options(origin: gpui::Point<Pixels>, rows: usize) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin, size: size(px(PANEL_W), px(height_for(rows))) })),
        titlebar: None,
        focus: true,
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

fn height_for(rows: usize) -> f32 {
    BASE_H + rows.max(1) as f32 * ROW_H + FOOT_H
}

pub struct PanelView {
    seen: u64,
    had_toasts: bool,
    saw_active: bool,
    opened: Instant,
    last_h: f32,
    _activation: Subscription,
    _tick: Task<()>,
}

impl PanelView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> PanelView {
        let activation = cx.observe_window_activation(window, |this, window, _cx| {
            if window.is_window_active() {
                this.saw_active = true;
            } else if this.saw_active && this.opened.elapsed() > Duration::from_millis(300) {
                window.remove_window(); // clicked elsewhere
            }
        });
        let tick = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(120)).await;
            let alive = this
                .update(cx, |this, cx| {
                    let bus = cx.global::<Shared>().bus.clone();
                    let (g, toasts) = (bus.gen(), !bus.toasts().is_empty());
                    if g != this.seen || toasts || this.had_toasts {
                        this.seen = g;
                        this.had_toasts = toasts;
                        cx.notify();
                    }
                })
                .is_ok();
            if !alive {
                break;
            }
        });
        PanelView { seen: 0, had_toasts: false, saw_active: window.is_window_active(), opened: Instant::now(), last_h: 0.0, _activation: activation, _tick: tick }
    }
}

/// Visible quick controls for a display, in registry order.
pub fn quick_features(mgr: &proart_core::manager::Manager, st: &Arc<DisplayState>) -> Vec<&'static Feature> {
    let wanted = mgr.with_settings(|s| s.ui.quick_features.clone());
    features::all().iter().filter(|f| wanted.iter().any(|w| w == f.id) && mgr.visible(st, f) && mgr.status(st, f) == Status::Ok).collect()
}

impl Render for PanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (mgr, bus, logo) = {
            let s = cx.global::<Shared>();
            (s.mgr.clone(), s.bus.clone(), s.logo.clone())
        };
        let setting = mgr.with_settings(|s| s.ui.theme.clone());
        let t = Theme::resolve(&setting, window.appearance());
        let st = mgr.displays().into_iter().next();
        if let Some(st) = &st {
            bus.ensure_probed(&mgr, st);
        }
        let features = st.as_ref().map(|s| quick_features(&mgr, s)).unwrap_or_default();

        // fit the window to its rows once the probe has told us how many there are
        let h = height_for(features.len());
        if (h - self.last_h).abs() > 0.5 {
            self.last_h = h;
            window.resize(size(px(PANEL_W), px(h)));
        }

        let ctl = Ctl { mgr: mgr.clone(), bus: bus.clone(), t, on_learn: None };
        let probing = st.as_ref().is_some_and(|s| bus.is_probing(s.key()));
        let label = st.as_ref().map(|s| mgr.label(s)).unwrap_or_else(|| "No display detected".into());
        let rows: Vec<AnyElement> = match &st {
            Some(st) => features.iter().map(|f| feature_row(&ctl, st, f)).collect(),
            None => Vec::new(),
        };

        let open = pill("panel-open", &t, "Open", PillKind::Normal).on_click(|_, window, cx| {
            window.remove_window();
            crate::open_main(cx, crate::main_view::Page::Overview);
        });

        let footer: AnyElement = match &st {
            Some(st) => {
                let presets = mgr.with_settings(|s| s.presets.clone());
                let (mgr2, bus2, st2) = (mgr.clone(), bus.clone(), st.clone());
                let off = pill("panel-off", &t, "Turn off", PillKind::Danger).on_click(move |_, window, cx| {
                    let (mgr, bus, st) = (mgr2.clone(), bus2.clone(), st2.clone());
                    let msg = format!("Turn {} off?", mgr.label(&st));
                    confirm(window, cx, &msg, "You can wake it with its power button.", move || actions::power_async(&mgr, &bus, &st, false));
                });
                let preset_dd = (!presets.is_empty()).then(|| {
                    let items: Vec<DropItem> = presets.iter().enumerate().map(|(i, p)| DropItem { label: p.name.clone().into(), code: Some(i as u16), selected: false }).collect();
                    let (mgr, ps) = (mgr.clone(), presets.clone());
                    let on_select: crate::ui::CodeFn = std::rc::Rc::new(move |i, _, _| actions::apply_preset_async(&mgr, ps[i as usize].clone()));
                    Dropdown::new("panel-presets", &t, "Presets", items, on_select)
                });
                div().flex().flex_row().items_center().justify_between().children(preset_dd).child(off).into_any_element()
            }
            None => div().into_any_element(),
        };

        div()
            .size_full()
            .relative()
            .rounded(px(16.))
            .bg(t.bg)
            .border_1()
            .border_color(t.border)
            .overflow_hidden()
            .text_color(t.text)
            .text_size(px(13.))
            .px(px(16.))
            .pt(px(14.))
            .pb(px(12.))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(10.))
                            .child(img(ImageSource::from(logo)).size(px(30.)))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).child("ProArt"))
                                    .child(div().text_size(px(11.5)).text_color(t.dim).child(SharedString::from(label))),
                            ),
                    )
                    .child(open),
            )
            .child(div().mt(px(10.)).mb(px(4.)).child(spectrum_bar(px(PANEL_W - 34.))))
            .child(div().flex().flex_col().flex_1().when(probing && rows.is_empty(), |d| d.child(div().py(px(12.)).text_color(t.dim).child("Reading the monitor\u{2026}"))).children(rows))
            .child(footer)
            .child(toast_stack(&bus, &t))
    }
}

pub fn origin_below(cx: &App, x_center: f32, y_top: f32, y_bottom: f32, rows: usize) -> gpui::Point<Pixels> {
    let (dw, dh) = cx.primary_display().map(|d| (f32::from(d.bounds().size.width), f32::from(d.bounds().size.height))).unwrap_or((1440.0, 900.0));
    let h = height_for(rows);
    let x = (x_center - PANEL_W / 2.0).clamp(8.0, (dw - PANEL_W - 8.0).max(8.0));
    // taskbar at the bottom (Windows/Linux): open upwards
    let y = if y_top < dh / 2.0 { y_bottom + 4.0 } else { y_top - h - 6.0 };
    point(px(x), px(y))
}

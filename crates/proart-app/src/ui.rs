//! Widgets GPUI does not ship: cards, rows, pills, switch, slider, dropdown.

use std::rc::Rc;

use gpui::{
    anchored, canvas, deferred, div, hsla, linear_color_stop, linear_gradient, point, prelude::*, px, relative, Bounds,
    Corner, DispatchPhase, ElementId, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    SharedString, Stateful, Window, App, Div, Rgba, FontWeight,
};

use crate::theme::{spectrum, Theme};

pub fn card(t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .bg(t.card)
        .border_1()
        .border_color(t.border)
        .rounded(px(12.))
        .px(px(16.))
        .py(px(4.))
}

pub fn hairline(t: &Theme) -> Div {
    div().h(px(1.)).w_full().bg(t.border)
}

/// A settings row: title (+ hint) on the left, the control on the right.
pub fn row(t: &Theme, title: &str, hint: &str, control: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .py(px(9.))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.))
                .child(div().text_size(px(14.)).text_color(t.text).child(title.to_string()))
                .when(!hint.is_empty(), |d| {
                    d.child(div().text_size(px(11.5)).text_color(t.dim).child(hint.to_string()))
                }),
        )
        .child(div().flex().flex_row().items_center().gap(px(10.)).flex_none().child(control))
}

/// Cards hold rows separated by hairlines.
pub fn separated(t: &Theme, rows: Vec<gpui::AnyElement>) -> Div {
    let mut c = card(t);
    let n = rows.len();
    for (i, r) in rows.into_iter().enumerate() {
        c = c.child(r);
        if i + 1 < n {
            c = c.child(hairline(t));
        }
    }
    c
}

pub fn section_title(t: &Theme, text: &str) -> Div {
    div()
        .mt(px(14.))
        .mb(px(6.))
        .text_size(px(11.))
        .font_weight(FontWeight::BOLD)
        .text_color(t.dim)
        .child(text.to_uppercase())
}

/// Seven gradient segments across the spectrum: the brand line.
pub fn spectrum_bar(width: Pixels) -> Div {
    let n = 7;
    let mut bar = div().flex().flex_row().w(width).h(px(2.)).rounded_full().overflow_hidden();
    for i in 0..n {
        let a = 200.0 + 300.0 * i as f32 / n as f32;
        let b = 200.0 + 300.0 * (i + 1) as f32 / n as f32;
        bar = bar.child(div().flex_1().h_full().bg(linear_gradient(90., linear_color_stop(spectrum(a), 0.), linear_color_stop(spectrum(b), 1.))));
    }
    bar
}

pub fn page_title(t: &Theme, title: &str, subtitle: &str, color: Hsla) -> Div {
    div()
        .flex()
        .flex_col()
        .mb(px(16.))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.))
                .child(div().size(px(10.)).rounded_full().bg(color))
                .child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).text_color(t.text).child(title.to_string())),
        )
        .when(!subtitle.is_empty(), |d| d.mt(px(2.)).child(div().text_size(px(13.)).text_color(t.dim).child(subtitle.to_string())))
        .child(div().mt(px(10.)).child(spectrum_bar(px(560.))))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PillKind {
    Normal,
    Primary,
    Danger,
}

/// Pill-shaped button. The caller attaches `.on_click(..)`.
pub fn pill(id: impl Into<ElementId>, t: &Theme, text: &str, kind: PillKind) -> Stateful<Div> {
    let (bg, fg, border): (Rgba, Rgba, Rgba) = match kind {
        PillKind::Primary => (t.accent, gpui::rgb(0xFFFFFF), t.accent),
        PillKind::Danger => (gpui::rgba(0x00000000), t.danger, t.danger),
        PillKind::Normal => (t.field, t.text, t.border),
    };
    let hover = if kind == PillKind::Primary { t.accent } else { t.hover };
    div()
        .id(id)
        .px(px(14.))
        .h(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .text_size(px(13.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover).opacity(0.92))
        .active(|s| s.opacity(0.8))
        .child(text.to_string())
}

/// On/off switch. The caller attaches `.on_click(..)`.
pub fn switch(id: impl Into<ElementId>, t: &Theme, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(40.))
        .h(px(22.))
        .rounded_full()
        .cursor_pointer()
        .bg(if on { t.accent } else { t.hover })
        .relative()
        .child(
            div()
                .absolute()
                .top(px(2.))
                .left(if on { px(20.) } else { px(2.) })
                .size(px(18.))
                .rounded_full()
                .bg(gpui::rgb(0xFFFFFF)),
        )
}

// ------------------------------------------------------------------ slider --

struct DragState {
    dragging: bool,
    bounds: Bounds<Pixels>,
}

fn frac_at(x: Pixels, b: &Bounds<Pixels>) -> f32 {
    let w: f32 = b.size.width.into();
    if w <= 0.0 {
        return 0.0;
    }
    ((f32::from(x) - f32::from(b.origin.x)) / w).clamp(0.0, 1.0)
}

pub type FracFn = Rc<dyn Fn(f32, &mut Window, &mut App)>;

/// Horizontal slider. `on_change` fires while dragging, `on_commit` on release.
#[derive(IntoElement)]
pub struct Slider {
    id: ElementId,
    t: Theme,
    frac: f32,
    enabled: bool,
    width: Pixels,
    on_change: FracFn,
    on_commit: FracFn,
}

impl Slider {
    pub fn new(id: impl Into<ElementId>, t: &Theme, frac: f32, enabled: bool, on_change: FracFn, on_commit: FracFn) -> Slider {
        Slider { id: id.into(), t: *t, frac: frac.clamp(0.0, 1.0), enabled, width: px(200.), on_change, on_commit }
    }
}

impl RenderOnce for Slider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| DragState { dragging: false, bounds: Bounds::default() });
        let (t, frac, enabled) = (self.t, self.frac, self.enabled);
        let (h, knob, track) = (px(24.), px(16.), px(5.));
        let top = (h - track) / 2.0;
        let (on_change, on_commit) = (self.on_change.clone(), self.on_commit.clone());

        let (down_state, down_change) = (state.clone(), on_change.clone());
        let (paint_state, prepaint_state) = (state.clone(), state.clone());
        let (move_change, up_commit) = (on_change, on_commit);

        let root = div().id(self.id.clone());
        #[cfg(test)]
        let root = root.debug_selector(|| format!("slider:{:?}", self.id));
        root
            .relative()
            .w(self.width)
            .h(h)
            .when(enabled, |d| d.cursor_pointer())
            .when(!enabled, |d| d.opacity(0.45))
            .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, window, cx| {
                if !enabled {
                    return;
                }
                let f = down_state.update(cx, |s, _| {
                    s.dragging = true;
                    frac_at(ev.position.x, &s.bounds)
                });
                down_change(f, window, cx);
            })
            // the usable travel is inset by half a knob on both sides
            .child(
                div()
                    .absolute()
                    .left(knob / 2.0)
                    .right(knob / 2.0)
                    .top(px(0.))
                    .h(h)
                    .child(
                        canvas(
                            move |bounds, _window, cx| {
                                prepaint_state.update(cx, |s, _| s.bounds = bounds);
                            },
                            move |_bounds, _, window, _cx| {
                                let (s1, c1) = (paint_state.clone(), move_change.clone());
                                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                                    if phase == DispatchPhase::Capture {
                                        return;
                                    }
                                    let (dragging, bounds) = {
                                        let s = s1.read(cx);
                                        (s.dragging, s.bounds)
                                    };
                                    if dragging {
                                        c1(frac_at(ev.position.x, &bounds), window, cx);
                                    }
                                });
                                let (s2, c2) = (paint_state.clone(), up_commit.clone());
                                window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                                    if phase == DispatchPhase::Capture || ev.button != MouseButton::Left {
                                        return;
                                    }
                                    let (was, bounds) = s2.update(cx, |s, _| {
                                        let was = s.dragging;
                                        s.dragging = false;
                                        (was, s.bounds)
                                    });
                                    if was {
                                        c2(frac_at(ev.position.x, &bounds), window, cx);
                                    }
                                });
                            },
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(div().absolute().left(px(-8.)).right(px(-8.)).top(top).h(track).rounded_full().bg(t.field))
                    .child(div().absolute().left(px(-8.)).top(top).h(track).w(relative(frac)).rounded_full().bg(t.accent))
                    .child(
                        div()
                            .absolute()
                            .left(relative(frac))
                            .ml(-(knob / 2.0))
                            .top((h - knob) / 2.0)
                            .size(knob)
                            .rounded_full()
                            .bg(gpui::rgb(0xFFFFFF))
                            .border_1()
                            .border_color(hsla(0., 0., 0., 0.25))
                            .shadow_sm(),
                    ),
            )
    }
}

// ---------------------------------------------------------------- dropdown --

#[derive(Clone)]
pub struct DropItem {
    pub label: SharedString,
    pub code: Option<u16>,
    pub selected: bool,
}

pub type CodeFn = Rc<dyn Fn(u16, &mut Window, &mut App)>;

/// Pop-up button listing choices; unlearned entries are shown disabled.
#[derive(IntoElement)]
pub struct Dropdown {
    id: ElementId,
    t: Theme,
    label: SharedString,
    items: Vec<DropItem>,
    on_select: CodeFn,
    width: Pixels,
}

impl Dropdown {
    pub fn new(id: impl Into<ElementId>, t: &Theme, label: impl Into<SharedString>, items: Vec<DropItem>, on_select: CodeFn) -> Dropdown {
        Dropdown { id: id.into(), t: *t, label: label.into(), items, on_select, width: px(210.) }
    }
}

impl RenderOnce for Dropdown {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let open = window.use_keyed_state(self.id.clone(), cx, |_, _| false);
        let is_open = *open.read(cx);
        let (t, width) = (self.t, self.width);

        let toggle = open.clone();
        let button = div().id(SharedString::from(format!("{:?}-btn", self.id)));
        #[cfg(test)]
        let button = button.debug_selector(|| format!("dd-btn:{:?}", self.id));
        let button = button
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w(width)
            .h(px(30.))
            .px(px(12.))
            .rounded(px(8.))
            .border_1()
            .border_color(if is_open { t.accent } else { t.border })
            .bg(t.field)
            .cursor_pointer()
            .text_size(px(13.))
            .text_color(t.text)
            .hover(move |s| s.bg(t.hover))
            .on_click(move |_, _, cx| toggle.update(cx, |s, cx| {
                *s = !*s;
                cx.notify();
            }))
            .child(div().overflow_hidden().child(self.label.clone()))
            .child(div().text_color(t.dim).text_size(px(12.)).child("\u{2304}"));

        let menu = is_open.then(|| {
            let (close, select) = (open.clone(), self.on_select.clone());
            let items: Vec<gpui::AnyElement> = self
                .items
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    let (close, select, code) = (close.clone(), select.clone(), item.code);
                    let base = div().id(SharedString::from(format!("{:?}-item-{i}", self.id)));
                    #[cfg(test)]
                    let base = base.debug_selector(|| format!("dd-item:{i}"));
                    let base = base
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .px(px(10.))
                        .h(px(28.))
                        .rounded(px(6.))
                        .text_size(px(13.))
                        .child(item.label.clone())
                        .when(item.selected, |d| d.child(div().text_color(t.accent).child("\u{2713}")));
                    match code {
                        Some(c) => base
                            .text_color(t.text)
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.hover))
                            .on_click(move |_, window, cx| {
                                close.update(cx, |s, cx| {
                                    *s = false;
                                    cx.notify();
                                });
                                select(c, window, cx);
                            })
                            .into_any_element(),
                        None => base.text_color(t.faint).into_any_element(),
                    }
                })
                .collect();
            let close_out = open.clone();
            deferred(
                anchored().anchor(Corner::TopRight).position(point(width, px(4.))).position_mode(gpui::AnchoredPositionMode::Local).snap_to_window_with_margin(px(8.)).child(
                    div()
                        .id(SharedString::from(format!("{:?}-menu", self.id)))
                        .occlude()
                        .w(width)
                        .max_h(px(320.))
                        .overflow_y_scroll()
                        .p(px(4.))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(t.border)
                        .bg(t.card)
                        .shadow_lg()
                        .on_mouse_down_out(move |_, _, cx| {
                            close_out.update(cx, |s, cx| {
                                *s = false;
                                cx.notify();
                            })
                        })
                        .children(items),
                ),
            )
            .with_priority(10)
        });

        div().relative().w(width).h(px(30.)).child(button).children(menu)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use gpui::{point, Context, Modifiers, MouseButton, Render, TestAppContext};

    use super::*;

    struct Root {
        frac: Rc<Cell<f32>>,
        commits: Rc<Cell<u32>>,
    }

    impl Render for Root {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let (f, c) = (self.frac.clone(), self.commits.clone());
            div().size_full().p(px(20.)).child(Slider::new(
                "slider",
                &Theme::dark(),
                0.0,
                true,
                Rc::new(move |fr, _, _| f.set(fr)),
                Rc::new(move |_, _, _| c.set(c.get() + 1)),
            ))
        }
    }

    use crate::theme::Theme;

    #[test]
    fn frac_at_clamps_to_the_track() {
        let b = Bounds { origin: point(px(28.), px(0.)), size: gpui::size(px(184.), px(24.)) };
        assert_eq!(frac_at(px(28.), &b), 0.0);
        assert!((frac_at(px(120.), &b) - 0.5).abs() < 1e-6);
        assert_eq!(frac_at(px(500.), &b), 1.0);
        assert_eq!(frac_at(px(-5.), &b), 0.0);
    }

    #[gpui::test]
    fn dragging_the_slider_reports_fractions_and_commits_on_release(cx: &mut TestAppContext) {
        let frac = Rc::new(Cell::new(-1.0));
        let commits = Rc::new(Cell::new(0));
        let (f2, c2) = (frac.clone(), commits.clone());
        let (_root, cx) = cx.add_window_view(move |_, _| Root { frac: f2, commits: c2 });
        cx.run_until_parked();

        // track spans x = 28..212 (20px padding + half a knob inset), y ~ 32
        let y = px(32.);
        cx.simulate_mouse_down(point(px(120.), y), MouseButton::Left, Modifiers::none());
        assert!((frac.get() - 0.5).abs() < 0.02, "press at the middle: {}", frac.get());
        cx.simulate_mouse_move(point(px(204.), y), Some(MouseButton::Left), Modifiers::none());
        assert!(frac.get() > 0.9, "dragged right: {}", frac.get());
        // dragging outside the control keeps tracking and clamps
        cx.simulate_mouse_move(point(px(400.), y), Some(MouseButton::Left), Modifiers::none());
        assert_eq!(frac.get(), 1.0);
        assert_eq!(commits.get(), 0, "not committed while dragging");
        cx.simulate_mouse_up(point(px(400.), y), MouseButton::Left, Modifiers::none());
        assert_eq!(commits.get(), 1, "committed once on release");
        // moving after release changes nothing
        cx.simulate_mouse_move(point(px(60.), y), None, Modifiers::none());
        assert_eq!(frac.get(), 1.0);
    }
}

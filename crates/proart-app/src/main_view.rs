//! The main window: sidebar + one page per OSD section, plus app pages.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    div, img, point, prelude::*, px, rgba, size, AnyElement, App, Bounds, Context, Entity, FontWeight, Hsla, ImageSource,
    IntoElement, Render, SharedString, Subscription, Task, TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use proart_core::features::{self, Group};
use proart_core::manager::{DisplayState, Manager, Status};

use crate::actions;
use crate::controls::{confirm, feature_row, Ctl};
use crate::learn::{LearnState, Snapshot};
use crate::state::{Bus, Shared, ToastKind};
use crate::text_field::{FieldEvent, TextField};
use crate::theme::{group_color, Theme};
use crate::ui::{card, page_title, pill, row, section_title, separated, switch, DropItem, Dropdown, PillKind};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Overview,
    Group(Group),
    Info,
    Presets,
    Displays,
    Settings,
    About,
}

impl Page {
    pub fn parse(text: &str) -> Option<Page> {
        match text.to_lowercase().as_str() {
            "overview" => Some(Page::Overview),
            "info" | "information" => Some(Page::Info),
            "presets" => Some(Page::Presets),
            "displays" => Some(Page::Displays),
            "settings" => Some(Page::Settings),
            "about" => Some(Page::About),
            other => Group::parse(other).map(Page::Group),
        }
    }
}

pub struct MainView {
    page: Page,
    display_idx: usize,
    learn: Option<LearnState>,
    preset_name: Entity<TextField>,
    name_fields: HashMap<String, Entity<TextField>>,
    login: Option<bool>,
    seen: u64,
    had_toasts: bool,
    live_key: String,
    subs: Vec<Subscription>,
    _tick: Task<()>,
}

pub fn window_options(cx: &mut App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(960.), px(660.)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("ProArt".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(18.), px(18.))),
        }),
        window_min_size: Some(size(px(820.), px(540.))),
        app_id: Some("proart".into()),
        ..Default::default()
    }
}

impl MainView {
    pub fn new(page: Page, window: &mut Window, cx: &mut Context<Self>) -> MainView {
        let theme = current_theme(window, cx);
        let preset_name = cx.new(|cx| TextField::new(cx, "e.g. Photo editing", theme));
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
        MainView {
            page,
            display_idx: 0,
            learn: None,
            preset_name,
            name_fields: HashMap::new(),
            login: None,
            seen: 0,
            had_toasts: false,
            live_key: String::new(),
            subs: Vec::new(),
            _tick: tick,
        }
    }

    fn current_display(&mut self, mgr: &Manager) -> Option<Arc<DisplayState>> {
        let list = mgr.displays();
        if list.is_empty() {
            return None;
        }
        if self.display_idx >= list.len() {
            self.display_idx = 0;
        }
        Some(list[self.display_idx].clone())
    }

    /// Keep values in sync with the monitor while this window is open.
    fn sync_live(&mut self, mgr: &Manager) {
        let key = format!("{:?}", self.page);
        if key == self.live_key {
            return;
        }
        self.live_key = key;
        let mut ids: Vec<&str> = mgr.with_settings(|s| s.ui.quick_features.clone()).iter().filter_map(|id| features::get(id).map(|f| f.id)).collect();
        if let Page::Group(g) = self.page {
            ids.extend(features::in_group(g).filter(|f| f.kind != features::Kind::Action).map(|f| f.id));
        }
        mgr.set_live_features(&ids);
        let live = mgr.with_settings(|s| s.global.live_values);
        mgr.set_live(live);
    }

    fn nav_item(&self, idx: usize, page: Page, label: &str, color: Hsla, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.page == page;
        let (hover, sel) = (t.field, t.hover);
        let item = div().id(("nav", idx));
        #[cfg(test)]
        let item = item.debug_selector(|| format!("nav:{label}"));
        item.flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .h(px(30.))
            .px(px(10.))
            .rounded(px(8.))
            .cursor_pointer()
            .text_size(px(13.5))
            .text_color(if selected { t.text } else { t.dim })
            .when(selected, move |d| d.bg(sel))
            .when(!selected, move |d| d.hover(move |s| s.bg(hover)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.page = page;
                cx.notify();
            }))
            .child(div().size(px(9.)).rounded_full().bg(color))
            .child(label.to_string())
            .into_any_element()
    }

    fn sidebar(&mut self, st: &Option<Arc<DisplayState>>, t: &Theme, mgr: &Arc<Manager>, logo: Arc<gpui::Image>, cx: &mut Context<Self>) -> AnyElement {
        let displays = mgr.displays();
        let weak = cx.weak_entity();
        let picker: AnyElement = match displays.len() {
            0 => div().text_size(px(12.)).text_color(t.dim).child("No displays detected").into_any_element(),
            1 => div()
                .flex()
                .flex_col()
                .child(div().text_size(px(13.)).font_weight(FontWeight::BOLD).child(mgr.label(&displays[0])))
                .child(div().text_size(px(11.)).text_color(t.dim).child(format!("{}\u{00d7}{}", displays[0].info.width, displays[0].info.height)))
                .into_any_element(),
            _ => {
                let items: Vec<DropItem> = displays
                    .iter()
                    .enumerate()
                    .map(|(i, d)| DropItem { label: mgr.label(d).into(), code: Some(i as u16), selected: i == self.display_idx })
                    .collect();
                let label = st.as_ref().map(|s| mgr.label(s)).unwrap_or_default();
                let on_select: crate::ui::CodeFn = Rc::new(move |i, _, cx| {
                    weak.update(cx, |this, cx| {
                        this.display_idx = i as usize;
                        cx.notify();
                    })
                    .ok();
                });
                Dropdown::new("display-picker", t, label, items, on_select).into_any_element()
            }
        };

        let mut idx = 0;
        let mut next = |page: Page, label: &str, color: Hsla, this: &Self, cx: &mut Context<Self>| {
            idx += 1;
            this.nav_item(idx, page, label, color, t, cx)
        };
        let overview = next(Page::Overview, "Overview", t.accent.into(), self, cx);
        let groups: Vec<AnyElement> = Group::ALL.iter().map(|g| next(Page::Group(*g), g.label(), group_color(*g), self, cx)).collect();
        let info = next(Page::Info, "Information", t.dim.into(), self, cx);
        let app_pages: Vec<AnyElement> = [(Page::Presets, "Presets"), (Page::Displays, "Displays"), (Page::Settings, "Settings"), (Page::About, "About")]
            .iter()
            .map(|(p, l)| next(*p, l, t.dim.into(), self, cx))
            .collect();

        div()
            .id("sidebar")
            .flex()
            .flex_col()
            .w(px(228.))
            .h_full()
            .flex_none()
            .bg(t.sidebar)
            .border_r_1()
            .border_color(t.border)
            .px(px(14.))
            .pt(px(if cfg!(target_os = "macos") { 44. } else { 16. }))
            .pb(px(12.))
            .gap(px(2.))
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.))
                    .mb(px(14.))
                    .child(img(ImageSource::from(logo)).size(px(34.)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(18.)).font_weight(FontWeight::BOLD).child("ProArt"))
                            .child(div().text_size(px(11.)).text_color(t.faint).child(format!("v{}", proart_core::VERSION))),
                    ),
            )
            .child(div().mb(px(12.)).child(picker))
            .child(overview)
            .child(section_title(t, "Monitor menu").mt(px(10.)))
            .children(groups)
            .child(info)
            .child(section_title(t, "App").mt(px(10.)))
            .children(app_pages)
            .into_any_element()
    }
}

fn current_theme(window: &Window, cx: &App) -> Theme {
    let setting = cx.global::<Shared>().mgr.with_settings(|s| s.ui.theme.clone());
    Theme::resolve(&setting, window.appearance())
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (mgr, bus, logo) = {
            let s = cx.global::<Shared>();
            (s.mgr.clone(), s.bus.clone(), s.logo.clone())
        };
        let t = current_theme(window, cx);
        self.sync_live(&mgr);
        let st = self.current_display(&mgr);
        if let Some(st) = &st {
            bus.ensure_probed(&mgr, st);
        }
        self.preset_name.update(cx, |f, _| f.theme = t);

        let weak = cx.weak_entity();
        let (b2, m2) = (bus.clone(), mgr.clone());
        let on_learn: crate::controls::LearnFn = Rc::new(move |st, f, _w, cx| {
            let state = LearnState::start(m2.clone(), b2.clone(), st, f);
            weak.update(cx, |this, cx| {
                this.learn = Some(state);
                cx.notify();
            })
            .ok();
        });
        let ctl = Ctl { mgr: mgr.clone(), bus: bus.clone(), t, on_learn: Some(on_learn) };

        let sidebar = self.sidebar(&st, &t, &mgr, logo.clone(), cx);
        let body = self.page_body(&ctl, &st, logo, window, cx);
        let learn = self.learn.clone().map(|l| self.learn_dialog(&l, &t, cx));
        let toasts = toast_stack(&bus, &t);

        div()
            .size_full()
            .relative()
            .flex()
            .flex_row()
            .bg(t.bg)
            .text_color(t.text)
            .text_size(px(13.))
            .child(sidebar)
            .child(
                div()
                    .id("content")
                    .flex_1()
                    .h_full()
                    .overflow_y_scroll()
                    .child(div().px(px(30.)).pt(px(40.)).pb(px(48.)).max_w(px(720.)).child(body)),
            )
            .children(learn)
            .child(toasts)
    }
}

pub fn toast_stack(bus: &Arc<Bus>, t: &Theme) -> AnyElement {
    let toasts = bus.toasts();
    div()
        .absolute()
        .bottom(px(18.))
        .left_0()
        .right_0()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.))
        .children(toasts.into_iter().map(|toast| {
            let accent = match toast.kind {
                ToastKind::Warn => t.warn,
                ToastKind::Error => t.danger,
            };
            div()
                .max_w(px(520.))
                .px(px(14.))
                .py(px(9.))
                .rounded(px(12.))
                .bg(t.card)
                .border_1()
                .border_color(accent)
                .text_size(px(13.))
                .text_color(t.text)
                .shadow_lg()
                .child(toast.text)
        }))
        .into_any_element()
}

// ---------------------------------------------------------------- pages --

impl MainView {
    fn page_body(&mut self, ctl: &Ctl, st: &Option<Arc<DisplayState>>, logo: Arc<gpui::Image>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = &ctl.t;
        let Some(stv) = st else {
            return match self.page {
                Page::Presets | Page::Settings | Page::About | Page::Displays => self.app_page(ctl, None, logo, window, cx),
                _ => div()
                    .child(page_title(t, "ProArt", "", t.accent.into()))
                    .child(
                        card(t).p(px(18.)).child(div().text_size(px(15.)).font_weight(FontWeight::BOLD).child("No display detected")).child(
                            div().mt(px(4.)).text_color(t.dim).child("Connect an ASUS ProArt monitor over DisplayPort or HDMI and enable DDC/CI in its OSD menu."),
                        ),
                    )
                    .into_any_element(),
            };
        };
        match self.page {
            Page::Overview => self.page_overview(ctl, stv, cx),
            Page::Group(g) => self.page_group(ctl, stv, g),
            Page::Info => self.page_info(ctl, stv),
            _ => self.app_page(ctl, Some(stv), logo, window, cx),
        }
    }

    fn app_page(&mut self, ctl: &Ctl, st: Option<&Arc<DisplayState>>, logo: Arc<gpui::Image>, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let _ = st;
        match self.page {
            Page::Presets => self.page_presets(ctl, cx),
            Page::Displays => self.page_displays(ctl, cx),
            Page::Settings => self.page_settings(ctl, cx),
            _ => self.page_about(ctl, logo, cx),
        }
    }

    fn page_overview(&mut self, ctl: &Ctl, st: &Arc<DisplayState>, cx: &mut Context<Self>) -> AnyElement {
        let t = &ctl.t;
        let wanted = ctl.mgr.with_settings(|s| s.ui.quick_features.clone());
        let rows: Vec<AnyElement> = features::all()
            .iter()
            .filter(|f| wanted.iter().any(|w| w == f.id) && ctl.mgr.visible(st, f) && ctl.mgr.status(st, f) == Status::Ok)
            .map(|f| feature_row(ctl, st, f))
            .collect();
        let probing = ctl.bus.is_probing(st.key());
        let (mgr, bus, s1, s2) = (ctl.mgr.clone(), ctl.bus.clone(), st.clone(), st.clone());
        let (mgr2, bus2) = (mgr.clone(), bus.clone());
        let off = pill("power-off", t, "Turn off", PillKind::Danger).on_click(cx.listener(move |_, _, window, cx| {
            let (mgr, bus, st) = (mgr.clone(), bus.clone(), s1.clone());
            let msg = format!("Turn {} off?", mgr.label(&st));
            confirm(window, cx, &msg, "You can wake it with its power button.", move || actions::power_async(&mgr, &bus, &st, false));
        }));
        let on = pill("power-on", t, "Turn on", PillKind::Normal).on_click(move |_, _, _| actions::power_async(&mgr2, &bus2, &s2, true));
        div()
            .child(page_title(t, "Overview", &format!("{}  \u{00b7}  {}\u{00d7}{}", ctl.mgr.label(st), st.info.width, st.info.height), t.accent.into()))
            .when(probing, |d| d.child(div().mb(px(8.)).text_color(t.dim).child("Reading the monitor\u{2026}")))
            .child(section_title(t, "Quick controls"))
            .child(separated(t, rows))
            .child(section_title(t, "Display power"))
            .child(card(t).child(row(t, "Power", "Turn the monitor off, or wake it", div().flex().flex_row().gap(px(8.)).child(on).child(off))))
            .into_any_element()
    }

    fn page_group(&mut self, ctl: &Ctl, st: &Arc<DisplayState>, g: Group) -> AnyElement {
        let t = &ctl.t;
        let blurb = match g {
            Group::Preset => "Picture mode of the monitor. Switching it can reset brightness and contrast.",
            Group::Blue => "Reduce blue light for comfortable reading.",
            Group::Palette => "Brightness, contrast and colour. Some controls lock in colour-managed modes such as sRGB.",
            Group::Image => "Sharpness and signal handling.",
            Group::Sound => "Speaker volume and mute.",
            Group::Input => "Switch the video input.",
            Group::System => "Monitor behaviour: language, key lock, power and OSD.",
        };
        let rows: Vec<AnyElement> = features::in_group(g).filter(|f| ctl.mgr.visible(st, f)).map(|f| feature_row(ctl, st, f)).collect();
        let probing = ctl.bus.is_probing(st.key());
        div()
            .child(page_title(t, g.label(), blurb, group_color(g)))
            .child(if rows.is_empty() {
                let msg = if probing {
                    "Reading the monitor\u{2026}".to_string()
                } else {
                    format!("{} does not expose {} over DDC/CI.", ctl.mgr.label(st), g.label())
                };
                card(t).py(px(16.)).child(div().text_color(t.dim).child(msg)).into_any_element()
            } else {
                separated(t, rows).into_any_element()
            })
            .into_any_element()
    }

    fn page_info(&mut self, ctl: &Ctl, st: &Arc<DisplayState>) -> AnyElement {
        let t = &ctl.t;
        let mut rows = vec![("Model".to_string(), if st.info.name.is_empty() { "-".into() } else { st.info.name.clone() })];
        if !st.info.serial.is_empty() {
            rows.push(("Serial".into(), st.info.serial.clone()));
        }
        if st.info.width > 0 {
            rows.push(("Resolution".into(), format!("{} \u{00d7} {}", st.info.width, st.info.height)));
        }
        rows.push(("Connection".into(), st.info.backend.clone()));
        rows.extend(ctl.bus.info_rows(st.key()));
        let rows: Vec<AnyElement> = rows.into_iter().map(|(k, v)| row(t, &k, "", div().text_size(px(13.5)).text_color(t.dim).child(v)).into_any_element()).collect();
        div().child(page_title(t, "Information", "Read from the monitor over DDC/CI", t.dim.into())).child(separated(t, rows)).into_any_element()
    }

    fn page_presets(&mut self, ctl: &Ctl, cx: &mut Context<Self>) -> AnyElement {
        let t = &ctl.t;
        let presets = ctl.mgr.with_settings(|s| s.presets.clone());
        let (mgr, field) = (ctl.mgr.clone(), self.preset_name.clone());
        let save = pill("preset-save", t, "Save", PillKind::Primary).on_click(cx.listener(move |this, _, _, cx| {
            let name = this.preset_name.read(cx).text();
            if name.trim().is_empty() {
                return;
            }
            let p = mgr.capture_preset(name.trim(), "star");
            mgr.with_settings(|s| s.presets.push(p));
            mgr.save_settings();
            field.update(cx, |f, cx| f.set_text("", cx));
            cx.notify();
        }));
        let rows: Vec<AnyElement> = presets
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let (mgr, pr) = (ctl.mgr.clone(), p.clone());
                let apply = pill(("preset-apply", i), t, "Apply", PillKind::Primary).on_click(move |_, _, _| actions::apply_preset_async(&mgr, pr.clone()));
                let (mgr, id) = (ctl.mgr.clone(), p.id.clone());
                let del = pill(("preset-del", i), t, "Delete", PillKind::Danger).on_click(cx.listener(move |_, _, _, cx| {
                    mgr.with_settings(|s| s.presets.retain(|x| x.id != id));
                    mgr.save_settings();
                    cx.notify();
                }));
                row(t, &p.name, &format!("{} display(s)", p.values.len()), div().flex().flex_row().gap(px(8.)).child(apply).child(del)).into_any_element()
            })
            .collect();
        div()
            .child(page_title(t, "Presets", "Snapshots of every setting, per display", t.accent.into()))
            .child(section_title(t, "Save the current settings"))
            .child(card(t).child(row(t, "Name", "", div().flex().flex_row().items_center().gap(px(8.)).child(self.preset_name.clone()).child(save))))
            .child(section_title(t, "Saved"))
            .child(if rows.is_empty() {
                card(t).py(px(14.)).child(div().text_color(t.dim).child("No presets yet.")).into_any_element()
            } else {
                separated(t, rows).into_any_element()
            })
            .into_any_element()
    }

    fn name_field(&mut self, st: &Arc<DisplayState>, ctl: &Ctl, cx: &mut Context<Self>) -> Entity<TextField> {
        let key = st.key().to_string();
        if let Some(f) = self.name_fields.get(&key) {
            f.update(cx, |f, _| f.theme = ctl.t);
            return f.clone();
        }
        let current = ctl.mgr.with_settings(|s| s.prefs(&key).name);
        let (t, placeholder) = (ctl.t, st.info.name.clone());
        let field = cx.new(|cx| {
            let mut f = TextField::new(cx, &placeholder, t);
            f.set_text(&current, cx);
            f
        });
        let (mgr, k) = (ctl.mgr.clone(), key.clone());
        self.subs.push(cx.subscribe(&field, move |_this, f, ev: &FieldEvent, cx| {
            if let FieldEvent::Changed = ev {
                let text = f.read(cx).text();
                mgr.with_settings(|s| s.display_prefs(&k).name = text);
                mgr.save_settings();
            }
        }));
        self.name_fields.insert(key, field.clone());
        field
    }

    fn page_displays(&mut self, ctl: &Ctl, cx: &mut Context<Self>) -> AnyElement {
        let t = &ctl.t;
        let displays = ctl.mgr.displays();
        let mut out = div().child(page_title(t, "Displays", "Names and what was learned from each monitor", t.accent.into()));
        if displays.is_empty() {
            return out.child(card(t).py(px(16.)).child(div().text_color(t.dim).child("No display detected."))).into_any_element();
        }
        for st in displays {
            let key = st.key().to_string();
            let prefs = ctl.mgr.with_settings(|s| s.prefs(&key));
            let name_field = self.name_field(&st, ctl, cx);
            let mut rows: Vec<AnyElement> = vec![row(t, "Name", "Shown in the app instead of the EDID name", name_field).into_any_element()];
            let (mgr, k) = (ctl.mgr.clone(), key.clone());
            let enabled = switch(SharedString::from(format!("en-{key}")), t, prefs.enabled).on_click(cx.listener(move |_, _, _, cx| {
                mgr.with_settings(|s| {
                    let p = s.display_prefs(&k);
                    p.enabled = !p.enabled;
                });
                mgr.save_settings();
                cx.notify();
            }));
            rows.push(row(t, "Enabled", "Disabled displays are ignored", enabled).into_any_element());
            for (id, learned) in &prefs.feature_map {
                let label = features::get(id).map(|f| f.label).unwrap_or("?");
                let (mgr, st2, id2) = (ctl.mgr.clone(), st.clone(), id.clone());
                let forget = pill(SharedString::from(format!("fg-{key}-{id}")), t, "Forget", PillKind::Danger).on_click(cx.listener(move |_, _, _, cx| {
                    if let Some(f) = features::get(&id2) {
                        mgr.forget(&st2, f);
                    }
                    cx.notify();
                }));
                rows.push(row(t, label, &format!("Learned: VCP 0x{:02X}, {} value(s)", learned.vcp, learned.values.len()), forget).into_any_element());
            }
            for id in &prefs.unsupported_features {
                let label = features::get(id).map(|f| f.label).unwrap_or("?");
                let (mgr, st2, id2) = (ctl.mgr.clone(), st.clone(), id.clone());
                let retry = pill(SharedString::from(format!("rt-{key}-{id}")), t, "Retry", PillKind::Normal).on_click(cx.listener(move |_, _, _, cx| {
                    if let Some(f) = features::get(&id2) {
                        mgr.forget(&st2, f);
                    }
                    cx.notify();
                }));
                rows.push(row(t, label, "Marked as not available on this display", retry).into_any_element());
            }
            out = out.child(section_title(t, &ctl.mgr.label(&st))).child(separated(t, rows));
        }
        out.into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn toggle_row(&self, ctl: &Ctl, id: &'static str, title: &str, hint: &str, on: bool, apply: fn(&mut proart_core::Settings), cx: &mut Context<Self>) -> AnyElement {
        let mgr = ctl.mgr.clone();
        let sw = switch(id, &ctl.t, on).on_click(cx.listener(move |this, _, _, cx| {
            mgr.with_settings(apply);
            mgr.save_settings();
            this.live_key.clear();
            cx.notify();
        }));
        row(&ctl.t, title, hint, sw).into_any_element()
    }

    fn page_settings(&mut self, ctl: &Ctl, cx: &mut Context<Self>) -> AnyElement {
        let t = &ctl.t;
        let g = ctl.mgr.with_settings(|s| s.global.clone());
        let theme = ctl.mgr.with_settings(|s| s.ui.theme.clone());
        let quick = ctl.mgr.with_settings(|s| s.ui.quick_features.clone());
        let login = *self.login.get_or_insert_with(proart_core::autostart::enabled);

        let weak = cx.weak_entity();
        let mgr = ctl.mgr.clone();
        let themes = [("auto", "Automatic"), ("light", "Light"), ("dark", "Dark")];
        let items: Vec<DropItem> = themes.iter().enumerate().map(|(i, (v, l))| DropItem { label: (*l).into(), code: Some(i as u16), selected: *v == theme }).collect();
        let label = themes.iter().find(|(v, _)| *v == theme).map(|(_, l)| *l).unwrap_or("Automatic");
        let on_theme: crate::ui::CodeFn = Rc::new(move |i, _, cx| {
            mgr.with_settings(|s| s.ui.theme = themes[i as usize].0.to_string());
            mgr.save_settings();
            weak.update(cx, |_, cx| cx.notify()).ok();
        });
        let theme_dd = Dropdown::new("theme-dd", t, label, items, on_theme);

        let (mgr, bus) = (ctl.mgr.clone(), ctl.bus.clone());
        let login_sw = switch("login", t, login).on_click(cx.listener(move |this, _, _, cx| {
            let want = !login;
            match std::env::current_exe().and_then(|exe| proart_core::autostart::set_enabled(want, &exe)) {
                Ok(()) => {
                    this.login = Some(want);
                    mgr.with_settings(|s| s.global.launch_at_login = want);
                    mgr.save_settings();
                }
                Err(e) => bus.toast(format!("Could not change launch at login: {e}"), ToastKind::Error),
            }
            cx.notify();
        }));

        let quick_rows: Vec<AnyElement> = features::all()
            .iter()
            .filter(|f| matches!(f.id, "preset" | "preset_calibrated" | "blue_light" | "brightness" | "contrast" | "volume" | "input" | "sharpness" | "color_temp" | "gamma"))
            .map(|f| {
                let on = quick.iter().any(|x| x == f.id);
                let mgr = ctl.mgr.clone();
                let sw = switch(SharedString::from(format!("q-{}", f.id)), t, on).on_click(cx.listener(move |this, _, _, cx| {
                    mgr.with_settings(|s| {
                        s.ui.quick_features.retain(|x| x != f.id);
                        if !on {
                            s.ui.quick_features.push(f.id.to_string());
                        }
                    });
                    mgr.save_settings();
                    this.live_key.clear();
                    cx.notify();
                }));
                row(t, f.label, "", sw).into_any_element()
            })
            .collect();

        div()
            .child(page_title(t, "Settings", "How ProArt looks and behaves", t.accent.into()))
            .child(section_title(t, "Appearance"))
            .child(card(t).child(row(t, "Theme", "Automatic follows the system", theme_dd)))
            .child(section_title(t, "Behaviour"))
            .child(separated(
                t,
                vec![
                    self.toggle_row(ctl, "live", "Live values", "Keep sliders in sync with the monitor's own buttons while open", g.live_values, |s| s.global.live_values = !s.global.live_values, cx),
                    self.toggle_row(ctl, "sync", "Mirror displays", "Brightness, contrast and volume follow across all displays", g.sync_all, |s| s.global.sync_all = !s.global.sync_all, cx),
                    self.toggle_row(ctl, "confirm", "Confirm risky actions", "Ask before switching input, power off or colour reset", g.confirm_dangerous, |s| s.global.confirm_dangerous = !s.global.confirm_dangerous, cx),
                    row(t, "Launch at login", "Start in the menu bar when you sign in", login_sw).into_any_element(),
                ],
            ))
            .child(section_title(t, "Quick controls"))
            .child(separated(t, quick_rows))
            .into_any_element()
    }

    fn page_about(&mut self, ctl: &Ctl, logo: Arc<gpui::Image>, cx: &mut Context<Self>) -> AnyElement {
        let t = &ctl.t;
        let path = proart_core::config::config_path();
        let reveal_path = path.clone();
        let reveal = pill("reveal", t, "Reveal", PillKind::Normal).on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&reveal_path)));
        div()
            .child(page_title(t, "About", "", t.dim.into()))
            .child(separated(
                t,
                vec![
                    row(t, "ProArt", &format!("Version {}", proart_core::VERSION), img(ImageSource::from(logo)).size(px(44.))).into_any_element(),
                    row(t, "Config", &path.display().to_string(), reveal).into_any_element(),
                    row(t, "Protocol", "DDC/CI: IOAVService (macOS), dxva2 (Windows), i2c-dev (Linux)", div()).into_any_element(),
                ],
            ))
            .into_any_element()
    }

    // ---------------------------------------------------------- learn dialog --
    fn learn_dialog(&self, l: &LearnState, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let snap = l.snapshot();
        let close = pill("learn-close", t, "Cancel", PillKind::Normal).on_click(cx.listener(|this, _, _, cx| {
            this.learn = None;
            cx.notify();
        }));
        let title = format!("Learn: {}", l.f.label);
        let mut body = div().flex().flex_col().gap(px(8.));
        let mut footer = div().flex().flex_row().gap(px(8.)).mt(px(14.));
        match snap {
            Snapshot::Scanning { done, total } => {
                body = body
                    .child(div().text_size(px(14.)).font_weight(FontWeight::BOLD).child("Scanning the monitor\u{2026}"))
                    .child(div().text_color(t.dim).child("Don't touch the monitor's buttons yet. This reads every register once (a couple of minutes)."))
                    .child(
                        div().mt(px(6.)).w_full().h(px(6.)).rounded_full().bg(t.field).child(
                            div().h_full().w(gpui::relative(done as f32 / total.max(1) as f32)).rounded_full().bg(t.accent),
                        ),
                    );
                footer = footer.child(close);
            }
            Snapshot::Step { index, total, name, busy, notes } => {
                let l2 = l.clone();
                let l3 = l.clone();
                body = body
                    .child(div().text_size(px(12.)).text_color(t.dim).child(format!("Step {} of {total}", index + 1)))
                    .child(div().text_size(px(14.5)).child(format!("On the monitor's own menu, set {} to \u{201c}{name}\u{201d}.", l.f.label)))
                    .child(div().text_color(t.dim).child("Wait about 5 seconds for it to apply, then press the button below. If your monitor has no such option, press Skip."))
                    .children(notes.into_iter().map(|n| div().text_size(px(12.)).text_color(t.warn).child(n)));
                if busy {
                    footer = footer.child(div().text_color(t.dim).child("Reading the monitor\u{2026}"));
                } else {
                    footer = footer
                        .child(pill("learn-set", t, "I've set it", PillKind::Primary).on_click(move |_, _, _| l2.advance(true)))
                        .child(pill("learn-skip", t, "Skip", PillKind::Normal).on_click(move |_, _, _| l3.advance(false)));
                }
                footer = footer.child(close);
            }
            Snapshot::Choose(list) => {
                body = body.child(div().text_size(px(14.)).child("Several registers matched. Pick one:"));
                for code in list {
                    let l2 = l.clone();
                    footer = footer.child(pill(("learn-pick", code as usize), t, &format!("0x{code:02X}"), PillKind::Normal).on_click(move |_, _, _| l2.choose(code)));
                }
                footer = footer.child(close);
            }
            Snapshot::Done(msg) => {
                body = body.child(div().text_size(px(14.)).child(msg));
                footer = footer.child(pill("learn-done", t, "Close", PillKind::Primary).on_click(cx.listener(|this, _, _, cx| {
                    this.learn = None;
                    cx.notify();
                })));
            }
        }
        div()
            .id("learn-overlay")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a8))
            .occlude()
            .child(
                div()
                    .w(px(460.))
                    .p(px(20.))
                    .rounded(px(14.))
                    .bg(t.card)
                    .border_1()
                    .border_color(t.border)
                    .shadow_lg()
                    .child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).mb(px(10.)).child(title))
                    .child(body)
                    .child(footer),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use gpui::{Image, ImageFormat, Modifiers, MouseButton, TestAppContext, VisualTestContext};
    use proart_core::backend::fake::FakeBackend;
    use proart_core::backend::{Backend, DisplayInfo, Session};
    use proart_core::Settings;

    use super::*;
    use crate::state::Bus;

    struct Shared2(Arc<FakeBackend>);
    impl Backend for Shared2 {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn list_displays(&self) -> Vec<DisplayInfo> {
            self.0.list_displays()
        }
        fn open_session(&self, i: &DisplayInfo) -> Option<Box<dyn Session>> {
            self.0.open_session(i)
        }
    }

    /// A MainView over a fake PA278QV, with the app globals installed.
    fn setup(cx: &mut TestAppContext, page: Page) -> (Arc<FakeBackend>, Arc<Manager>, Entity<MainView>, &mut VisualTestContext) {
        let fake = Arc::new(FakeBackend::sample());
        fake.rename(0, "PA278QV");
        fake.set_register(0xDC, 13, 35);
        fake.set_register(0x14, 5, 12);
        let mgr = Manager::new(Box::new(Shared2(fake.clone())), Settings::default(), Arc::new(|_| {}));
        mgr.refresh();
        mgr.start();
        let st = mgr.displays()[0].clone();
        mgr.probe(&st);
        mgr.read(&st, features::get("brightness").unwrap());
        let (m2, bus) = (mgr.clone(), Arc::new(Bus::default()));
        cx.update(|cx| {
            cx.set_global(Shared { mgr: m2, bus, logo: Arc::new(Image::from_bytes(ImageFormat::Png, Vec::new())), tray: false });
            crate::text_field::bind_keys(cx);
        });
        let (view, vcx) = cx.add_window_view(move |window, cx| MainView::new(page, window, cx));
        vcx.run_until_parked();
        (fake, mgr, view, vcx)
    }

    fn center(b: Bounds<gpui::Pixels>) -> gpui::Point<gpui::Pixels> {
        point(b.origin.x + b.size.width / 2.0, b.origin.y + b.size.height / 2.0)
    }

    #[gpui::test]
    fn clicking_the_sidebar_changes_page(cx: &mut TestAppContext) {
        let (_fake, _mgr, view, cx) = setup(cx, Page::Overview);
        let b = cx.debug_bounds("nav:ProArt Palette").expect("nav item is drawn");
        cx.simulate_click(center(b), Modifiers::none());
        assert_eq!(view.read_with(cx, |v, _| v.page), Page::Group(Group::Palette));
    }

    #[gpui::test]
    fn dragging_a_slider_writes_the_ddc_register(cx: &mut TestAppContext) {
        let (fake, mgr, _view, cx) = setup(cx, Page::Group(Group::Palette));
        assert_eq!(fake.register(0x10).unwrap().0, 86);
        let id = format!("{}:brightness", mgr.displays()[0].key());
        let b = cx.debug_bounds(Box::leak(format!("slider:{:?}", gpui::ElementId::from(SharedString::from(id))).into_boxed_str())).expect("brightness slider is drawn");
        // press near the right end of the travel
        let y = center(b).y;
        let x = b.origin.x + b.size.width * 0.9;
        cx.simulate_mouse_down(point(x, y), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(point(x, y), MouseButton::Left, Modifiers::none());
        std::thread::sleep(Duration::from_millis(500)); // write queue debounce
        let now = fake.register(0x10).unwrap().0;
        assert!(now > 80 && now <= 100, "brightness moved to the right end of the slider, got {now}");
    }

    #[gpui::test]
    fn picking_from_a_dropdown_writes_the_mapped_value(cx: &mut TestAppContext) {
        let (fake, mgr, _view, cx) = setup(cx, Page::Group(Group::Preset));
        // PA278QV preset table is built in: sRGB Mode = 13; choose Darkroom Mode = 24
        let key = mgr.displays()[0].key().to_string();
        let btn = cx.debug_bounds(Box::leak(format!("dd-btn:{:?}", gpui::ElementId::from(SharedString::from(format!("{key}:preset:dd")))).into_boxed_str())).expect("preset dropdown is drawn");
        cx.simulate_click(center(btn), Modifiers::none());
        cx.run_until_parked();
        // Standard, sRGB, Rec.709, DCI-P3, Scenery, Reading, Darkroom (index 6), Rapid
        let item = cx.debug_bounds("dd-item:6").expect("menu is open and lists the modes");
        cx.simulate_click(center(item), Modifiers::none());
        cx.run_until_parked();
        for _ in 0..40 {
            if fake.register(0xDC).unwrap().0 == 24 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(fake.register(0xDC).unwrap().0, 24, "Darkroom Mode");
    }
}

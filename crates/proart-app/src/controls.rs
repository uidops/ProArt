//! Controls for one OSD feature (slider, pop-up, button), shared by the
//! window and the menu-bar panel.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{div, prelude::*, px, AnyElement, App, PromptButton, PromptLevel, SharedString, Window};
use proart_core::features::{Feature, Kind};
use proart_core::manager::{DisplayState, Manager};
use proart_core::tint::{self, SoftPicture};

use crate::actions;
use crate::state::{Bus, Shared, ToastKind};
use crate::theme::Theme;
use crate::ui::{pill, row, DropItem, Dropdown, PillKind, Slider};

pub type LearnFn = Rc<dyn Fn(Arc<DisplayState>, &'static Feature, &mut Window, &mut App)>;

/// Shows a confirmation inside the view instead of a native prompt. The
/// menu-bar panel needs this: a prompt takes its focus, which closes it.
pub type ConfirmFn = Rc<dyn Fn(Confirmation, &mut Window, &mut App)>;

pub struct Confirmation {
    pub message: SharedString,
    pub detail: &'static str,
    pub then: Box<dyn FnOnce()>,
}

/// Everything a control needs; cheap to clone.
#[derive(Clone)]
pub struct Ctl {
    pub mgr: Arc<Manager>,
    pub bus: Arc<Bus>,
    pub t: Theme,
    pub on_learn: Option<LearnFn>,
    pub on_confirm: Option<ConfirmFn>,
}

/// Ask first when the user has not turned confirmations off: through
/// `on_confirm` when given, otherwise with a native prompt.
pub fn confirm(on_confirm: Option<&ConfirmFn>, window: &mut Window, cx: &mut App, message: &str, detail: &'static str, then: impl FnOnce() + 'static) {
    if !cx.global::<Shared>().mgr.with_settings(|s| s.global.confirm_dangerous) {
        then();
        return;
    }
    if let Some(show) = on_confirm {
        show(Confirmation { message: SharedString::from(message.to_string()), detail, then: Box::new(then) }, window, cx);
        return;
    }
    let rx = window.prompt(
        PromptLevel::Warning,
        message,
        Some(detail),
        &[PromptButton::ok("Continue"), PromptButton::cancel("Cancel")],
        cx,
    );
    cx.spawn(async move |_cx| {
        if let Ok(0) = rx.await {
            then();
        }
    })
    .detach();
}

// ------------------------------------------------------------ software picture --
// Warmth, extra dimming and gamma through the display's colour tables: they
// work on any monitor, independent of DDC/CI.

fn set_picture(mgr: &Arc<Manager>, bus: &Arc<Bus>, st: &Arc<DisplayState>, picture: SoftPicture, save: bool) {
    let ok = if save { mgr.set_software_picture(st, picture) } else { mgr.preview_software_picture(st, picture) };
    if !ok {
        bus.toast(format!("Could not change the colour tables of {}", mgr.label(st)), ToastKind::Error);
    }
    bus.repaint();
}

/// Value text to the left of a software slider.
fn value_label(t: &Theme, text: String) -> gpui::Div {
    div().w(px(52.)).text_size(px(13.)).text_color(t.dim).flex().justify_end().child(text)
}

/// A slider over a software-picture field, previewing while dragged.
fn picture_slider(
    ctl: &Ctl,
    st: &Arc<DisplayState>,
    name: &str,
    frac: f32,
    label: String,
    from_frac: fn(SoftPicture, f32) -> SoftPicture,
) -> AnyElement {
    let (mgr, bus, s) = (ctl.mgr.clone(), ctl.bus.clone(), st.clone());
    let on_change: crate::ui::FracFn = Rc::new(move |fr, _, _| set_picture(&mgr, &bus, &s, from_frac(mgr.software_picture(&s), fr), false));
    let (mgr, bus, s) = (ctl.mgr.clone(), ctl.bus.clone(), st.clone());
    let on_commit: crate::ui::FracFn = Rc::new(move |fr, _, _| set_picture(&mgr, &bus, &s, from_frac(mgr.software_picture(&s), fr), true));
    let id = SharedString::from(format!("{}:software_{name}", st.key()));
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.))
        .child(value_label(&ctl.t, label))
        .child(Slider::new(id, &ctl.t, frac, true, on_change, on_commit))
        .into_any_element()
}

/// Software blue-light filter by level, like the monitor's own filter.
pub fn software_blue_row(ctl: &Ctl, st: &Arc<DisplayState>, hint: &str) -> AnyElement {
    let picture = ctl.mgr.software_picture(st);
    let level = picture.level();
    let items: Vec<DropItem> = tint::LEVELS
        .iter()
        .enumerate()
        .map(|(i, name)| DropItem { label: (*name).into(), code: Some(i as u16), selected: level == Some(i as u8) })
        .collect();
    let label = match level {
        Some(l) => tint::LEVELS[l as usize].to_string(),
        None => format!("Custom ({} K)", picture.kelvin),
    };
    let (mgr, bus, s) = (ctl.mgr.clone(), ctl.bus.clone(), st.clone());
    let on_select: crate::ui::CodeFn = Rc::new(move |code, _, _| {
        let p = SoftPicture { kelvin: tint::level_kelvin(code as u8), ..mgr.software_picture(&s) };
        set_picture(&mgr, &bus, &s, p, true);
    });
    let dropdown = Dropdown::new(SharedString::from(format!("{}:software_blue:dd", st.key())), &ctl.t, label, items, on_select);
    row(&ctl.t, "Software Blue Light", hint, dropdown).into_any_element()
}

/// Warmth in kelvin: right is warmer.
pub fn warmth_row(ctl: &Ctl, st: &Arc<DisplayState>) -> AnyElement {
    let k = ctl.mgr.software_picture(st).kelvin;
    let span = (tint::NEUTRAL_KELVIN - tint::MIN_KELVIN) as f32;
    let frac = (tint::NEUTRAL_KELVIN - k) as f32 / span;
    let slider = picture_slider(ctl, st, "warmth", frac, format!("{k} K"), |p, fr| {
        let span = (tint::NEUTRAL_KELVIN - tint::MIN_KELVIN) as f32;
        // snap to 50 K so levels can be hit exactly
        let k = tint::NEUTRAL_KELVIN as f32 - fr * span;
        SoftPicture { kelvin: ((k / 50.0).round() * 50.0) as u32, ..p }
    });
    row(&ctl.t, "Warmth", "6500 K is the monitor's own white; lower is warmer.", slider).into_any_element()
}

/// Extra dimming below the monitor's lowest brightness.
pub fn dim_row(ctl: &Ctl, st: &Arc<DisplayState>) -> AnyElement {
    let dim = ctl.mgr.software_picture(st).dim;
    let frac = dim as f32 / tint::MAX_DIM as f32;
    let slider = picture_slider(ctl, st, "dim", frac, format!("{dim} %"), |p, fr| SoftPicture { dim: (fr * tint::MAX_DIM as f32).round() as u8, ..p });
    row(&ctl.t, "Extra dimming", "Darker than the monitor's minimum brightness, for night work.", slider).into_any_element()
}

/// Gamma curve on the Mac side.
pub fn gamma_row(ctl: &Ctl, st: &Arc<DisplayState>) -> AnyElement {
    let gamma = ctl.mgr.software_picture(st).gamma;
    let name = |g: f32| if (g - tint::NEUTRAL_GAMMA).abs() < 0.01 { format!("{g:.1} (unchanged)") } else { format!("{g:.1}") };
    let items: Vec<DropItem> =
        tint::GAMMAS.iter().enumerate().map(|(i, g)| DropItem { label: name(*g).into(), code: Some(i as u16), selected: (g - gamma).abs() < 0.01 }).collect();
    let (mgr, bus, s) = (ctl.mgr.clone(), ctl.bus.clone(), st.clone());
    let on_select: crate::ui::CodeFn = Rc::new(move |code, _, _| {
        let p = SoftPicture { gamma: tint::GAMMAS[code as usize], ..mgr.software_picture(&s) };
        set_picture(&mgr, &bus, &s, p, true);
    });
    let dropdown = Dropdown::new(SharedString::from(format!("{}:software_gamma:dd", st.key())), &ctl.t, name(gamma), items, on_select);
    row(&ctl.t, "Gamma", "Applied on the Mac; the monitor's own gamma stays as it is.", dropdown).into_any_element()
}

/// A labelled row for one feature; unlearned vendor items offer "Learn…".
pub fn feature_row(ctl: &Ctl, st: &Arc<DisplayState>, f: &'static Feature) -> AnyElement {
    use proart_core::manager::Status;
    let t = &ctl.t;
    if ctl.mgr.status(st, f) == Status::Unlearned {
        let on_learn = ctl.on_learn.clone();
        let st = st.clone();
        let button = pill(SharedString::from(format!("learn-{}", f.id)), t, "Learn\u{2026}", PillKind::Primary).on_click(move |_, window, cx| {
            if let Some(cb) = &on_learn {
                cb(st.clone(), f, window, cx);
            }
        });
        return row(t, f.label, "ASUS-specific: teach ProArt how this monitor stores it", button).into_any_element();
    }
    let hint = if f.vendor() && !f.verified && !f.note.is_empty() && f.group != proart_core::features::Group::Preset { f.note } else { "" };
    row(t, f.label, hint, control(ctl, st, f)).into_any_element()
}

pub fn control(ctl: &Ctl, st: &Arc<DisplayState>, f: &'static Feature) -> AnyElement {
    match f.kind {
        Kind::Slider => slider(ctl, st, f),
        Kind::Choice => dropdown(ctl, st, f),
        Kind::Action => action(ctl, st, f),
    }
}

fn slider(ctl: &Ctl, st: &Arc<DisplayState>, f: &'static Feature) -> AnyElement {
    let t = ctl.t;
    let max = st.max(f.id).unwrap_or(f.max).max(1);
    let value = st.value(f.id);
    let frac = value.map(|v| v as f32 / max as f32).unwrap_or(0.0);
    let id = SharedString::from(format!("{}:{}", st.key(), f.id));

    let (mgr, s) = (ctl.mgr.clone(), st.clone());
    let on_change: crate::ui::FracFn = Rc::new(move |fr, _, _| mgr.queue_write(&s, f, (fr * max as f32).round() as u16));
    let (mgr, bus, s) = (ctl.mgr.clone(), ctl.bus.clone(), st.clone());
    let on_commit: crate::ui::FracFn = Rc::new(move |fr, _, _| actions::check_later(&mgr, &bus, &s, f, (fr * max as f32).round() as u16));

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .w(px(34.))
                .text_size(px(13.))
                .text_color(t.dim)
                .flex()
                .justify_end()
                .child(value.map(|v| v.to_string()).unwrap_or_else(|| "\u{2014}".into())),
        )
        .child(Slider::new(id, &t, frac, value.is_some(), on_change, on_commit))
        .into_any_element()
}

fn dropdown(ctl: &Ctl, st: &Arc<DisplayState>, f: &'static Feature) -> AnyElement {
    let res = ctl.mgr.resolve(st, f);
    let current = st.value(f.id);
    let label = current.map(|v| ctl.mgr.choice_label(st, f, v)).unwrap_or_else(|| "\u{2014}".into());
    let items: Vec<DropItem> = res
        .choices
        .iter()
        .map(|(name, code)| DropItem {
            label: if code.is_some() { name.clone().into() } else { format!("{name} (not learned)").into() },
            code: *code,
            selected: code.is_some() && *code == current,
        })
        .collect();
    let names: Vec<(u16, String)> = res.choices.iter().filter_map(|(n, c)| c.map(|c| (c, n.clone()))).collect();
    let (ctl2, s) = (ctl.clone(), st.clone());
    let on_select: crate::ui::CodeFn = Rc::new(move |code, window, cx| {
        let name = names.iter().find(|(c, _)| *c == code).map(|(_, n)| n.clone()).unwrap_or_default();
        let (mgr, bus, s) = (ctl2.mgr.clone(), ctl2.bus.clone(), s.clone());
        if f.dangerous {
            let msg = format!("Set {} to {name} on {}?", f.label, mgr.label(&s));
            confirm(ctl2.on_confirm.as_ref(), window, cx, &msg, "The display may switch signal or lock controls.", move || actions::write_async(&mgr, &bus, &s, f, Some(code)));
        } else {
            actions::write_async(&mgr, &bus, &s, f, Some(code));
        }
    });
    Dropdown::new(SharedString::from(format!("{}:{}:dd", st.key(), f.id)), &ctl.t, label, items, on_select).into_any_element()
}

fn action(ctl: &Ctl, st: &Arc<DisplayState>, f: &'static Feature) -> AnyElement {
    let text = match f.id {
        "mute" => "Toggle",
        "color_reset" => "Reset",
        _ => "Run",
    };
    let kind = if f.dangerous { PillKind::Danger } else { PillKind::Normal };
    let (mgr, bus, s, on_confirm) = (ctl.mgr.clone(), ctl.bus.clone(), st.clone(), ctl.on_confirm.clone());
    pill(SharedString::from(format!("{}:{}:act", st.key(), f.id)), &ctl.t, text, kind)
        .on_click(move |_, window, cx| {
            let (mgr, bus, s) = (mgr.clone(), bus.clone(), s.clone());
            if f.id == "mute" {
                actions::mute_async(&mgr, &s);
            } else if f.dangerous {
                let msg = format!("{} on {}?", f.label, mgr.label(&s));
                confirm(on_confirm.as_ref(), window, cx, &msg, "This cannot be undone from here.", move || actions::write_async(&mgr, &bus, &s, f, None));
            } else {
                actions::write_async(&mgr, &bus, &s, f, None);
            }
        })
        .into_any_element()
}

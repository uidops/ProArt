//! Controls for one OSD feature (slider, pop-up, button), shared by the
//! window and the menu-bar panel.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{div, prelude::*, px, AnyElement, App, PromptButton, PromptLevel, SharedString, Window};
use proart_core::features::{Feature, Kind};
use proart_core::manager::{DisplayState, Manager};

use crate::actions;
use crate::state::{Bus, Shared};
use crate::theme::Theme;
use crate::ui::{pill, row, DropItem, Dropdown, PillKind, Slider};

pub type LearnFn = Rc<dyn Fn(Arc<DisplayState>, &'static Feature, &mut Window, &mut App)>;

/// Everything a control needs; cheap to clone.
#[derive(Clone)]
pub struct Ctl {
    pub mgr: Arc<Manager>,
    pub bus: Arc<Bus>,
    pub t: Theme,
    pub on_learn: Option<LearnFn>,
}

/// Ask first when the user has not turned confirmations off.
pub fn confirm(window: &mut Window, cx: &mut App, message: &str, detail: &str, then: impl FnOnce() + 'static) {
    if !cx.global::<Shared>().mgr.with_settings(|s| s.global.confirm_dangerous) {
        then();
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
            confirm(window, cx, &msg, "The display may switch signal or lock controls.", move || actions::write_async(&mgr, &bus, &s, f, Some(code)));
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
    let (mgr, bus, s) = (ctl.mgr.clone(), ctl.bus.clone(), st.clone());
    pill(SharedString::from(format!("{}:{}:act", st.key(), f.id)), &ctl.t, text, kind)
        .on_click(move |_, window, cx| {
            let (mgr, bus, s) = (mgr.clone(), bus.clone(), s.clone());
            if f.id == "mute" {
                actions::mute_async(&mgr, &s);
            } else if f.dangerous {
                let msg = format!("{} on {}?", f.label, mgr.label(&s));
                confirm(window, cx, &msg, "This cannot be undone from here.", move || actions::write_async(&mgr, &bus, &s, f, None));
            } else {
                actions::write_async(&mgr, &bus, &s, f, None);
            }
        })
        .into_any_element()
}

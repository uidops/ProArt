//! Main-thread side of the keyboard keys and per-app presets (macOS): turns
//! key presses into monitor changes with an on-screen indicator, keeps the
//! key tap's view of the settings current and follows the frontmost app.

use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{point, prelude::*, px, App, AsyncApp, SharedString, WindowHandle};
use proart_core::config::Preset;
use proart_core::features;
use proart_core::manager::{DisplayState, Manager};

use crate::actions;
use crate::app_presets::{AppSwitcher, Step};
use crate::mac;
use crate::media_keys::{self, KeyAction, KeyMsg};
use crate::osd_view::{self, OsdKind, OsdView};
use crate::state::Shared;

const OSD_TIME: Duration = Duration::from_millis(1500);
/// Brightness and volume keys move 1/16 of the range, like macOS.
const STEPS: f32 = 16.0;

struct Watch {
    keys: Receiver<KeyMsg>,
    switcher: AppSwitcher,
    /// Settings from before an app's preset, put back when it goes.
    restore: Option<Preset>,
    own: Option<String>,
    osd: Option<(WindowHandle<OsdView>, Instant)>,
}

pub fn start(cx: &mut App) {
    let (tx, rx) = channel();
    media_keys::start(tx);
    let mut w = Watch { keys: rx, switcher: AppSwitcher::default(), restore: None, own: mac::own_bundle_id(), osd: None };
    cx.spawn(async move |cx: &mut AsyncApp| {
        let mut n = 0u64;
        loop {
            cx.background_executor().timer(Duration::from_millis(50)).await;
            n += 1;
            let alive = cx.update(|cx| {
                let msgs: Vec<KeyMsg> = w.keys.try_iter().collect();
                for m in msgs {
                    w.key(cx, m);
                }
                if n % 20 == 0 {
                    w.sync_keys(cx);
                    w.front_app(cx);
                }
                w.expire_osd(cx);
            });
            if alive.is_err() {
                return;
            }
        }
    })
    .detach();
}

fn cg_id(st: &DisplayState) -> Option<u32> {
    st.info.extra.get("cg_id")?.parse().ok()
}

/// Move a slider feature one key step; returns the new fraction.
fn nudge(mgr: &Arc<Manager>, st: &Arc<DisplayState>, id: &str, up: bool) -> Option<f32> {
    let f = features::get(id)?;
    if !mgr.visible(st, f) {
        return None;
    }
    let max = st.max(f.id).unwrap_or(f.max).max(1);
    let cur = st.value(f.id).unwrap_or(max / 2);
    let step = ((max as f32 / STEPS).round() as u16).max(1);
    let new = if up { cur.saturating_add(step).min(max) } else { cur.saturating_sub(step) };
    mgr.queue_write(st, f, new);
    Some(new as f32 / max as f32)
}

impl Watch {
    fn key(&mut self, cx: &mut App, msg: KeyMsg) {
        let mgr = cx.global::<Shared>().mgr.clone();
        let displays = mgr.displays();
        let under = msg.display.and_then(|id| displays.iter().find(|d| cg_id(d) == Some(id)).cloned());
        let has_volume = |d: &Arc<DisplayState>| features::get("volume").is_some_and(|f| mgr.visible(d, f));
        match msg.action {
            KeyAction::BrightnessUp | KeyAction::BrightnessDown => {
                let Some(st) = under else { return };
                if let Some(frac) = nudge(&mgr, &st, "brightness", msg.action == KeyAction::BrightnessUp) {
                    self.show_osd(cx, OsdKind::Brightness, frac, None);
                }
            }
            KeyAction::VolumeUp | KeyAction::VolumeDown | KeyAction::Mute => {
                let st = under.filter(has_volume).or_else(|| displays.iter().find(|d| !d.info.builtin && has_volume(d)).cloned());
                let Some(st) = st else { return };
                if msg.action == KeyAction::Mute {
                    actions::mute_async(&mgr, &st);
                    let max = st.max("volume").unwrap_or(100).max(1) as f32;
                    let frac = st.value("volume").unwrap_or(0) as f32 / max;
                    self.show_osd(cx, OsdKind::Volume, frac, Some("Mute".into()));
                } else if let Some(frac) = nudge(&mgr, &st, "volume", msg.action == KeyAction::VolumeUp) {
                    self.show_osd(cx, OsdKind::Volume, frac, None);
                }
            }
        }
    }

    fn show_osd(&mut self, cx: &mut App, kind: OsdKind, frac: f32, note: Option<SharedString>) {
        let until = Instant::now() + OSD_TIME;
        if let Some((h, _)) = self.osd {
            let shown = h.update(cx, |v, _, cx| {
                (v.kind, v.frac, v.note) = (kind, frac, note.clone());
                cx.notify();
            });
            if shown.is_ok() {
                self.osd = Some((h, until));
                return;
            }
        }
        let (w, h) = cx.primary_display().map(|d| (f32::from(d.bounds().size.width), f32::from(d.bounds().size.height))).unwrap_or((1440.0, 900.0));
        let origin = point(px((w - osd_view::OSD_W) / 2.0), px(h - osd_view::OSD_H - 120.0));
        match cx.open_window(osd_view::window_options(origin), |_, cx| cx.new(|_| OsdView { kind, frac, note })) {
            Ok(handle) => self.osd = Some((handle, until)),
            Err(e) => log::warn!("cannot show the indicator: {e}"),
        }
    }

    fn expire_osd(&mut self, cx: &mut App) {
        if let Some((h, until)) = self.osd {
            if Instant::now() >= until {
                let _ = h.update(cx, |_, window, _| window.remove_window());
                self.osd = None;
            }
        }
    }

    /// Tell the key tap what it may take.
    fn sync_keys(&mut self, cx: &mut App) {
        let mgr = cx.global::<Shared>().mgr.clone();
        let keys = mgr.with_settings(|s| s.automation.keys.clone());
        let c = media_keys::config();
        c.brightness.store(keys.brightness, std::sync::atomic::Ordering::Relaxed);
        c.volume.store(keys.volume, std::sync::atomic::Ordering::Relaxed);
        let brightness = features::get("brightness");
        let ids = mgr.displays().iter().filter(|d| !d.info.builtin && brightness.is_some_and(|f| mgr.visible(d, f))).filter_map(|d| cg_id(d)).collect();
        *c.monitors.lock().unwrap() = ids;
    }

    fn front_app(&mut self, cx: &mut App) {
        let mgr = cx.global::<Shared>().mgr.clone();
        let (rules, presets) = mgr.with_settings(|s| (s.automation.app_presets.clone(), s.presets.clone()));
        if rules.is_empty() && self.restore.is_none() {
            return;
        }
        let Some(front) = mac::frontmost_app() else { return };
        let find = |id: &str| presets.iter().find(|p| p.id == id).cloned();
        match self.switcher.on_front(&front.bundle_id, &rules, self.own.as_deref()) {
            Step::Nothing => {}
            Step::SaveAndApply(id) => {
                let mut before = mgr.capture_preset("Before app preset", "");
                // never switch the input back and forth behind the user's back
                for values in before.values.values_mut() {
                    values.retain(|id, _| features::get(id).is_some_and(|f| !f.dangerous));
                }
                self.restore = Some(before);
                if let Some(p) = find(&id) {
                    actions::apply_preset_async(&mgr, p);
                }
            }
            Step::Apply(id) => {
                if let Some(p) = find(&id) {
                    actions::apply_preset_async(&mgr, p);
                }
            }
            Step::Restore => {
                if let Some(p) = self.restore.take() {
                    actions::apply_preset_async(&mgr, p);
                }
            }
        }
    }
}

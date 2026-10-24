//! Work that must not block the UI thread: DDC writes, read-backs, power.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use proart_core::features::{self, Feature, Group};
use proart_core::manager::{DisplayState, Manager};

use crate::state::{Bus, ToastKind};

pub fn write_async(mgr: &Arc<Manager>, bus: &Arc<Bus>, st: &Arc<DisplayState>, f: &'static Feature, value: Option<u16>) {
    let (mgr, bus, st) = (mgr.clone(), bus.clone(), st.clone());
    thread::Builder::new()
        .name("write".into())
        .spawn(move || {
            let (sent, applied) = mgr.write_confirmed(&st, f, value, Duration::from_secs(3));
            if !sent {
                bus.toast(format!("{} rejected {}", mgr.label(&st), f.label), ToastKind::Error);
            } else if applied == Some(false) {
                bus.toast(
                    format!(
                        "{} ignored the change: it locks {} in its current picture mode. Try another ProArt Preset.",
                        mgr.label(&st),
                        f.label
                    ),
                    ToastKind::Warn,
                );
            }
            if f.group == Group::Preset {
                // a mode switch can move brightness and contrast: show what the monitor now has
                thread::sleep(Duration::from_secs(2));
                for id in ["brightness", "contrast", "color_temp"] {
                    if let Some(g) = features::get(id) {
                        mgr.read(&st, g);
                    }
                }
            }
            bus.repaint();
        })
        .ok();
}

pub fn power_async(mgr: &Arc<Manager>, bus: &Arc<Bus>, st: &Arc<DisplayState>, on: bool) {
    let (mgr, bus, st) = (mgr.clone(), bus.clone(), st.clone());
    thread::Builder::new()
        .name("power".into())
        .spawn(move || {
            if !mgr.power(&st, on) {
                bus.toast(format!("{} rejected the power command", mgr.label(&st)), ToastKind::Error);
            }
        })
        .ok();
}

pub fn mute_async(mgr: &Arc<Manager>, st: &Arc<DisplayState>) {
    let (mgr, st) = (mgr.clone(), st.clone());
    thread::spawn(move || mgr.toggle_mute(&st));
}

/// After a slider drag, read back once the monitor has settled and warn if it
/// ignored the change (a mode lock).
pub fn check_later(mgr: &Arc<Manager>, bus: &Arc<Bus>, st: &Arc<DisplayState>, f: &'static Feature, value: u16) {
    let (mgr, bus, st) = (mgr.clone(), bus.clone(), st.clone());
    thread::Builder::new()
        .name("verify".into())
        .spawn(move || {
            thread::sleep(Duration::from_millis(3200));
            if st.value(f.id) != Some(value) {
                return; // moved again meanwhile
            }
            if let Some(got) = mgr.read(&st, f) {
                if got != value.min(st.max(f.id).unwrap_or(f.max)) {
                    bus.toast(format!("{} did not change: {} locks it in its current picture mode", f.label, mgr.label(&st)), ToastKind::Warn);
                }
            }
        })
        .ok();
}

pub fn apply_preset_async(mgr: &Arc<Manager>, preset: proart_core::config::Preset) {
    let mgr = mgr.clone();
    thread::spawn(move || mgr.apply_preset(&preset));
}

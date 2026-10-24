//! State shared between windows and worker threads.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{Global, Image};
use proart_core::manager::{DisplayState, Manager};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Warn,
    Error,
}

#[derive(Clone)]
pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub until: Instant,
}

/// Cross-thread mailbox: workers bump a generation counter, the views poll it
/// and re-render, and toasts / probe results live here.
#[derive(Default)]
pub struct Bus {
    gen: AtomicU64,
    toasts: Mutex<Vec<Toast>>,
    info: Mutex<HashMap<String, Vec<(String, String)>>>,
    probed: Mutex<HashSet<String>>,
    probing: Mutex<HashSet<String>>,
}

impl Bus {
    pub fn repaint(&self) {
        self.gen.fetch_add(1, Ordering::Relaxed);
    }

    pub fn gen(&self) -> u64 {
        self.gen.load(Ordering::Relaxed)
    }

    pub fn toast(&self, text: impl Into<String>, kind: ToastKind) {
        self.toasts.lock().unwrap().push(Toast { text: text.into(), kind, until: Instant::now() + Duration::from_secs(6) });
        self.repaint();
    }

    /// Live toasts (expired ones are dropped).
    pub fn toasts(&self) -> Vec<Toast> {
        let mut t = self.toasts.lock().unwrap();
        t.retain(|x| x.until > Instant::now());
        t.clone()
    }

    pub fn info_rows(&self, key: &str) -> Vec<(String, String)> {
        self.info.lock().unwrap().get(key).cloned().unwrap_or_default()
    }

    pub fn is_probing(&self, key: &str) -> bool {
        self.probing.lock().unwrap().contains(key)
    }

    /// Probe a display once in the background so the UI knows what it has.
    pub fn ensure_probed(self: &Arc<Self>, mgr: &Arc<Manager>, st: &Arc<DisplayState>) {
        let key = st.key().to_string();
        if self.probed.lock().unwrap().contains(&key) || !self.probing.lock().unwrap().insert(key.clone()) {
            return;
        }
        let (bus, mgr, st) = (self.clone(), mgr.clone(), st.clone());
        std::thread::Builder::new()
            .name("probe".into())
            .spawn(move || {
                mgr.probe(&st);
                let rows = mgr.info_rows(&st);
                bus.info.lock().unwrap().insert(key.clone(), rows);
                bus.probing.lock().unwrap().remove(&key);
                bus.probed.lock().unwrap().insert(key);
                bus.repaint();
            })
            .ok();
    }
}

/// Globals every window reads.
pub struct Shared {
    pub mgr: Arc<Manager>,
    pub bus: Arc<Bus>,
    pub logo: Arc<Image>,
    /// A tray icon exists, so closing the last window keeps the app running.
    pub tray: bool,
}

impl Global for Shared {}

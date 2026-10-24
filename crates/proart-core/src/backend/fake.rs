//! In-memory backend used by tests and as the fallback on unsupported OSes.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use super::{Backend, DisplayInfo, Session};
use crate::ddc::VcpReply;

/// Shared register file: opcode -> (current, max).
pub type Store = Arc<Mutex<HashMap<u8, (u16, u16)>>>;

pub struct FakeBackend {
    pub displays: Mutex<Vec<DisplayInfo>>,
    pub store: Store,
    /// Registers that accept a write frame but ignore it (mode-locked controls).
    pub locked: Arc<Mutex<HashSet<u8>>>,
}

impl FakeBackend {
    pub fn empty() -> Self {
        FakeBackend { displays: Mutex::new(Vec::new()), store: Store::default(), locked: Arc::default() }
    }

    /// Two displays sharing one register file, brightness/contrast/volume preset.
    pub fn sample() -> Self {
        let b = FakeBackend::empty();
        {
            let mut d = b.displays.lock().unwrap();
            for (key, name) in [("fake-1", "FakeDisplay"), ("fake-2", "FakeDisplay2")] {
                d.push(DisplayInfo {
                    key: key.into(),
                    name: name.into(),
                    backend: "fake".into(),
                    ..DisplayInfo::default()
                });
            }
        }
        {
            let mut s = b.store.lock().unwrap();
            s.insert(0x10, (86, 100));
            s.insert(0x12, (100, 100));
            s.insert(0x62, (50, 100));
        }
        b
    }

    pub fn set_register(&self, code: u8, cur: u16, max: u16) {
        self.store.lock().unwrap().insert(code, (cur, max));
    }

    pub fn lock_register(&self, code: u8) {
        self.locked.lock().unwrap().insert(code);
    }

    pub fn register(&self, code: u8) -> Option<(u16, u16)> {
        self.store.lock().unwrap().get(&code).copied()
    }

    pub fn rename(&self, index: usize, name: &str) {
        self.displays.lock().unwrap()[index].name = name.to_string();
    }
}

struct FakeSession(Store, Arc<Mutex<HashSet<u8>>>);

impl Session for FakeSession {
    fn get_vcp(&mut self, code: u8) -> Option<VcpReply> {
        self.0.lock().unwrap().get(&code).map(|&(current, max)| VcpReply { current, max })
    }

    fn set_vcp(&mut self, code: u8, value: u16) -> bool {
        if self.1.lock().unwrap().contains(&code) {
            return true; // frame sent, silently ignored
        }
        let mut s = self.0.lock().unwrap();
        let max = s.get(&code).map(|c| c.1).unwrap_or(100);
        s.insert(code, (value.min(max), max));
        true
    }
}

impl Backend for FakeBackend {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn list_displays(&self) -> Vec<DisplayInfo> {
        self.displays.lock().unwrap().clone()
    }

    fn open_session(&self, _info: &DisplayInfo) -> Option<Box<dyn Session>> {
        Some(Box::new(FakeSession(self.store.clone(), self.locked.clone())))
    }
}

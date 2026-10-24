//! Display backends: one per OS behind a common interface, plus a timeout
//! guard so a misbehaving monitor can never wedge the UI (a stuck DDC read can
//! hang a Mac).

use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use crate::ddc::VcpReply;

pub mod fake;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod windows;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayInfo {
    /// Stable identity across reconnects.
    pub key: String,
    pub name: String,
    pub model: String,
    pub serial: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub builtin: bool,
    pub width: u32,
    pub height: u32,
    pub backend: String,
    pub extra: HashMap<String, String>,
}

impl DisplayInfo {
    pub fn label(&self, override_name: &str) -> String {
        if !override_name.is_empty() {
            override_name.to_string()
        } else if !self.name.is_empty() {
            self.name.clone()
        } else {
            "Display".to_string()
        }
    }
}

/// An open control channel to one display.
pub trait Session: Send {
    fn get_vcp(&mut self, code: u8) -> Option<VcpReply>;
    fn set_vcp(&mut self, code: u8, value: u16) -> bool;
    fn capabilities(&mut self) -> Option<String> {
        None
    }
    /// Software dimming fallback (0-100) when DDC rejects brightness writes.
    fn set_software_level(&mut self, _level: u8) -> bool {
        false
    }
    /// A single read without retries (used for feature probing and scans,
    /// where most opcodes are expected to be unanswered).
    fn probe_vcp(&mut self, code: u8) -> Option<VcpReply> {
        self.get_vcp(code)
    }
    /// True once the display stopped answering; the manager reopens it later.
    fn is_dead(&self) -> bool {
        false
    }
}

pub trait Backend: Send + Sync {
    fn name(&self) -> &'static str;
    fn list_displays(&self) -> Vec<DisplayInfo>;
    fn open_session(&self, info: &DisplayInfo) -> Option<Box<dyn Session>>;
    /// Cheap fingerprint of the connected displays, polled for hotplug.
    fn poll_signature(&self) -> String {
        self.list_displays().iter().map(|d| d.key.as_str()).collect::<Vec<_>>().join("|")
    }
}

/// The backend for the OS this binary was built for.
pub fn default_backend() -> Box<dyn Backend> {
    #[cfg(target_os = "macos")]
    return Box::new(macos::MacBackend::new());
    #[cfg(windows)]
    return Box::new(windows::WindowsBackend::new());
    #[cfg(target_os = "linux")]
    return Box::new(linux::LinuxBackend::new());
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    return Box::new(fake::FakeBackend::empty());
}

// --------------------------------------------------------------------------
// Timeout guard
// --------------------------------------------------------------------------

/// Raw transport: blocking, may hang. Owned by a worker thread.
pub trait Transport: Send + 'static {
    fn get(&mut self, code: u8) -> Option<VcpReply>;
    fn set(&mut self, code: u8, value: u16) -> bool;
    fn capabilities(&mut self) -> Option<String> {
        None
    }
    fn software_level(&mut self, _level: u8) -> bool {
        false
    }
}

enum Job {
    Get(u8, Sender<Option<VcpReply>>),
    Set(u8, u16, Sender<bool>),
    Caps(Sender<Option<String>>),
    Soft(u8, Sender<bool>),
}

/// Runs a [`Transport`] on its own thread and gives every call a deadline.
/// After `MAX_TIMEOUTS` consecutive timeouts the display is treated as dead
/// and further calls fail immediately instead of queueing behind a hung call.
pub struct GuardedSession {
    jobs: Sender<Job>,
    timeout: Duration,
    timeouts: u32,
    dead: bool,
    retries: usize,
}

const MAX_TIMEOUTS: u32 = 3;

impl GuardedSession {
    pub fn new<T: Transport>(mut transport: T, timeout: Duration, retries: usize) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("ddc-io".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    match job {
                        Job::Get(code, reply) => {
                            let _ = reply.send(transport.get(code));
                        }
                        Job::Set(code, value, reply) => {
                            let _ = reply.send(transport.set(code, value));
                        }
                        Job::Caps(reply) => {
                            let _ = reply.send(transport.capabilities());
                        }
                        Job::Soft(level, reply) => {
                            let _ = reply.send(transport.software_level(level));
                        }
                    }
                }
            })
            .expect("spawn ddc-io thread");
        GuardedSession { jobs: tx, timeout, timeouts: 0, dead: false, retries }
    }

    fn call<R: Default>(&mut self, make: impl FnOnce(Sender<R>) -> Job) -> R {
        if self.dead {
            return R::default();
        }
        let (tx, rx) = mpsc::channel();
        if self.jobs.send(make(tx)).is_err() {
            self.dead = true;
            return R::default();
        }
        match rx.recv_timeout(self.timeout) {
            Ok(v) => {
                self.timeouts = 0;
                v
            }
            Err(RecvTimeoutError::Timeout) => {
                self.timeouts += 1;
                if self.timeouts >= MAX_TIMEOUTS {
                    self.dead = true;
                }
                R::default()
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.dead = true;
                R::default()
            }
        }
    }
}

impl Session for GuardedSession {
    fn get_vcp(&mut self, code: u8) -> Option<VcpReply> {
        for attempt in 0..=self.retries {
            if let Some(r) = self.call(|tx| Job::Get(code, tx)) {
                return Some(r);
            }
            if self.dead {
                return None;
            }
            if attempt < self.retries {
                thread::sleep(Duration::from_millis(100));
            }
        }
        None
    }

    fn set_vcp(&mut self, code: u8, value: u16) -> bool {
        // one retry: a monitor that just woke often drops the first frame
        self.call(|tx| Job::Set(code, value, tx)) || self.call(|tx| Job::Set(code, value, tx))
    }

    fn capabilities(&mut self) -> Option<String> {
        self.call(Job::Caps)
    }

    fn set_software_level(&mut self, level: u8) -> bool {
        self.call(|tx| Job::Soft(level, tx))
    }

    fn probe_vcp(&mut self, code: u8) -> Option<VcpReply> {
        self.call(|tx| Job::Get(code, tx))
    }

    fn is_dead(&self) -> bool {
        self.dead
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Hang;
    impl Transport for Hang {
        fn get(&mut self, _code: u8) -> Option<VcpReply> {
            thread::sleep(Duration::from_millis(400));
            None
        }
        fn set(&mut self, _c: u8, _v: u16) -> bool {
            false
        }
    }

    #[test]
    fn a_hung_display_times_out_and_is_declared_dead() {
        let mut s = GuardedSession::new(Hang, Duration::from_millis(30), 0);
        for _ in 0..3 {
            assert!(s.get_vcp(0x10).is_none());
        }
        let t = std::time::Instant::now();
        assert!(s.get_vcp(0x10).is_none(), "dead sessions fail fast");
        assert!(t.elapsed() < Duration::from_millis(20));
    }
}

//! "Learn" wizard state: teaches ProArt where this monitor stores an
//! ASUS-specific OSD item by watching registers while the user changes it on
//! the monitor. The UI lives in `main_view.rs`; this is the engine.

use std::sync::{Arc, Mutex};
use std::thread;

use proart_core::features::{Feature, Kind};
use proart_core::learn::{LearnSession, Outcome};
use proart_core::manager::{DisplayState, Manager};

use crate::state::Bus;

enum Phase {
    Scanning,
    Step(usize),
    Choose(Vec<u8>),
    Done,
}

struct Shared {
    phase: Phase,
    progress: (usize, usize),
    session: Option<LearnSession>,
    notes: Vec<String>,
    busy: bool,
    message: String,
}

/// What the dialog should show right now.
pub enum Snapshot {
    Scanning { done: usize, total: usize },
    Step { index: usize, total: usize, name: String, busy: bool, notes: Vec<String> },
    Choose(Vec<u8>),
    Done(String),
}

#[derive(Clone)]
pub struct LearnState {
    pub f: &'static Feature,
    st: Arc<DisplayState>,
    mgr: Arc<Manager>,
    bus: Arc<Bus>,
    names: Vec<String>,
    shared: Arc<Mutex<Shared>>,
}

impl LearnState {
    pub fn start(mgr: Arc<Manager>, bus: Arc<Bus>, st: Arc<DisplayState>, f: &'static Feature) -> LearnState {
        let names: Vec<String> = if f.kind == Kind::Slider {
            vec!["minimum".into(), "maximum".into()]
        } else {
            f.options.iter().map(|s| s.to_string()).collect()
        };
        let shared = Arc::new(Mutex::new(Shared {
            phase: Phase::Scanning,
            progress: (0, 255),
            session: None,
            notes: Vec::new(),
            busy: true,
            message: String::new(),
        }));
        {
            let (mgr, bus, st, names, shared) = (mgr.clone(), bus.clone(), st.clone(), names.clone(), shared.clone());
            thread::Builder::new()
                .name("learn-scan".into())
                .spawn(move || {
                    let sh = shared.clone();
                    let b = bus.clone();
                    let session = LearnSession::begin_with_progress(&mgr, &st, f, names, &mut |done, total| {
                        sh.lock().unwrap().progress = (done, total);
                        if done % 4 == 0 {
                            b.repaint();
                        }
                    });
                    let mut s = shared.lock().unwrap();
                    s.session = Some(session);
                    s.phase = Phase::Step(0);
                    s.busy = false;
                    bus.repaint();
                })
                .ok();
        }
        LearnState { f, st, mgr, bus, names, shared }
    }

    pub fn snapshot(&self) -> Snapshot {
        let sh = self.shared.lock().unwrap();
        match &sh.phase {
            Phase::Scanning => Snapshot::Scanning { done: sh.progress.0, total: sh.progress.1 },
            Phase::Step(i) => Snapshot::Step { index: *i, total: self.names.len(), name: self.names[*i].clone(), busy: sh.busy, notes: sh.notes.clone() },
            Phase::Choose(list) => Snapshot::Choose(list.clone()),
            Phase::Done => Snapshot::Done(sh.message.clone()),
        }
    }

    /// The user set the value on the monitor (`capture`) or skipped it.
    pub fn advance(&self, capture: bool) {
        let (i, mut session) = {
            let mut sh = self.shared.lock().unwrap();
            let Phase::Step(i) = sh.phase else { return };
            if sh.busy {
                return;
            }
            sh.busy = true;
            (i, sh.session.take())
        };
        let (name, last) = (self.names[i].clone(), i + 1 == self.names.len());
        let (shared, mgr, st, bus, f) = (self.shared.clone(), self.mgr.clone(), self.st.clone(), self.bus.clone(), self.f);
        thread::Builder::new()
            .name("learn-capture".into())
            .spawn(move || {
                let mut note = None;
                if capture {
                    if let Some(s) = session.as_mut() {
                        if !s.capture(&mgr, &st, &name) {
                            note = Some(format!("Nothing changed, so \u{201c}{name}\u{201d} was ignored."));
                        }
                    }
                }
                let mut sh = shared.lock().unwrap();
                if let Some(n) = note {
                    sh.notes.push(n);
                }
                if !last {
                    sh.session = session;
                    sh.phase = Phase::Step(i + 1);
                    sh.busy = false;
                    bus.repaint();
                    return;
                }
                if let Some(session) = session {
                    match session.outcome() {
                        Outcome::NothingChanged | Outcome::NoRegister => {
                            mgr.mark_unsupported(&st, f);
                            sh.message = format!("{} is not exposed by this monitor over DDC/CI, so it was marked as not available.", f.label);
                            sh.phase = Phase::Done;
                        }
                        Outcome::NeedMore => {
                            sh.message = "Only one value changed, which is not enough to tell the register apart. Try again and change at least two values.".into();
                            sh.phase = Phase::Done;
                        }
                        Outcome::Found(list) => {
                            let pick = if list.len() == 1 { Some(list[0]) } else { f.vcp.filter(|h| list.contains(h)) };
                            match pick {
                                Some(code) => {
                                    let values = session.values(code);
                                    mgr.save_learned(&st, f, code, values.clone());
                                    sh.message = format!("Learned {}: register 0x{code:02X}, {} value(s).", f.label, values.len());
                                    sh.phase = Phase::Done;
                                }
                                None => {
                                    sh.session = Some(session);
                                    sh.phase = Phase::Choose(list);
                                }
                            }
                        }
                    }
                }
                sh.busy = false;
                bus.repaint();
            })
            .ok();
    }

    /// Several registers matched: the user picked one.
    pub fn choose(&self, code: u8) {
        let mut sh = self.shared.lock().unwrap();
        if let Some(session) = sh.session.take() {
            let values = session.values(code);
            self.mgr.save_learned(&self.st, self.f, code, values.clone());
            sh.message = format!("Learned {}: register 0x{code:02X}, {} value(s).", self.f.label, values.len());
            sh.phase = Phase::Done;
        }
        self.bus.repaint();
    }
}

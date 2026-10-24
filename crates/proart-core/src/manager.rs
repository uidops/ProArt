//! Display manager: hotplug-aware display states, value cache, coalesced
//! background writes, live value polling, feature resolution (learned /
//! built-in / per-model), probing, presets and power.
//!
//! Every method that touches a display blocks on DDC I/O; the GUI calls them
//! from worker threads or through [`Manager::queue_write`].

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::backend::{Backend, DisplayInfo, Session};
use crate::config::{Learned, Preset, Settings};
use crate::ddc::{MUTE_OFF, MUTE_ON, POWER_OFF, POWER_ON, VCP_MUTE, VCP_POWER_MODE, VCP_TABLE_VERSION};
use crate::features::{self, Feature, Gen, Kind};
use crate::learn::Scan;

const POLL_INTERVAL: Duration = Duration::from_millis(2500);
/// Don't read back a control the user touched this recently.
const WRITE_QUIET: Duration = Duration::from_millis(1500);
const DEAD_COOLDOWN: Duration = Duration::from_secs(10);
/// Sliders mirrored across displays when "sync all" is on.
const SYNC_FEATURES: &[&str] = &["brightness", "contrast", "volume"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The set of displays changed.
    Displays,
    /// A feature value changed (write, read-back or live poll).
    Value { display: String, feature: String },
}

pub type EventFn = Arc<dyn Fn(Event) + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    /// ASUS-specific and not yet learned for this display.
    Unlearned,
    /// The display does not expose it.
    Unsupported,
}

/// A feature's opcode and values as they apply to one display.
#[derive(Clone, Debug, Default)]
pub struct Resolved {
    pub vcp: Option<u8>,
    pub choices: Vec<(String, Option<u16>)>,
}

#[derive(Default)]
struct IoState {
    session: Option<Box<dyn Session>>,
    dead_since: Option<Instant>,
}

#[derive(Default)]
struct Data {
    values: HashMap<&'static str, u16>,
    max: HashMap<&'static str, u16>,
    status: HashMap<&'static str, Status>,
    gen: Gen,
    mute_native: Option<bool>,
    premute_volume: Option<u16>,
    power_max: Option<u16>,
    last_write: HashMap<&'static str, Instant>,
    read_failures: u32,
    retry_after: Option<Instant>,
}

pub struct DisplayState {
    pub info: DisplayInfo,
    io: Mutex<IoState>,
    data: Mutex<Data>,
}

impl DisplayState {
    fn new(info: DisplayInfo) -> DisplayState {
        DisplayState { info, io: Mutex::new(IoState::default()), data: Mutex::new(Data::default()) }
    }

    pub fn key(&self) -> &str {
        &self.info.key
    }

    pub fn value(&self, id: &str) -> Option<u16> {
        self.data.lock().unwrap().values.iter().find(|(k, _)| **k == id).map(|(_, v)| *v)
    }

    pub fn max(&self, id: &str) -> Option<u16> {
        self.data.lock().unwrap().max.iter().find(|(k, _)| **k == id).map(|(_, v)| *v)
    }

    pub fn gen(&self) -> Gen {
        self.data.lock().unwrap().gen
    }

    fn set_value(&self, id: &'static str, v: u16) -> bool {
        self.data.lock().unwrap().values.insert(id, v) != Some(v)
    }

    fn recently_written(&self, id: &str) -> bool {
        let d = self.data.lock().unwrap();
        d.last_write.iter().any(|(k, t)| *k == id && t.elapsed() < WRITE_QUIET)
    }
}

type PendingKey = (String, &'static str);
type PendingMap = HashMap<PendingKey, (Arc<DisplayState>, &'static Feature, u16)>;

pub struct Manager {
    backend: Box<dyn Backend>,
    settings: Mutex<Settings>,
    states: Mutex<Vec<Arc<DisplayState>>>,
    on_event: EventFn,
    stop: AtomicBool,
    live: AtomicBool,
    live_features: Mutex<Vec<&'static Feature>>,
    pending: Mutex<PendingMap>,
    pending_cv: Condvar,
}

impl Manager {
    pub fn new(backend: Box<dyn Backend>, settings: Settings, on_event: EventFn) -> Arc<Manager> {
        Arc::new(Manager {
            backend,
            settings: Mutex::new(settings),
            states: Mutex::new(Vec::new()),
            on_event,
            stop: AtomicBool::new(false),
            live: AtomicBool::new(false),
            live_features: Mutex::new(Vec::new()),
            pending: Mutex::new(HashMap::new()),
            pending_cv: Condvar::new(),
        })
    }

    /// Discover displays and start the hotplug, write and live-value threads.
    pub fn start(self: &Arc<Self>) {
        self.refresh();
        let m = self.clone();
        thread::Builder::new()
            .name("hotplug".into())
            .spawn(move || {
                let mut signature = m.backend.poll_signature();
                while !m.stop.load(Ordering::Relaxed) {
                    thread::sleep(POLL_INTERVAL);
                    let now = m.backend.poll_signature();
                    if now != signature {
                        signature = now;
                        m.refresh();
                    }
                }
            })
            .expect("spawn hotplug thread");
        let m = self.clone();
        thread::Builder::new().name("writes".into()).spawn(move || m.write_loop()).expect("spawn write thread");
        let m = self.clone();
        thread::Builder::new().name("live".into()).spawn(move || m.live_loop()).expect("spawn live thread");
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.pending_cv.notify_all();
    }

    // -- settings ----------------------------------------------------------
    pub fn with_settings<R>(&self, f: impl FnOnce(&mut Settings) -> R) -> R {
        f(&mut self.settings.lock().unwrap())
    }

    pub fn save_settings(&self) {
        let snapshot = self.settings.lock().unwrap().clone();
        if let Err(e) = snapshot.save() {
            log::warn!("could not save settings: {e}");
        }
    }

    // -- display set --------------------------------------------------------
    pub fn refresh(&self) {
        let infos = self.backend.list_displays();
        {
            let mut states = self.states.lock().unwrap();
            let old = std::mem::take(&mut *states);
            for info in infos {
                match old.iter().find(|s| s.info == info) {
                    Some(existing) => states.push(existing.clone()),
                    None => states.push(Arc::new(DisplayState::new(info))),
                }
            }
        }
        (self.on_event)(Event::Displays);
    }

    /// Enabled displays.
    pub fn displays(&self) -> Vec<Arc<DisplayState>> {
        let states = self.states.lock().unwrap().clone();
        self.with_settings(|s| states.into_iter().filter(|d| s.displays.get(d.key()).is_none_or(|p| p.enabled)).collect())
    }

    /// By 1-based index, key, label or EDID name.
    pub fn find_display(&self, selector: Option<&str>) -> Option<Arc<DisplayState>> {
        let list = self.displays();
        let Some(sel) = selector else { return list.into_iter().next() };
        if let Ok(n) = sel.parse::<usize>() {
            if n >= 1 && n <= list.len() {
                return Some(list[n - 1].clone());
            }
        }
        list.into_iter().find(|d| d.key() == sel || d.info.name == sel || self.label(d) == sel)
    }

    pub fn label(&self, st: &DisplayState) -> String {
        let name = self.with_settings(|s| s.displays.get(st.key()).map(|p| p.name.clone()).unwrap_or_default());
        st.info.label(&name)
    }

    // -- session --------------------------------------------------------------
    fn with_session<R>(&self, st: &DisplayState, f: impl FnOnce(&mut dyn Session) -> R) -> Option<R> {
        let mut io = st.io.lock().unwrap();
        if io.session.as_ref().is_some_and(|s| s.is_dead()) {
            let since = *io.dead_since.get_or_insert_with(Instant::now);
            if since.elapsed() < DEAD_COOLDOWN {
                return None;
            }
            io.session = None;
            io.dead_since = None;
        }
        if io.session.is_none() {
            io.session = self.backend.open_session(&st.info);
        }
        io.session.as_mut().map(|s| f(s.as_mut()))
    }

    // -- feature resolution ----------------------------------------------------
    fn learned_entry(&self, st: &DisplayState, f: &Feature) -> Option<Learned> {
        if let Some(l) = self.with_settings(|s| s.displays.get(st.key()).and_then(|d| d.feature_map.get(f.id)).cloned()) {
            return Some(l);
        }
        features::profile_entry(&[&st.info.name, &st.info.model], f.id).map(|e| Learned {
            vcp: e.vcp,
            values: e.values.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        })
    }

    pub fn marked_unsupported(&self, st: &DisplayState, f: &Feature) -> bool {
        self.with_settings(|s| {
            s.displays.get(st.key()).is_some_and(|d| {
                d.unsupported_features.iter().any(|x| x == f.id) && !d.feature_map.contains_key(f.id)
            })
        })
    }

    pub fn resolve(&self, st: &DisplayState, f: &Feature) -> Resolved {
        let names = || -> Vec<String> {
            if f.options.is_empty() {
                f.choices.iter().map(|c| c.name.to_string()).collect()
            } else {
                f.options.iter().map(|n| n.to_string()).collect()
            }
        };
        if let Some(l) = self.learned_entry(st, f) {
            return Resolved { vcp: Some(l.vcp), choices: names().into_iter().map(|n| { let v = l.values.get(&n).copied(); (n, v) }).collect() };
        }
        if f.vendor() && !f.documented {
            return Resolved { vcp: None, choices: names().into_iter().map(|n| (n, None)).collect() };
        }
        let gen = st.gen();
        Resolved { vcp: f.opcode(gen), choices: f.choices.iter().map(|c| (c.name.to_string(), Some(c.code))).collect() }
    }

    pub fn needs_learning(&self, st: &DisplayState, f: &Feature) -> bool {
        f.vendor() && !f.documented && self.learned_entry(st, f).is_none() && !self.marked_unsupported(st, f)
    }

    pub fn status(&self, st: &DisplayState, f: &Feature) -> Status {
        if self.marked_unsupported(st, f) {
            return Status::Unsupported;
        }
        if self.needs_learning(st, f) {
            return Status::Unlearned;
        }
        st.data.lock().unwrap().status.get(f.id).copied().unwrap_or(Status::Ok)
    }

    /// Should the feature be listed for this display at all?
    pub fn visible(&self, st: &DisplayState, f: &Feature) -> bool {
        if let Some(g) = f.only_gen {
            if g != st.gen() {
                return false;
            }
        }
        if self.with_settings(|s| s.displays.get(st.key()).is_some_and(|d| d.hidden_features.iter().any(|x| x == f.id))) {
            return false;
        }
        self.status(st, f) != Status::Unsupported
    }

    /// Name of a choice value, if the display's table has one.
    pub fn choice_name(&self, st: &DisplayState, f: &Feature, value: u16) -> Option<String> {
        self.resolve(st, f).choices.into_iter().find(|(_, c)| *c == Some(value)).map(|(n, _)| n)
    }

    /// Display text for a choice value; values outside the table (e.g. a
    /// picture mode the OSD has but the table does not) read as "Other (0x0E)".
    pub fn choice_label(&self, st: &DisplayState, f: &Feature, value: u16) -> String {
        self.choice_name(st, f, value).unwrap_or_else(|| format!("Other (0x{value:02X})"))
    }

    // -- reads -------------------------------------------------------------------
    pub fn read(&self, st: &DisplayState, f: &'static Feature) -> Option<u16> {
        if f.id == "mute" {
            return st.value("mute");
        }
        let res = self.resolve(st, f);
        let vcp = res.vcp?;
        let started = Instant::now();
        let reply = self.with_session(st, |s| s.get_vcp(vcp)).flatten();
        let learned = self.learned_entry(st, f).is_some();
        let Some(r) = reply else {
            if !learned {
                st.data.lock().unwrap().status.insert(f.id, Status::Unsupported);
            }
            return st.value(f.id);
        };
        let (changed, stale) = {
            let mut d = st.data.lock().unwrap();
            d.status.insert(f.id, Status::Ok);
            d.max.insert(f.id, r.max);
            // the reply predates a write made while we were reading: keep the newer value
            let stale = d.last_write.get(f.id).is_some_and(|t| *t >= started);
            let changed = !stale && d.values.insert(f.id, r.current) != Some(r.current);
            (changed, stale)
        };
        if stale {
            return st.value(f.id);
        }
        if changed {
            self.emit_value(st, f);
        }
        Some(r.current)
    }

    fn emit_value(&self, st: &DisplayState, f: &Feature) {
        (self.on_event)(Event::Value { display: st.key().to_string(), feature: f.id.to_string() });
    }

    pub fn detect_generation(&self, st: &DisplayState) -> Gen {
        let reply = self.with_session(st, |s| s.probe_vcp(VCP_TABLE_VERSION)).flatten();
        let gen = match reply {
            Some(r) if (r.max >> 8) & 0xFF == 2 => Gen::New,
            _ => Gen::Legacy,
        };
        st.data.lock().unwrap().gen = gen;
        gen
    }

    /// Read every feature once so the UI knows what the monitor has.
    pub fn probe(&self, st: &DisplayState) {
        self.detect_generation(st);
        for f in features::all() {
            if (f.kind == Kind::Action && f.id == "mute")
                || self.needs_learning(st, f)
                || self.marked_unsupported(st, f)
                || f.only_gen.is_some_and(|g| g != st.gen())
            {
                continue;
            }
            self.read_once(st, f);
        }
    }

    /// Like [`read`] but without retries: used while probing.
    fn read_once(&self, st: &DisplayState, f: &'static Feature) {
        let Some(vcp) = self.resolve(st, f).vcp else { return };
        let reply = self.with_session(st, |s| s.probe_vcp(vcp)).flatten();
        let learned = self.learned_entry(st, f).is_some();
        let mut d = st.data.lock().unwrap();
        match reply {
            Some(r) => {
                d.status.insert(f.id, Status::Ok);
                d.max.insert(f.id, r.max);
                d.values.insert(f.id, r.current);
            }
            None if !learned => {
                d.status.insert(f.id, Status::Unsupported);
            }
            None => {}
        }
    }

    pub fn info_rows(&self, st: &DisplayState) -> Vec<(String, String)> {
        let mut rows = Vec::new();
        for item in features::INFO_ITEMS {
            if let Some(Some(r)) = self.with_session(st, |s| s.probe_vcp(item.vcp)) {
                rows.push((item.label.to_string(), features::format_info(item.format, r.current)));
            }
        }
        rows
    }

    // -- writes -------------------------------------------------------------------
    /// Blocking write. Returns true when the frame was sent (DDC/CI writes are
    /// not acknowledged; see [`write_confirmed`]).
    pub fn write(&self, st: &DisplayState, f: &'static Feature, value: Option<u16>) -> bool {
        if f.id == "mute" {
            self.toggle_mute(st);
            return true;
        }
        let res = self.resolve(st, f);
        let Some(vcp) = res.vcp else { return false };
        let value = match f.kind {
            Kind::Action => f.action_value,
            Kind::Choice => match value {
                Some(v) if res.choices.iter().any(|(_, c)| *c == Some(v)) => v,
                _ => return false,
            },
            Kind::Slider => match value {
                Some(v) => v.min(st.max(f.id).unwrap_or(f.max)),
                None => return false,
            },
        };
        let ok = self
            .with_session(st, |s| {
                let mut ok = s.set_vcp(vcp, value);
                for extra in f.vcps {
                    if !ok {
                        break;
                    }
                    thread::sleep(Duration::from_millis(20));
                    ok = s.set_vcp(*extra, value);
                }
                if !ok && f.id == "brightness" {
                    // DDC rejected it: dim in software instead
                    s.set_software_level(value.min(100) as u8);
                }
                ok
            })
            .unwrap_or(false);
        if ok && f.kind != Kind::Action {
            {
                let mut d = st.data.lock().unwrap();
                d.last_write.insert(f.id, Instant::now());
            }
            if st.set_value(f.id, value) {
                self.emit_value(st, f);
            }
            self.with_settings(|s| {
                s.display_prefs(st.key()).value_cache.insert(f.id.to_string(), value);
            });
        }
        ok
    }

    /// Write, wait for the monitor to settle, read back. Returns
    /// `(sent, applied)`; `applied` is `None` when it cannot be read back.
    ///
    /// ASUS monitors silently ignore a control the current picture mode locks
    /// (a PA278QV in sRGB mode ignores colour-temperature writes), so a sent
    /// frame is not proof the setting changed.
    pub fn write_confirmed(&self, st: &DisplayState, f: &'static Feature, value: Option<u16>, settle: Duration) -> (bool, Option<bool>) {
        if !self.write(st, f, value) {
            return (false, None);
        }
        if f.kind == Kind::Action {
            return (true, None);
        }
        thread::sleep(settle);
        st.data.lock().unwrap().last_write.remove(f.id);
        let want = match (f.kind, value) {
            (Kind::Slider, Some(v)) => v.min(st.max(f.id).unwrap_or(f.max)),
            (_, Some(v)) => v,
            _ => return (true, None),
        };
        match self.read(st, f) {
            Some(got) => (true, Some(got == want)),
            None => (true, None),
        }
    }

    /// Optimistic, coalesced write for drags: the cached value updates now,
    /// the DDC write follows after the debounce, last value per control wins.
    pub fn queue_write(&self, st: &Arc<DisplayState>, f: &'static Feature, value: u16) {
        let mut targets = vec![(st.clone(), f)];
        let (sync, ) = self.with_settings(|s| (s.global.sync_all, ));
        if sync && SYNC_FEATURES.contains(&f.id) {
            for other in self.displays() {
                if other.key() != st.key() && self.visible(&other, f) {
                    targets.push((other, f));
                }
            }
        }
        for (target, feature) in targets {
            {
                let mut d = target.data.lock().unwrap();
                d.last_write.insert(feature.id, Instant::now());
            }
            if target.set_value(feature.id, value) {
                self.emit_value(&target, feature);
            }
            self.pending.lock().unwrap().insert((target.key().to_string(), feature.id), (target, feature, value));
        }
        self.pending_cv.notify_all();
    }

    fn write_loop(self: Arc<Self>) {
        loop {
            let batch: Vec<_> = {
                let mut p = self.pending.lock().unwrap();
                while p.is_empty() && !self.stop.load(Ordering::Relaxed) {
                    p = self.pending_cv.wait(p).unwrap();
                }
                if self.stop.load(Ordering::Relaxed) {
                    return;
                }
                drop(p);
                let debounce = self.with_settings(|s| s.global.write_debounce_ms);
                thread::sleep(Duration::from_millis(debounce));
                self.pending.lock().unwrap().drain().map(|(_, v)| v).collect()
            };
            for (state, feature, value) in batch {
                self.write(&state, feature, Some(value));
            }
        }
    }

    // -- power & mute ------------------------------------------------------------------
    pub fn power(&self, st: &DisplayState, on: bool) -> bool {
        let code = if on { POWER_ON } else { self.off_code(st) };
        self.with_session(st, |s| s.set_vcp(VCP_POWER_MODE, code)).unwrap_or(false)
    }

    /// Deepest "off" the monitor implements: MCCS 5 = power off, but plenty of
    /// monitors advertise max 4 and would treat 5 as out of range.
    fn off_code(&self, st: &DisplayState) -> u16 {
        let cached = st.data.lock().unwrap().power_max;
        let mx = cached.or_else(|| {
            let r = self.with_session(st, |s| s.probe_vcp(VCP_POWER_MODE)).flatten()?;
            st.data.lock().unwrap().power_max = Some(r.max);
            Some(r.max)
        });
        match mx {
            Some(m) if m > 0 => POWER_OFF.min(m),
            _ => POWER_OFF,
        }
    }

    pub fn toggle_mute(&self, st: &DisplayState) {
        let native = {
            let known = st.data.lock().unwrap().mute_native;
            match known {
                Some(n) => n,
                None => {
                    // not every monitor implements 0x8D (the PA278QV does not)
                    let reply = self.with_session(st, |s| s.probe_vcp(VCP_MUTE)).flatten();
                    let mut d = st.data.lock().unwrap();
                    d.mute_native = Some(reply.is_some());
                    if let Some(r) = reply {
                        d.values.insert("mute", r.current);
                    }
                    reply.is_some()
                }
            }
        };
        if !native {
            return self.toggle_mute_by_volume(st);
        }
        // MCCS 0x8D: 1 = mute, 2 = unmute (ASUS capabilities list `8D(01 02)`)
        let new = if st.value("mute") == Some(MUTE_ON) { MUTE_OFF } else { MUTE_ON };
        if self.with_session(st, |s| s.set_vcp(VCP_MUTE, new)).unwrap_or(false) {
            st.set_value("mute", new);
        }
    }

    /// Mute emulation: volume to 0, remembering the level to restore.
    fn toggle_mute_by_volume(&self, st: &DisplayState) {
        let Some(volume) = features::get("volume") else { return };
        let current = st.value("volume").or_else(|| self.read(st, volume)).unwrap_or(0);
        if current > 0 {
            st.data.lock().unwrap().premute_volume = Some(current);
            self.write(st, volume, Some(0));
            st.set_value("mute", MUTE_ON);
        } else {
            let back = st.data.lock().unwrap().premute_volume.unwrap_or(20);
            self.write(st, volume, Some(back));
            st.set_value("mute", MUTE_OFF);
        }
    }

    // -- live values ----------------------------------------------------------------------
    pub fn set_live(&self, on: bool) {
        self.live.store(on, Ordering::Relaxed);
    }

    pub fn set_live_features(&self, ids: &[&str]) {
        *self.live_features.lock().unwrap() = ids.iter().filter_map(|id| features::get(id)).collect();
    }

    fn live_loop(self: Arc<Self>) {
        while !self.stop.load(Ordering::Relaxed) {
            if !self.live.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(200));
                continue;
            }
            let feats = self.live_features.lock().unwrap().clone();
            for st in self.displays() {
                if st.data.lock().unwrap().retry_after.is_some_and(|t| Instant::now() < t) {
                    continue;
                }
                for f in &feats {
                    if !self.live.load(Ordering::Relaxed) || !self.visible(&st, f) || st.recently_written(f.id)
                        || self.pending.lock().unwrap().contains_key(&(st.key().to_string(), f.id))
                    {
                        continue;
                    }
                    let ok = self.read(&st, f).is_some();
                    let mut d = st.data.lock().unwrap();
                    if ok {
                        d.read_failures = 0;
                    } else {
                        // back off from a display whose reads keep failing
                        d.read_failures += 1;
                        let secs = (2u64 << d.read_failures.min(4)).min(15);
                        d.retry_after = Some(Instant::now() + Duration::from_secs(secs));
                        break;
                    }
                }
            }
            let ms = self.with_settings(|s| s.global.live_interval_ms).max(250);
            thread::sleep(Duration::from_millis(ms));
        }
    }

    // -- learning ---------------------------------------------------------------------------
    /// Value of every register the monitor answers (all opcodes when `codes` is None).
    pub fn scan(&self, st: &DisplayState, codes: Option<&[u8]>) -> Scan {
        self.scan_progress(st, codes, &mut |_, _| {})
    }

    pub fn scan_progress(&self, st: &DisplayState, codes: Option<&[u8]>, progress: &mut dyn FnMut(usize, usize)) -> Scan {
        let all: Vec<u8> = (1..=255u8).collect();
        let list = codes.unwrap_or(&all);
        let mut out = Scan::new();
        for (i, code) in list.iter().enumerate() {
            if let Some(Some(r)) = self.with_session(st, |s| s.probe_vcp(*code)) {
                out.insert(*code, r.current);
            }
            progress(i + 1, list.len());
            thread::sleep(Duration::from_millis(30));
        }
        out
    }

    pub fn save_learned(&self, st: &DisplayState, f: &Feature, vcp: u8, values: BTreeMap<String, u16>) {
        self.with_settings(|s| {
            let p = s.display_prefs(st.key());
            p.unsupported_features.retain(|x| x != f.id);
            p.feature_map.insert(f.id.to_string(), Learned { vcp, values });
        });
        st.data.lock().unwrap().status.remove(f.id);
        self.save_settings();
    }

    pub fn mark_unsupported(&self, st: &DisplayState, f: &Feature) {
        self.with_settings(|s| {
            let p = s.display_prefs(st.key());
            if !p.unsupported_features.iter().any(|x| x == f.id) {
                p.unsupported_features.push(f.id.to_string());
            }
            p.feature_map.remove(f.id);
        });
        st.data.lock().unwrap().status.insert(f.id, Status::Unsupported);
        self.save_settings();
    }

    /// Drop the learned mapping and any "not available" mark.
    pub fn forget(&self, st: &DisplayState, f: &Feature) {
        self.with_settings(|s| {
            let p = s.display_prefs(st.key());
            p.feature_map.remove(f.id);
            p.unsupported_features.retain(|x| x != f.id);
        });
        let mut d = st.data.lock().unwrap();
        d.values.remove(f.id);
        d.status.remove(f.id);
        drop(d);
        self.save_settings();
    }

    // -- presets ----------------------------------------------------------------------------
    pub fn capture_preset(&self, name: &str, icon: &str) -> Preset {
        let mut preset = Preset {
            id: format!("p{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)),
            name: name.to_string(),
            icon: icon.to_string(),
            power: "dontChange".into(),
            ..Preset::default()
        };
        for st in self.displays() {
            let mut values = BTreeMap::new();
            for f in features::all() {
                if f.kind == Kind::Action || f.id == "key_lock" || f.id == "language" || !self.visible(&st, f) {
                    continue;
                }
                if let Some(v) = st.value(f.id) {
                    values.insert(f.id.to_string(), v);
                }
            }
            if !values.is_empty() {
                preset.values.insert(st.key().to_string(), values);
            }
        }
        preset
    }

    pub fn apply_preset(&self, preset: &Preset) {
        let displays = self.displays();
        for (key, values) in &preset.values {
            let st = displays.iter().find(|d| d.key() == key).cloned().or_else(|| (displays.len() == 1).then(|| displays[0].clone()));
            let Some(st) = st else { continue };
            // mode first: it may reset brightness/contrast, which the preset then restores
            let mut order: Vec<_> = values.iter().filter_map(|(id, v)| features::get(id).map(|f| (f, *v))).collect();
            order.sort_by_key(|(f, _)| (f.group != features::Group::Preset) as u8);
            for (f, v) in order {
                self.write(&st, f, Some(v));
                if f.group == features::Group::Preset {
                    thread::sleep(Duration::from_secs(4)); // the monitor settles slowly after a mode switch
                }
            }
            match preset.power.as_str() {
                "turnOff" => {
                    self.power(&st, false);
                }
                "turnOn" => {
                    self.power(&st, true);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
impl Manager {
    fn save_settings_in_memory_for_test(&self, st: &DisplayState, f: &Feature, vcp: u8, values: &[(&str, u16)]) {
        self.with_settings(|s| {
            s.display_prefs(st.key()).feature_map.insert(
                f.id.to_string(),
                Learned { vcp, values: values.iter().map(|(n, v)| (n.to_string(), *v)).collect() },
            );
        });
        st.data.lock().unwrap().status.remove(f.id);
    }

    fn mark_unsupported_in_memory_for_test(&self, st: &DisplayState, f: &Feature) {
        self.with_settings(|s| s.display_prefs(st.key()).unsupported_features.push(f.id.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::fake::FakeBackend;
    use std::sync::Arc as StdArc;

    struct Rig {
        mgr: StdArc<Manager>,
        fake: StdArc<FakeBackend>,
    }

    /// Backend wrapper so the test keeps a handle on the register file.
    struct Shared(StdArc<FakeBackend>);
    impl Backend for Shared {
        fn name(&self) -> &'static str { "fake" }
        fn list_displays(&self) -> Vec<DisplayInfo> { self.0.list_displays() }
        fn open_session(&self, i: &DisplayInfo) -> Option<Box<dyn Session>> { self.0.open_session(i) }
    }

    fn rig() -> Rig {
        let fake = StdArc::new(FakeBackend::sample());
        let mgr = Manager::new(Box::new(Shared(fake.clone())), Settings::default(), Arc::new(|_| {}));
        mgr.refresh();
        Rig { mgr, fake }
    }

    fn feat(id: &str) -> &'static Feature {
        features::get(id).unwrap()
    }

    #[test]
    fn reads_writes_and_clamps_sliders() {
        let r = rig();
        let st = r.mgr.displays()[0].clone();
        assert_eq!(r.mgr.read(&st, feat("brightness")), Some(86));
        assert!(r.mgr.write(&st, feat("brightness"), Some(40)));
        assert_eq!(r.fake.register(0x10).unwrap().0, 40);
        assert!(r.mgr.write(&st, feat("brightness"), Some(250)));
        assert_eq!(r.fake.register(0x10).unwrap().0, 100, "clamped to the monitor's max");
    }

    #[test]
    fn vendor_features_never_write_until_documented_or_learned() {
        let r = rig();
        r.fake.set_register(0xDC, 13, 35);
        let st = r.mgr.displays()[0].clone();
        let preset = feat("preset");
        assert_eq!(r.mgr.status(&st, preset), Status::Unlearned);
        assert!(!r.mgr.write(&st, preset, Some(13)));
        assert_eq!(r.fake.register(0xDC), Some((13, 35)), "untouched");
        r.mgr.save_settings_in_memory_for_test(&st, preset, 0xDC, &[("Standard Mode", 0), ("sRGB Mode", 13)]);
        assert_eq!(r.mgr.status(&st, preset), Status::Ok);
        assert!(r.mgr.write(&st, preset, Some(0)));
        assert_eq!(r.fake.register(0xDC).unwrap().0, 0);
        assert!(!r.mgr.write(&st, preset, Some(99)), "not a learned value");
    }

    #[test]
    fn documented_vendor_features_work_and_hide_when_silent() {
        let r = rig();
        r.fake.set_register(0xE6, 0, 255);
        let st = r.mgr.displays()[0].clone();
        r.mgr.probe(&st);
        let blue = feat("blue_light");
        assert_eq!(r.mgr.status(&st, blue), Status::Ok);
        assert!(r.mgr.write(&st, blue, Some(3)));
        assert_eq!(r.fake.register(0xE6).unwrap().0, 3);
        assert!(!r.mgr.write(&st, blue, Some(9)), "not a level");
        assert!(!r.mgr.visible(&st, feat("trace_free")), "monitor never answered 0xE0");
        assert!(r.mgr.visible(&st, feat("brightness")));
    }

    #[test]
    fn table_generation_selects_the_opcode_and_visibility() {
        let r = rig();
        let st = r.mgr.displays()[0].clone();
        assert_eq!(r.mgr.detect_generation(&st), Gen::Legacy);
        assert_eq!(r.mgr.resolve(&st, feat("power_saving")).vcp, Some(0xEE));
        assert!(r.mgr.visible(&st, feat("ascr")) || r.mgr.status(&st, feat("ascr")) == Status::Ok);
        r.fake.set_register(0xEF, 0x0201, 0x0200);
        assert_eq!(r.mgr.detect_generation(&st), Gen::New);
        assert_eq!(r.mgr.resolve(&st, feat("power_saving")).vcp, Some(0xE1));
        assert!(!r.mgr.visible(&st, feat("ascr")), "0xE1 means Power Saving on the new table");
    }

    #[test]
    fn linked_black_level_writes_three_channels() {
        let r = rig();
        for c in [0x6C, 0x6E, 0x70] {
            r.fake.set_register(c, 50, 100);
        }
        let st = r.mgr.displays()[0].clone();
        assert!(r.mgr.write(&st, feat("black_level"), Some(60)));
        assert!([0x6C, 0x6E, 0x70].iter().all(|c| r.fake.register(*c).unwrap().0 == 60));
    }

    #[test]
    fn pa278qv_preset_table_is_built_in() {
        let r = rig();
        r.fake.rename(0, "PA278QV");
        r.mgr.refresh();
        let st = r.mgr.displays()[0].clone();
        r.fake.set_register(0xDC, 13, 35);
        let preset = feat("preset");
        assert_eq!(r.mgr.status(&st, preset), Status::Ok, "no learn needed");
        assert_eq!(r.mgr.read(&st, preset), Some(13));
        assert_eq!(r.mgr.choice_label(&st, preset, 13), "sRGB Mode");
        assert!(r.mgr.write(&st, preset, Some(24)));
        assert_eq!(r.fake.register(0xDC).unwrap().0, 24);
    }

    #[test]
    fn marking_unsupported_hides_and_forget_restores() {
        let r = rig();
        let st = r.mgr.displays()[0].clone();
        let src = feat("sound_source");
        assert_eq!(r.mgr.status(&st, src), Status::Unlearned);
        r.mgr.mark_unsupported_in_memory_for_test(&st, src);
        assert_eq!(r.mgr.status(&st, src), Status::Unsupported);
        assert!(!r.mgr.visible(&st, src));
        r.mgr.with_settings(|s| s.display_prefs(st.key()).unsupported_features.clear());
        assert_eq!(r.mgr.status(&st, src), Status::Unlearned);
    }

    #[test]
    fn mute_uses_mccs_values_or_falls_back_to_volume() {
        let r = rig();
        let st = r.mgr.displays()[0].clone();
        r.mgr.read(&st, feat("volume"));
        r.mgr.toggle_mute(&st); // no 0x8D on the fake: emulate with volume
        assert_eq!(r.fake.register(0x62).unwrap().0, 0);
        r.mgr.toggle_mute(&st);
        assert_eq!(r.fake.register(0x62).unwrap().0, 50);

        let r = rig();
        r.fake.set_register(0x8D, 2, 2);
        let st = r.mgr.displays()[0].clone();
        r.mgr.toggle_mute(&st);
        assert_eq!(r.fake.register(0x8D).unwrap().0, 1);
        r.mgr.toggle_mute(&st);
        assert_eq!(r.fake.register(0x8D).unwrap().0, 2, "unmute is 2, never 0");
    }

    #[test]
    fn power_off_clamps_to_the_monitors_max() {
        let r = rig();
        r.fake.set_register(0xD6, 1, 4);
        let st = r.mgr.displays()[0].clone();
        assert!(r.mgr.power(&st, false));
        assert_eq!(r.fake.register(0xD6).unwrap().0, 4);
        r.fake.set_register(0xD6, 1, 5);
        let r2 = rig();
        r2.fake.set_register(0xD6, 1, 5);
        let st2 = r2.mgr.displays()[0].clone();
        assert!(r2.mgr.power(&st2, false));
        assert_eq!(r2.fake.register(0xD6).unwrap().0, 5);
        assert!(r2.mgr.power(&st2, true));
        assert_eq!(r2.fake.register(0xD6).unwrap().0, 1);
    }

    #[test]
    fn write_confirmed_detects_a_mode_lock() {
        let r = rig();
        r.fake.set_register(0x14, 5, 12);
        let st = r.mgr.displays()[0].clone();
        let settle = Duration::from_millis(1);
        let (sent, applied) = r.mgr.write_confirmed(&st, feat("color_temp"), Some(4), settle);
        assert_eq!((sent, applied), (true, Some(true)));
        // sRGB mode: the monitor takes the frame and ignores it
        r.fake.lock_register(0x14);
        let (sent, applied) = r.mgr.write_confirmed(&st, feat("color_temp"), Some(8), settle);
        assert_eq!((sent, applied), (true, Some(false)), "the read-back exposes the lock");
        assert_eq!(r.fake.register(0x14).unwrap().0, 4);
    }

    #[test]
    fn queued_writes_coalesce_to_the_last_value() {
        let r = rig();
        r.mgr.start();
        let st = r.mgr.displays()[0].clone();
        for v in [10, 20, 30, 42] {
            r.mgr.queue_write(&st, feat("brightness"), v);
        }
        assert_eq!(st.value("brightness"), Some(42), "optimistic cache");
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(r.fake.register(0x10).unwrap().0, 42);
        r.mgr.stop();
    }

    #[test]
    fn sync_mirrors_brightness_to_other_displays() {
        let r = rig();
        r.mgr.with_settings(|s| s.global.sync_all = true);
        r.mgr.start();
        let all = r.mgr.displays();
        r.mgr.queue_write(&all[0], feat("brightness"), 33);
        assert_eq!(all[1].value("brightness"), Some(33));
        r.mgr.stop();
    }

    #[test]
    fn presets_capture_and_apply() {
        let r = rig();
        let st = r.mgr.displays()[0].clone();
        r.mgr.read(&st, feat("brightness"));
        r.mgr.read(&st, feat("contrast"));
        let preset = r.mgr.capture_preset("Movie", "x");
        assert_eq!(preset.values[st.key()]["brightness"], 86);
        r.mgr.write(&st, feat("brightness"), Some(10));
        r.mgr.apply_preset(&preset);
        assert_eq!(r.fake.register(0x10).unwrap().0, 86);
    }
}

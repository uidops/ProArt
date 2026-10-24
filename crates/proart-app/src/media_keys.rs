//! The Mac's brightness, volume and mute keys for the monitor (like
//! MonitorControl): a keyboard event tap on its own thread, which needs the
//! Accessibility permission. Keys are only taken when the monitor should
//! handle them; everything else passes through to macOS:
//!
//! * brightness keys while the pointer is on a monitor ProArt controls;
//! * volume and mute while sound plays through a monitor.

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::mac;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    BrightnessUp,
    BrightnessDown,
    VolumeUp,
    VolumeDown,
    Mute,
}

impl KeyAction {
    /// From the NX_KEYTYPE_* code of a system-defined key event.
    fn from_nx(code: isize) -> Option<KeyAction> {
        match code {
            0 => Some(KeyAction::VolumeUp),
            1 => Some(KeyAction::VolumeDown),
            2 => Some(KeyAction::BrightnessUp),
            3 => Some(KeyAction::BrightnessDown),
            7 => Some(KeyAction::Mute),
            _ => None,
        }
    }

    fn is_brightness(self) -> bool {
        matches!(self, KeyAction::BrightnessUp | KeyAction::BrightnessDown)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct KeyMsg {
    pub action: KeyAction,
    /// CoreGraphics display the pointer was on.
    pub display: Option<u32>,
}

/// What the tap may take; kept up to date by the main thread.
#[derive(Default)]
pub struct KeyConfig {
    pub brightness: AtomicBool,
    pub volume: AtomicBool,
    /// CoreGraphics ids of the monitors ProArt controls over DDC/CI.
    pub monitors: Mutex<HashSet<u32>>,
}

struct Tap {
    tx: Sender<KeyMsg>,
}

static CONFIG: OnceLock<KeyConfig> = OnceLock::new();
static TAP: OnceLock<Mutex<Tap>> = OnceLock::new();
static PORT: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn config() -> &'static KeyConfig {
    CONFIG.get_or_init(KeyConfig::default)
}

/// Is the event tap installed (the permission was granted)?
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

type TapCallback = extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> *mut c_void;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(tap: u32, place: u32, options: u32, mask: u64, callback: TapCallback, user: *mut c_void) -> *mut c_void;
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
    fn CGEventGetLocation(event: *const c_void) -> mac::CGPoint;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFMachPortCreateRunLoopSource(alloc: *const c_void, port: *mut c_void, order: isize) -> *mut c_void;
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopAddSource(rl: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopRun();
    static kCFRunLoopCommonModes: *const c_void;
}

extern "C" {
    fn objc_getClass(name: *const std::ffi::c_char) -> *mut c_void;
    fn sel_registerName(name: *const std::ffi::c_char) -> *mut c_void;
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

const SESSION_TAP: u32 = 1;
const HEAD_INSERT: u32 = 0;
const DEFAULT_OPTIONS: u32 = 0;
const NX_SYSDEFINED: u32 = 14;
const NX_SUBTYPE_AUX_CONTROL_BUTTONS: i16 = 8;
const TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

/// (action, key down) from a system-defined event, through NSEvent.
unsafe fn decode(event: *mut c_void) -> Option<(KeyAction, bool)> {
    let class = objc_getClass(c"NSEvent".as_ptr());
    let with_cg: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void = std::mem::transmute(objc_msgSend as *const ());
    let ns = with_cg(class, sel_registerName(c"eventWithCGEvent:".as_ptr()), event);
    if ns.is_null() {
        return None;
    }
    let subtype: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i16 = std::mem::transmute(objc_msgSend as *const ());
    if subtype(ns, sel_registerName(c"subtype".as_ptr())) != NX_SUBTYPE_AUX_CONTROL_BUTTONS {
        return None;
    }
    let data1: unsafe extern "C" fn(*mut c_void, *mut c_void) -> isize = std::mem::transmute(objc_msgSend as *const ());
    let data = data1(ns, sel_registerName(c"data1".as_ptr()));
    let code = (data & 0xFFFF_0000) >> 16;
    let state = (data & 0xFF00) >> 8;
    Some((KeyAction::from_nx(code)?, state == 0xA))
}

/// Should ProArt take this key (instead of macOS)?
fn wanted(action: KeyAction, display: Option<u32>) -> bool {
    let c = config();
    let monitors = c.monitors.lock().unwrap();
    if action.is_brightness() {
        c.brightness.load(Ordering::Relaxed) && display.is_some_and(|d| monitors.contains(&d))
    } else {
        c.volume.load(Ordering::Relaxed) && !monitors.is_empty() && mac::sound_on_display()
    }
}

extern "C" fn callback(_proxy: *mut c_void, kind: u32, event: *mut c_void, _user: *mut c_void) -> *mut c_void {
    if kind == TAP_DISABLED_BY_TIMEOUT || kind == TAP_DISABLED_BY_USER_INPUT {
        let port = PORT.load(Ordering::Relaxed);
        if !port.is_null() {
            unsafe { CGEventTapEnable(port, true) };
        }
        return event;
    }
    if kind != NX_SYSDEFINED {
        return event;
    }
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let decoded = decode(event);
        objc_autoreleasePoolPop(pool);
        let Some((action, down)) = decoded else { return event };
        let display = mac::display_at(CGEventGetLocation(event));
        if !wanted(action, display) {
            return event;
        }
        if down {
            if let Some(tap) = TAP.get() {
                let _ = tap.lock().unwrap().tx.send(KeyMsg { action, display });
            }
        }
        // taken: key down and up both stop here
        std::ptr::null_mut()
    }
}

/// Install the tap once it is wanted and permitted; retries until then.
pub fn start(tx: Sender<KeyMsg>) {
    if TAP.set(Mutex::new(Tap { tx })).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("media-keys".into())
        .spawn(|| loop {
            let c = config();
            let wanted = c.brightness.load(Ordering::Relaxed) || c.volume.load(Ordering::Relaxed);
            if wanted && mac::accessibility_trusted(false) {
                let port = unsafe { CGEventTapCreate(SESSION_TAP, HEAD_INSERT, DEFAULT_OPTIONS, 1 << NX_SYSDEFINED, callback, std::ptr::null_mut()) };
                if !port.is_null() {
                    PORT.store(port, Ordering::Relaxed);
                    ACTIVE.store(true, Ordering::Relaxed);
                    unsafe {
                        let source = CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0);
                        CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
                        CGEventTapEnable(port, true);
                        CFRunLoopRun(); // for the life of the app
                    }
                    ACTIVE.store(false, Ordering::Relaxed);
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        })
        .expect("spawn media-keys thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nx_key_codes_map_to_actions() {
        assert_eq!(KeyAction::from_nx(2), Some(KeyAction::BrightnessUp));
        assert_eq!(KeyAction::from_nx(3), Some(KeyAction::BrightnessDown));
        assert_eq!(KeyAction::from_nx(0), Some(KeyAction::VolumeUp));
        assert_eq!(KeyAction::from_nx(1), Some(KeyAction::VolumeDown));
        assert_eq!(KeyAction::from_nx(7), Some(KeyAction::Mute));
        assert_eq!(KeyAction::from_nx(16), None, "play/pause stays with macOS");
    }

    #[test]
    fn keys_pass_through_unless_enabled_and_aimed_at_a_monitor() {
        let c = config();
        c.monitors.lock().unwrap().insert(7);
        assert!(!wanted(KeyAction::BrightnessUp, Some(7)), "off by default");
        c.brightness.store(true, Ordering::Relaxed);
        assert!(wanted(KeyAction::BrightnessUp, Some(7)));
        assert!(!wanted(KeyAction::BrightnessUp, Some(1)), "pointer on the built-in display");
        assert!(!wanted(KeyAction::BrightnessDown, None));
        c.brightness.store(false, Ordering::Relaxed);
        c.monitors.lock().unwrap().clear();
    }
}

//! macOS queries behind the automation features: the frontmost and running
//! apps (per-app presets), Accessibility trust (keyboard keys), whether sound
//! plays through a monitor and which display the pointer is on.
//!
//! AppKit calls here must run on the main thread.

use std::ffi::{c_char, c_void, CStr};

type Id = *mut c_void;

extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Id;
    fn objc_msgSend();
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
    static kAXTrustedCheckOptionPrompt: *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDictionaryCreate(alloc: *const c_void, keys: *const *const c_void, values: *const *const c_void, n: isize, kcb: *const c_void, vcb: *const c_void) -> *const c_void;
    fn CFRelease(cf: *const c_void);
    static kCFBooleanTrue: *const c_void;
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGGetDisplaysWithPoint(point: CGPoint, max: u32, ids: *mut u32, count: *mut u32) -> i32;
}

#[repr(C)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyData(object: u32, address: *const PropertyAddress, qsize: u32, qdata: *const c_void, size: *mut u32, data: *mut c_void) -> i32;
}

const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

unsafe fn send(obj: Id, sel: &CStr) -> Id {
    let f: unsafe extern "C" fn(Id, Id) -> Id = std::mem::transmute(objc_msgSend as *const ());
    f(obj, sel_registerName(sel.as_ptr()))
}

unsafe fn send_index(obj: Id, sel: &CStr, i: usize) -> Id {
    let f: unsafe extern "C" fn(Id, Id, usize) -> Id = std::mem::transmute(objc_msgSend as *const ());
    f(obj, sel_registerName(sel.as_ptr()), i)
}

unsafe fn send_int(obj: Id, sel: &CStr) -> isize {
    let f: unsafe extern "C" fn(Id, Id) -> isize = std::mem::transmute(objc_msgSend as *const ());
    f(obj, sel_registerName(sel.as_ptr()))
}

unsafe fn string(ns: Id) -> Option<String> {
    if ns.is_null() {
        return None;
    }
    let f: unsafe extern "C" fn(Id, Id) -> *const c_char = std::mem::transmute(objc_msgSend as *const ());
    let p = f(ns, sel_registerName(c"UTF8String".as_ptr()));
    (!p.is_null()).then(|| CStr::from_ptr(p).to_string_lossy().into_owned())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppInfo {
    pub bundle_id: String,
    pub name: String,
}

unsafe fn app_info(app: Id) -> Option<AppInfo> {
    if app.is_null() {
        return None;
    }
    let bundle_id = string(send(app, c"bundleIdentifier"))?;
    let name = string(send(app, c"localizedName")).unwrap_or_else(|| bundle_id.clone());
    Some(AppInfo { bundle_id, name })
}

unsafe fn workspace() -> Id {
    send(objc_getClass(c"NSWorkspace".as_ptr()), c"sharedWorkspace")
}

/// The app in front.
pub fn frontmost_app() -> Option<AppInfo> {
    unsafe { app_info(send(workspace(), c"frontmostApplication")) }
}

/// Apps with a Dock icon, by name.
pub fn running_apps() -> Vec<AppInfo> {
    let mut out = Vec::new();
    unsafe {
        let list = send(workspace(), c"runningApplications");
        if list.is_null() {
            return out;
        }
        for i in 0..send_int(list, c"count").max(0) as usize {
            let app = send_index(list, c"objectAtIndex:", i);
            // NSApplicationActivationPolicyRegular
            if send_int(app, c"activationPolicy") == 0 {
                if let Some(info) = app_info(app) {
                    out.push(info);
                }
            }
        }
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out.dedup_by(|a, b| a.bundle_id == b.bundle_id);
    out
}

/// This app's bundle identifier (None when run outside a bundle).
pub fn own_bundle_id() -> Option<String> {
    unsafe { string(send(send(objc_getClass(c"NSBundle".as_ptr()), c"mainBundle"), c"bundleIdentifier")) }
}

/// May this app watch the keyboard? With `prompt`, macOS asks the user and
/// lists the app under Privacy & Security > Accessibility.
pub fn accessibility_trusted(prompt: bool) -> bool {
    unsafe {
        if !prompt {
            return AXIsProcessTrustedWithOptions(std::ptr::null()) != 0;
        }
        let (keys, values) = ([kAXTrustedCheckOptionPrompt], [kCFBooleanTrue]);
        let dict = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks as *const u8 as *const c_void,
            &kCFTypeDictionaryValueCallBacks as *const u8 as *const c_void,
        );
        let trusted = AXIsProcessTrustedWithOptions(dict) != 0;
        CFRelease(dict);
        trusted
    }
}

pub fn open_accessibility_settings() {
    let _ = std::process::Command::new("open").arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility").spawn();
}

/// Does the default sound output go to a monitor (HDMI or DisplayPort)?
pub fn sound_on_display() -> bool {
    unsafe {
        let mut device = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let default_out = PropertyAddress { selector: fourcc(b"dOut"), scope: fourcc(b"glob"), element: 0 };
        if AudioObjectGetPropertyData(1, &default_out, 0, std::ptr::null(), &mut size, &mut device as *mut u32 as *mut c_void) != 0 || device == 0 {
            return false;
        }
        let mut transport = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let addr = PropertyAddress { selector: fourcc(b"tran"), scope: fourcc(b"glob"), element: 0 };
        if AudioObjectGetPropertyData(device, &addr, 0, std::ptr::null(), &mut size, &mut transport as *mut u32 as *mut c_void) != 0 {
            return false;
        }
        transport == fourcc(b"hdmi") || transport == fourcc(b"dprt")
    }
}

/// The CoreGraphics display at a point in global coordinates.
pub fn display_at(point: CGPoint) -> Option<u32> {
    let (mut id, mut n) = (0u32, 0u32);
    let ok = unsafe { CGGetDisplaysWithPoint(point, 1, &mut id, &mut n) } == 0;
    (ok && n > 0).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fourcc_codes_match_coreaudio() {
        assert_eq!(fourcc(b"hdmi"), 0x68646d69);
        assert_eq!(fourcc(b"dprt"), 0x64707274);
    }
}

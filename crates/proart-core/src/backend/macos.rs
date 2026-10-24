//! macOS backend: DDC/CI over IOAVService (Apple Silicon, the same transport
//! m1ddc and MonitorControl use) with the classic IOFBI2C path for Intel Macs,
//! CoreDisplay brightness for the built-in panel and gamma-table software
//! dimming as a last resort.
//!
//! Protocol notes (verified on an Apple M3 Pro + ASUS PA278QV):
//! * each external display is a `DCPAVServiceProxy` IORegistry node, opened
//!   with `IOAVServiceCreateWithService` (exported by IOKit, headerless);
//! * raw I2C with the *7-bit* chip address 0x37 and register 0x51; requests
//!   are XOR-checksummed starting from `0x6E ^ 0x51`, written twice with ~50 ms
//!   settle delays;
//! * `IOAVServiceCopyEDID` maps each proxy to a CoreGraphics display, so
//!   proxy and display stay associated across hotplug.
//!
//! All I/O runs behind [`GuardedSession`], so a display that stops answering
//! can never block the caller for more than a couple of seconds.

#![allow(non_snake_case, non_camel_case_types, clippy::missing_safety_doc)]

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CString};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::Duration;

use super::{Backend, DisplayInfo, GuardedSession, Session, Transport};
use crate::ddc::{
    build_get_request, build_set_request, parse_edid, parse_vcp_reply, parse_vcp_reply_unchecked,
    VcpReply, DDC_CHIP, DDC_REGISTER, VCP_BRIGHTNESS,
};

type IoService = u32;
type IoIterator = u32;
type KernReturn = i32;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const c_void);
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFDataGetLength(data: *const c_void) -> isize;
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingServices(main_port: u32, matching: *mut c_void, existing: *mut IoIterator) -> KernReturn;
    fn IOIteratorNext(iterator: IoIterator) -> IoService;
    fn IOObjectRelease(object: u32) -> KernReturn;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGGetOnlineDisplayList(max: u32, ids: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayIsBuiltin(id: u32) -> u32;
    fn CGDisplayVendorNumber(id: u32) -> u32;
    fn CGDisplayModelNumber(id: u32) -> u32;
    fn CGDisplaySerialNumber(id: u32) -> u32;
    fn CGDisplayPixelsWide(id: u32) -> usize;
    fn CGDisplayPixelsHigh(id: u32) -> usize;
    fn CGGetDisplayTransferByTable(id: u32, cap: u32, r: *mut f32, g: *mut f32, b: *mut f32, n: *mut u32) -> i32;
    fn CGSetDisplayTransferByTable(id: u32, n: u32, r: *const f32, g: *const f32, b: *const f32) -> i32;
    fn CGDisplayRestoreColorSyncSettings();
}

const DDC_WAIT: Duration = Duration::from_millis(50);
const IO_TIMEOUT: Duration = Duration::from_millis(2500);

// ------------------------------------------------------- IOAVService symbols --

type CreateWithServiceFn = unsafe extern "C" fn(*const c_void, IoService) -> *mut c_void;
type CopyEdidFn = unsafe extern "C" fn(*mut c_void, *mut *const c_void) -> i32;
type ReadI2cFn = unsafe extern "C" fn(*mut c_void, u32, u32, *mut c_void, u32) -> i32;
type WriteI2cFn = unsafe extern "C" fn(*mut c_void, u32, u32, *const c_void, u32) -> i32;

struct AvApi {
    create: CreateWithServiceFn,
    copy_edid: CopyEdidFn,
    read: ReadI2cFn,
    write: WriteI2cFn,
}

fn sym(name: &str) -> *mut c_void {
    let c = CString::new(name).unwrap();
    unsafe { libc::dlsym(libc::RTLD_DEFAULT, c.as_ptr()) }
}

/// The IOAVService entry points, resolved at run time: they only exist on
/// macOS builds that have them (Apple Silicon and newer).
fn av_api() -> Option<&'static AvApi> {
    use std::sync::OnceLock;
    static API: OnceLock<Option<AvApi>> = OnceLock::new();
    API.get_or_init(|| {
        let (c, e, r, w) = (
            sym("IOAVServiceCreateWithService"),
            sym("IOAVServiceCopyEDID"),
            sym("IOAVServiceReadI2C"),
            sym("IOAVServiceWriteI2C"),
        );
        if c.is_null() || e.is_null() || r.is_null() || w.is_null() {
            return None;
        }
        unsafe {
            Some(AvApi {
                create: std::mem::transmute::<*mut c_void, CreateWithServiceFn>(c),
                copy_edid: std::mem::transmute::<*mut c_void, CopyEdidFn>(e),
                read: std::mem::transmute::<*mut c_void, ReadI2cFn>(r),
                write: std::mem::transmute::<*mut c_void, WriteI2cFn>(w),
            })
        }
    })
    .as_ref()
}

/// An opened IOAVService plus the IORegistry service it came from.
struct AvHandle {
    av: *mut c_void,
    service: IoService,
}

// The handle is only used behind the per-display GuardedSession thread.
unsafe impl Send for AvHandle {}
unsafe impl Sync for AvHandle {}

impl Drop for AvHandle {
    fn drop(&mut self) {
        unsafe {
            if !self.av.is_null() {
                CFRelease(self.av);
            }
            IOObjectRelease(self.service);
        }
    }
}

struct AvTransport {
    handle: Arc<AvHandle>,
    gamma: Option<Gamma>,
    cg_id: u32,
}

impl AvTransport {
    fn write(&self, payload: &[u8]) -> bool {
        let api = match av_api() {
            Some(a) => a,
            None => return false,
        };
        // m1ddc writes every DDC packet twice
        for _ in 0..2 {
            sleep(DDC_WAIT);
            let err = unsafe {
                (api.write)(self.handle.av, DDC_CHIP, DDC_REGISTER, payload.as_ptr() as *const c_void, payload.len() as u32)
            };
            if err != 0 {
                return false;
            }
        }
        true
    }

    fn read(&self, len: usize) -> Option<Vec<u8>> {
        let api = av_api()?;
        sleep(DDC_WAIT);
        let mut buf = vec![0u8; len];
        let err = unsafe { (api.read)(self.handle.av, DDC_CHIP, DDC_REGISTER, buf.as_mut_ptr() as *mut c_void, len as u32) };
        (err == 0).then_some(buf)
    }
}

impl Transport for AvTransport {
    fn get(&mut self, code: u8) -> Option<VcpReply> {
        if !self.write(&build_get_request(code)) {
            return None;
        }
        parse_vcp_reply(&self.read(12)?, code)
    }

    fn set(&mut self, code: u8, value: u16) -> bool {
        self.write(&build_set_request(code, value))
    }

    fn software_level(&mut self, level: u8) -> bool {
        let g = self.gamma.get_or_insert_with(|| Gamma::capture(self.cg_id));
        g.apply(self.cg_id, level)
    }
}

impl Drop for AvTransport {
    fn drop(&mut self) {
        if let Some(g) = &self.gamma {
            g.restore(self.cg_id);
        }
    }
}

// ------------------------------------------------------------- gamma fallback --

struct Gamma {
    base: Option<[Vec<f32>; 3]>,
}

impl Gamma {
    fn capture(id: u32) -> Gamma {
        let n = 256u32;
        let (mut r, mut g, mut b) = (vec![0f32; 256], vec![0f32; 256], vec![0f32; 256]);
        let mut size = 0u32;
        let ok = unsafe { CGGetDisplayTransferByTable(id, n, r.as_mut_ptr(), g.as_mut_ptr(), b.as_mut_ptr(), &mut size) } == 0;
        Gamma { base: ok.then_some([r, g, b]) }
    }

    fn apply(&self, id: u32, level: u8) -> bool {
        let Some(base) = &self.base else { return false };
        let f = level.min(100) as f32 / 100.0;
        let scaled: Vec<Vec<f32>> = base.iter().map(|ch| ch.iter().map(|v| (v * f).min(1.0)).collect()).collect();
        unsafe { CGSetDisplayTransferByTable(id, 256, scaled[0].as_ptr(), scaled[1].as_ptr(), scaled[2].as_ptr()) == 0 }
    }

    fn restore(&self, _id: u32) {
        if self.base.is_some() {
            unsafe { CGDisplayRestoreColorSyncSettings() };
        }
    }
}

// ------------------------------------------------------- built-in display --

struct CoreDisplayLib {
    set: unsafe extern "C" fn(u32, f64),
    get: unsafe extern "C" fn(u32) -> f64,
}

fn core_display() -> Option<&'static CoreDisplayLib> {
    use std::sync::OnceLock;
    static LIB: OnceLock<Option<CoreDisplayLib>> = OnceLock::new();
    LIB.get_or_init(|| unsafe {
        let path = CString::new("/System/Library/PrivateFrameworks/CoreDisplay.framework/CoreDisplay").unwrap();
        let h = libc::dlopen(path.as_ptr(), libc::RTLD_LAZY);
        if h.is_null() {
            return None;
        }
        let s = libc::dlsym(h, c"CoreDisplay_Display_SetUserBrightness".as_ptr());
        let g = libc::dlsym(h, c"CoreDisplay_Display_GetUserBrightness".as_ptr());
        if s.is_null() || g.is_null() {
            return None;
        }
        Some(CoreDisplayLib {
            set: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(u32, f64)>(s),
            get: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(u32) -> f64>(g),
        })
    })
    .as_ref()
}

struct BuiltinTransport {
    cg_id: u32,
    gamma: Option<Gamma>,
}

impl Transport for BuiltinTransport {
    fn get(&mut self, code: u8) -> Option<VcpReply> {
        if code != VCP_BRIGHTNESS {
            return None;
        }
        let lib = core_display()?;
        let v = unsafe { (lib.get)(self.cg_id) };
        Some(VcpReply { current: (v * 100.0).round().clamp(0.0, 100.0) as u16, max: 100 })
    }

    fn set(&mut self, code: u8, value: u16) -> bool {
        if code != VCP_BRIGHTNESS {
            return false;
        }
        match core_display() {
            Some(lib) => {
                unsafe { (lib.set)(self.cg_id, (value as f64 / 100.0).clamp(0.0, 1.0)) };
                true
            }
            None => self.software_level(value.min(100) as u8),
        }
    }

    fn software_level(&mut self, level: u8) -> bool {
        let g = self.gamma.get_or_insert_with(|| Gamma::capture(self.cg_id));
        g.apply(self.cg_id, level)
    }
}

impl Drop for BuiltinTransport {
    fn drop(&mut self) {
        if let Some(g) = &self.gamma {
            g.restore(self.cg_id);
        }
    }
}

// ------------------------------------------------- legacy IOFBI2C (Intel) --

#[repr(C, packed(4))]
struct IOI2CRequest {
    send_transaction_type: u32,
    reply_transaction_type: u32,
    send_address: u32,
    reply_address: u32,
    send_sub_address: u8,
    reply_sub_address: u8,
    reserved_a: [u8; 2],
    min_reply_delay: u64,
    result: u32,
    comm_flags: u32,
    pad_a: u32,
    send_bytes: u32,
    reserved_b: [u32; 2],
    pad_b: u32,
    reply_bytes: u32,
    completion: *mut c_void,
    send_buffer: u64,
    reply_buffer: u64,
    reserved_c: [u32; 10],
}

const K_IO_I2C_SIMPLE: u32 = 1;
const K_IO_I2C_DDCCI_REPLY: u32 = 2;

type FbCopyI2cFn = unsafe extern "C" fn(IoService, u32, *mut u32) -> i32;
type I2cOpenFn = unsafe extern "C" fn(u32, u32, *mut *mut c_void) -> i32;
type I2cSendFn = unsafe extern "C" fn(*mut c_void, u32, *mut IOI2CRequest) -> i32;
type I2cCloseFn = unsafe extern "C" fn(*mut c_void, u32) -> i32;
type FbCountFn = unsafe extern "C" fn(IoService, *mut u32) -> i32;
type DisplayPortFn = unsafe extern "C" fn(u32) -> u32;

struct LegacyApi {
    copy_iface: FbCopyI2cFn,
    open: I2cOpenFn,
    send: I2cSendFn,
    close: I2cCloseFn,
    count: FbCountFn,
    port: DisplayPortFn,
}

fn legacy_api() -> Option<&'static LegacyApi> {
    use std::sync::OnceLock;
    static API: OnceLock<Option<LegacyApi>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        let names = [
            "IOFBCopyI2CInterfaceForBus",
            "IOI2CInterfaceOpen",
            "IOI2CSendRequest",
            "IOI2CInterfaceClose",
            "IOFBGetI2CInterfaceCount",
            "CGDisplayIOServicePort",
        ];
        let p: Vec<*mut c_void> = names.iter().map(|n| sym(n)).collect();
        if p.iter().any(|x| x.is_null()) {
            return None;
        }
        Some(LegacyApi {
            copy_iface: std::mem::transmute::<*mut c_void, FbCopyI2cFn>(p[0]),
            open: std::mem::transmute::<*mut c_void, I2cOpenFn>(p[1]),
            send: std::mem::transmute::<*mut c_void, I2cSendFn>(p[2]),
            close: std::mem::transmute::<*mut c_void, I2cCloseFn>(p[3]),
            count: std::mem::transmute::<*mut c_void, FbCountFn>(p[4]),
            port: std::mem::transmute::<*mut c_void, DisplayPortFn>(p[5]),
        })
    })
    .as_ref()
}

struct LegacyTransport {
    connect: *mut c_void,
}

unsafe impl Send for LegacyTransport {}

impl LegacyTransport {
    fn open(cg_id: u32) -> Option<LegacyTransport> {
        let api = legacy_api()?;
        unsafe {
            let service = (api.port)(cg_id);
            if service == 0 {
                return None;
            }
            let mut buses = 0u32;
            if (api.count)(service, &mut buses) != 0 || buses == 0 {
                return None;
            }
            let mut iface = 0u32;
            if (api.copy_iface)(service, 0, &mut iface) != 0 {
                return None;
            }
            let mut connect: *mut c_void = std::ptr::null_mut();
            let kr = (api.open)(iface, 0, &mut connect);
            IOObjectRelease(iface);
            (kr == 0 && !connect.is_null()).then_some(LegacyTransport { connect })
        }
    }

    fn blank_request() -> IOI2CRequest {
        unsafe { std::mem::zeroed() }
    }
}

impl Transport for LegacyTransport {
    fn get(&mut self, code: u8) -> Option<VcpReply> {
        let api = legacy_api()?;
        let chk = 0x6E ^ 0x51 ^ 0x82 ^ 0x01 ^ code;
        let send = [0x51u8, 0x82, 0x01, code, chk];
        let mut reply = [0u8; 11];
        let mut req = Self::blank_request();
        req.send_address = 0x6E;
        req.send_transaction_type = K_IO_I2C_SIMPLE;
        req.send_buffer = send.as_ptr() as u64;
        req.send_bytes = 5;
        req.reply_address = 0x6F;
        req.reply_transaction_type = K_IO_I2C_DDCCI_REPLY;
        req.reply_buffer = reply.as_mut_ptr() as u64;
        req.reply_bytes = 11;
        req.min_reply_delay = 40 * 1_000_000;
        let kr = unsafe { (api.send)(self.connect, 0, &mut req) };
        let result = req.result;
        if kr != 0 || result != 0 {
            return None;
        }
        // whole-frame checksum (seed 0x6F) verified here, as the Python port did
        if reply.iter().fold(0u8, |a, b| a ^ b) != 0x6F {
            return None;
        }
        parse_vcp_reply_unchecked(&reply, code)
    }

    fn set(&mut self, code: u8, value: u16) -> bool {
        let Some(api) = legacy_api() else { return false };
        let data = [0x51u8, 0x84, 0x03, code, (value >> 8) as u8, value as u8];
        let chk = data.iter().fold(0x6Eu8, |a, b| a ^ b);
        let send = [data[0], data[1], data[2], data[3], data[4], data[5], chk];
        let mut req = Self::blank_request();
        req.send_address = 0x6E;
        req.send_transaction_type = K_IO_I2C_SIMPLE;
        req.send_buffer = send.as_ptr() as u64;
        req.send_bytes = 7;
        let kr = unsafe { (api.send)(self.connect, 0, &mut req) };
        sleep(Duration::from_millis(30));
        let result = req.result;
        kr == 0 && result == 0
    }
}

impl Drop for LegacyTransport {
    fn drop(&mut self) {
        if let Some(api) = legacy_api() {
            unsafe { (api.close)(self.connect, 0) };
        }
    }
}

// ----------------------------------------------------------------- backend --

pub struct MacBackend {
    /// EDID identity -> opened AVService, refreshed on every `list_displays`.
    proxies: Mutex<HashMap<(u16, u16, u32), Arc<AvHandle>>>,
}

impl MacBackend {
    pub fn new() -> Self {
        MacBackend { proxies: Mutex::new(HashMap::new()) }
    }

    fn online_displays() -> Vec<u32> {
        let mut ids = [0u32; 16];
        let mut count = 0u32;
        if unsafe { CGGetOnlineDisplayList(16, ids.as_mut_ptr(), &mut count) } != 0 {
            return Vec::new();
        }
        ids[..(count as usize).min(16)].to_vec()
    }

    /// Enumerate `DCPAVServiceProxy` nodes, keyed by EDID identity.
    fn scan_proxies() -> HashMap<(u16, u16, u32), (Arc<AvHandle>, crate::ddc::Edid)> {
        let mut out = HashMap::new();
        let Some(api) = av_api() else { return out };
        unsafe {
            let matching = IOServiceMatching(c"DCPAVServiceProxy".as_ptr());
            let mut it: IoIterator = 0;
            if matching.is_null() || IOServiceGetMatchingServices(0, matching, &mut it) != 0 {
                return out;
            }
            loop {
                let svc = IOIteratorNext(it);
                if svc == 0 {
                    break;
                }
                let av = (api.create)(std::ptr::null(), svc);
                if av.is_null() {
                    IOObjectRelease(svc);
                    continue;
                }
                let handle = Arc::new(AvHandle { av, service: svc });
                let mut edid_ref: *const c_void = std::ptr::null();
                let edid = if (api.copy_edid)(av, &mut edid_ref) == 0 && !edid_ref.is_null() {
                    let len = CFDataGetLength(edid_ref).max(0) as usize;
                    let bytes = std::slice::from_raw_parts(CFDataGetBytePtr(edid_ref), len).to_vec();
                    CFRelease(edid_ref);
                    parse_edid(&bytes)
                } else {
                    None
                };
                if let Some(e) = edid {
                    out.insert((e.vendor_id, e.product_id, e.serial32), (handle, e));
                }
            }
            IOObjectRelease(it);
        }
        out
    }
}

impl Default for MacBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for MacBackend {
    fn name(&self) -> &'static str {
        "macos"
    }

    fn list_displays(&self) -> Vec<DisplayInfo> {
        let ids = Self::online_displays();
        let found = Self::scan_proxies();
        let mut cache: HashMap<(u16, u16, u32), Arc<AvHandle>> = HashMap::new();
        let mut out = Vec::new();
        for did in ids {
            let builtin = unsafe { CGDisplayIsBuiltin(did) } != 0;
            let (vendor, model, serial) =
                unsafe { (CGDisplayVendorNumber(did) as u16, CGDisplayModelNumber(did) as u16, CGDisplaySerialNumber(did)) };
            let mut info = DisplayInfo {
                key: format!("cg-{did}"),
                name: if builtin { "Built-in Display".into() } else { "External Display".into() },
                vendor_id: vendor,
                product_id: model,
                builtin,
                width: unsafe { CGDisplayPixelsWide(did) } as u32,
                height: unsafe { CGDisplayPixelsHigh(did) } as u32,
                backend: "macos".into(),
                ..DisplayInfo::default()
            };
            info.extra.insert("cg_id".into(), did.to_string());
            if !builtin {
                let mut matched = found.get(&(vendor, model, serial));
                if matched.is_none() {
                    // EDID serial32 sometimes differs: match vendor+product when unambiguous
                    let cands: Vec<_> = found.iter().filter(|((v, m, _), _)| (*v, *m) == (vendor, model)).collect();
                    if cands.len() == 1 {
                        matched = Some(cands[0].1);
                    }
                }
                if let Some((handle, edid)) = matched {
                    if !edid.name.is_empty() {
                        info.name = edid.name.clone();
                    }
                    info.model = info.name.clone();
                    info.serial = edid.serial_text.clone();
                    info.key = format!("edid-{vendor:04x}-{model:04x}-{serial:x}");
                    let id = (edid.vendor_id, edid.product_id, edid.serial32);
                    info.extra.insert("av".into(), format!("{:04x}-{:04x}-{:x}", id.0, id.1, id.2));
                    cache.insert(id, handle.clone());
                }
            }
            if info.model.is_empty() {
                info.model = info.name.clone();
            }
            out.push(info);
        }
        *self.proxies.lock().unwrap() = cache;
        out
    }

    fn poll_signature(&self) -> String {
        // cheap: no IORegistry walk, so polling does not churn service handles
        Self::online_displays()
            .iter()
            .map(|&d| unsafe {
                format!("{d}:{:x}:{:x}:{:x}:{}x{}", CGDisplayVendorNumber(d), CGDisplayModelNumber(d),
                        CGDisplaySerialNumber(d), CGDisplayPixelsWide(d), CGDisplayPixelsHigh(d))
            })
            .collect::<Vec<_>>()
            .join("|")
    }

    fn open_session(&self, info: &DisplayInfo) -> Option<Box<dyn Session>> {
        let cg_id: u32 = info.extra.get("cg_id")?.parse().ok()?;
        if info.builtin {
            return Some(Box::new(GuardedSession::new(BuiltinTransport { cg_id, gamma: None }, IO_TIMEOUT, 0)));
        }
        // 1) Apple Silicon: IOAVService
        if let Some(av) = info.extra.get("av") {
            let key = {
                let mut it = av.split('-');
                let v = u16::from_str_radix(it.next()?, 16).ok()?;
                let p = u16::from_str_radix(it.next()?, 16).ok()?;
                let s = u32::from_str_radix(it.next()?, 16).ok()?;
                (v, p, s)
            };
            if let Some(handle) = self.proxies.lock().unwrap().get(&key).cloned() {
                return Some(Box::new(GuardedSession::new(
                    AvTransport { handle, gamma: None, cg_id },
                    IO_TIMEOUT,
                    1,
                )));
            }
        }
        // 2) Intel / classic IOFBI2C
        LegacyTransport::open(cg_id).map(|t| Box::new(GuardedSession::new(t, IO_TIMEOUT, 1)) as Box<dyn Session>)
    }
}

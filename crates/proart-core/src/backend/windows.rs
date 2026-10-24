//! Windows backend: the high-level monitor configuration API in `dxva2.dll`
//! (`Get/SetVCPFeature`, capabilities request) - the same calls ASUS
//! DisplayWidget Center makes. No driver or extra install is needed.

use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows_sys::Win32::Devices::Display::{
    CapabilitiesRequestAndCapabilitiesReply, DestroyPhysicalMonitor, GetCapabilitiesStringLength,
    GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR, GetVCPFeatureAndVCPFeatureReply,
    SetVCPFeature, PHYSICAL_MONITOR,
};
use windows_sys::Win32::Foundation::{HANDLE, LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, DISPLAY_DEVICEW, HDC, HMONITOR, MONITORINFO,
    MONITORINFOEXW,
};

use super::{Backend, DisplayInfo, GuardedSession, Session, Transport};
use crate::ddc::VcpReply;

const IO_TIMEOUT: Duration = Duration::from_millis(5000);

/// An open physical-monitor handle, destroyed when the last user drops it.
struct PhysHandle(HANDLE);

// dxva2 handles are plain tokens; calls are serialised by the per-display worker thread.
unsafe impl Send for PhysHandle {}
unsafe impl Sync for PhysHandle {}

impl Drop for PhysHandle {
    fn drop(&mut self) {
        unsafe { DestroyPhysicalMonitor(self.0) };
    }
}

struct WinTransport {
    handle: Arc<PhysHandle>,
}

impl Transport for WinTransport {
    fn get(&mut self, code: u8) -> Option<VcpReply> {
        let (mut vct, mut cur, mut max) = (0i32, 0u32, 0u32);
        let ok = unsafe { GetVCPFeatureAndVCPFeatureReply(self.handle.0, code, &mut vct, &mut cur, &mut max) };
        (ok != 0).then(|| VcpReply { current: cur.min(0xFFFF) as u16, max: if max == 0 { 0xFF } else { max.min(0xFFFF) as u16 } })
    }

    fn set(&mut self, code: u8, value: u16) -> bool {
        unsafe { SetVCPFeature(self.handle.0, code, value as u32) != 0 }
    }

    fn capabilities(&mut self) -> Option<String> {
        capabilities_of(self.handle.0)
    }
}

fn capabilities_of(handle: HANDLE) -> Option<String> {
    let mut len = 0u32;
    if unsafe { GetCapabilitiesStringLength(handle, &mut len) } == 0 || len == 0 || len > 8192 {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    if unsafe { CapabilitiesRequestAndCapabilitiesReply(handle, buf.as_mut_ptr(), len) } == 0 {
        return None;
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    Some(String::from_utf8_lossy(&buf[..end]).into_owned())
}

fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|c| *c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

unsafe extern "system" fn collect_monitor(hmon: HMONITOR, _hdc: HDC, _rc: *mut RECT, data: LPARAM) -> i32 {
    let list = &mut *(data as *mut Vec<(HMONITOR, String, u32, u32)>);
    let mut info: MONITORINFOEXW = std::mem::zeroed();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(hmon, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) != 0 {
        let rc = info.monitorInfo.rcMonitor;
        list.push((hmon, wide_to_string(&info.szDevice), (rc.right - rc.left).max(0) as u32, (rc.bottom - rc.top).max(0) as u32));
    }
    1
}

/// Friendly name and PnP id of the monitor attached to a display device
/// (`\\.\DISPLAY1`), e.g. ("ASUS PA278QV", "AUS2701").
fn monitor_identity(device: &str) -> (String, String) {
    let wide: Vec<u16> = device.encode_utf16().chain(std::iter::once(0)).collect();
    let mut dd: DISPLAY_DEVICEW = unsafe { std::mem::zeroed() };
    dd.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
    if unsafe { EnumDisplayDevicesW(wide.as_ptr(), 0, &mut dd, 0) } == 0 {
        return (String::new(), String::new());
    }
    let name = wide_to_string(&dd.DeviceString);
    let id = wide_to_string(&dd.DeviceID); // MONITOR\AUS2701\{guid}\0004
    let pnp = id.split('\\').nth(1).unwrap_or("").to_string();
    (name, pnp)
}

struct Cached {
    handle: Arc<PhysHandle>,
}

pub struct WindowsBackend {
    /// display device name -> its physical monitor handle
    cache: Mutex<std::collections::HashMap<String, Cached>>,
}

impl WindowsBackend {
    pub fn new() -> Self {
        WindowsBackend { cache: Mutex::new(Default::default()) }
    }
}

impl Default for WindowsBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for WindowsBackend {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn list_displays(&self) -> Vec<DisplayInfo> {
        let mut monitors: Vec<(HMONITOR, String, u32, u32)> = Vec::new();
        unsafe { EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(collect_monitor), &mut monitors as *mut _ as LPARAM) };
        let mut cache = self.cache.lock().unwrap();
        let mut fresh = std::collections::HashMap::new();
        let mut out = Vec::new();
        for (hmon, device, width, height) in monitors {
            let mut count = 0u32;
            if unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(hmon, &mut count) } == 0 || count == 0 {
                continue;
            }
            // reuse the handle of a display we already opened
            let handle = match cache.remove(&device) {
                Some(c) => c.handle,
                None => {
                    let mut arr: Vec<PHYSICAL_MONITOR> = (0..count).map(|_| unsafe { std::mem::zeroed() }).collect();
                    if unsafe { GetPhysicalMonitorsFromHMONITOR(hmon, count, arr.as_mut_ptr()) } == 0 {
                        continue;
                    }
                    // keep the first, release the rest
                    for extra in arr.iter().skip(1) {
                        unsafe { DestroyPhysicalMonitor(extra.hPhysicalMonitor) };
                    }
                    Arc::new(PhysHandle(arr[0].hPhysicalMonitor))
                }
            };
            let desc = String::new();
            let (friendly, pnp) = monitor_identity(&device);
            let caps = capabilities_of(handle.0);
            let model = caps
                .as_deref()
                .and_then(|c| c.split("model(").nth(1))
                .and_then(|r| r.split(')').next())
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(String::from);
            let generic = friendly.is_empty() || friendly.contains("Generic");
            let name = model.clone().filter(|m| !m.eq_ignore_ascii_case("ProArtDisplay")).unwrap_or_else(|| {
                if generic {
                    if desc.is_empty() { "Monitor".into() } else { desc.clone() }
                } else {
                    friendly.clone()
                }
            });
            let mut info = DisplayInfo {
                key: format!("win-{}-{}", pnp.to_lowercase(), device.trim_start_matches('\\').trim_start_matches('.').trim_start_matches('\\').to_lowercase()),
                name: name.clone(),
                model: name,
                width,
                height,
                backend: "windows".into(),
                ..DisplayInfo::default()
            };
            info.extra.insert("device".into(), device.clone());
            info.extra.insert("pnp".into(), pnp);
            fresh.insert(device, Cached { handle });
            out.push(info);
        }
        *cache = fresh; // handles of unplugged displays drop here
        out
    }

    fn poll_signature(&self) -> String {
        let mut monitors: Vec<(HMONITOR, String, u32, u32)> = Vec::new();
        unsafe { EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(collect_monitor), &mut monitors as *mut _ as LPARAM) };
        monitors.iter().map(|(_, d, w, h)| format!("{d}:{w}x{h}")).collect::<Vec<_>>().join("|")
    }

    fn open_session(&self, info: &DisplayInfo) -> Option<Box<dyn Session>> {
        let cache = self.cache.lock().unwrap();
        let handle = cache.get(info.extra.get("device")?)?.handle.clone();
        Some(Box::new(GuardedSession::new(WinTransport { handle }, IO_TIMEOUT, 1)))
    }
}

// keep `c_void` referenced for platforms where HANDLE is not already that type
#[allow(dead_code)]
type _Unused = *mut c_void;

//! Linux backend: DDC/CI straight over `/dev/i2c-N` (no ddcutil needed).
//!
//! Connected DRM connectors are found in sysfs; each exposes its EDID and the
//! I2C bus of its DDC channel (a `ddc` symlink, or an `i2c-N` child). Needs the
//! `i2c-dev` kernel module and permission on `/dev/i2c-*` (see
//! `packaging/linux/60-proart-i2c.rules`). The built-in laptop panel is driven
//! through `/sys/class/backlight` instead.

use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use super::{Backend, DisplayInfo, GuardedSession, Session, Transport};
use crate::ddc::{build_get_request, build_set_request, parse_edid, parse_vcp_reply, VcpReply, DDC_CHIP, VCP_BRIGHTNESS};

const I2C_SLAVE: libc::c_ulong = 0x0703;
const I2C_SLAVE_FORCE: libc::c_ulong = 0x0706;
const IO_TIMEOUT: Duration = Duration::from_millis(3000);
const DRM: &str = "/sys/class/drm";

pub struct LinuxBackend;

impl LinuxBackend {
    pub fn new() -> Self {
        LinuxBackend
    }
}

impl Default for LinuxBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn read_trim(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// `/sys/class/drm/card0-HDMI-A-1` style directories whose status is "connected".
fn connected_connectors() -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(DRM) else { return Vec::new() };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("card") && n.contains('-')))
        .filter(|p| read_trim(&p.join("status")).as_deref() == Some("connected"))
        .collect();
    out.sort();
    out
}

/// The I2C bus number of a connector's DDC channel.
fn ddc_bus(dir: &Path) -> Option<u32> {
    if let Ok(link) = fs::read_link(dir.join("ddc")) {
        if let Some(n) = link.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix("i2c-")) {
            return n.parse().ok();
        }
    }
    fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        e.file_name().to_str().and_then(|n| n.strip_prefix("i2c-")).and_then(|n| n.parse().ok())
    })
}

fn first_mode(dir: &Path) -> (u32, u32) {
    let mode = fs::read_to_string(dir.join("modes")).unwrap_or_default();
    let line = mode.lines().next().unwrap_or("");
    let mut it = line.split('x');
    let w = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let h = it.next().and_then(|v| v.split(['i', 'p']).next()).and_then(|v| v.parse().ok()).unwrap_or(0);
    (w, h)
}

fn backlight_dir() -> Option<PathBuf> {
    fs::read_dir("/sys/class/backlight").ok()?.flatten().map(|e| e.path()).find(|p| p.join("max_brightness").exists())
}

impl Backend for LinuxBackend {
    fn name(&self) -> &'static str {
        "linux"
    }

    fn list_displays(&self) -> Vec<DisplayInfo> {
        let mut out: Vec<DisplayInfo> = Vec::new();
        for dir in connected_connectors() {
            let conn = dir.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            let builtin = conn.contains("eDP") || conn.contains("LVDS") || conn.contains("DSI");
            let edid = fs::read(dir.join("edid")).ok().and_then(|b| parse_edid(&b));
            let (width, height) = first_mode(&dir);
            let mut info = DisplayInfo {
                key: format!("drm-{conn}"),
                name: if builtin { "Built-in Display".into() } else { "External Display".into() },
                builtin,
                width,
                height,
                backend: "linux".into(),
                ..DisplayInfo::default()
            };
            if let Some(e) = &edid {
                info.vendor_id = e.vendor_id;
                info.product_id = e.product_id;
                if !e.name.is_empty() {
                    info.name = e.name.clone();
                }
                info.serial = e.serial_text.clone();
                let key = format!("edid-{:04x}-{:04x}-{:x}", e.vendor_id, e.product_id, e.serial32);
                // two identical monitors would share a key: keep them apart by connector
                info.key = if out.iter().any(|d| d.key == key) { format!("{key}-{conn}") } else { key };
            }
            info.model = info.name.clone();
            if builtin {
                if let Some(bl) = backlight_dir() {
                    info.extra.insert("backlight".into(), bl.display().to_string());
                }
            } else if let Some(bus) = ddc_bus(&dir) {
                info.extra.insert("bus".into(), bus.to_string());
            } else {
                continue; // no DDC channel: nothing to control
            }
            out.push(info);
        }
        out
    }

    fn poll_signature(&self) -> String {
        connected_connectors()
            .iter()
            .map(|p| p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string())
            .collect::<Vec<_>>()
            .join("|")
    }

    fn open_session(&self, info: &DisplayInfo) -> Option<Box<dyn Session>> {
        if let Some(bl) = info.extra.get("backlight") {
            return Some(Box::new(GuardedSession::new(BacklightTransport { dir: PathBuf::from(bl) }, IO_TIMEOUT, 0)));
        }
        let bus: u32 = info.extra.get("bus")?.parse().ok()?;
        I2cTransport::open(bus).map(|t| Box::new(GuardedSession::new(t, IO_TIMEOUT, 1)) as Box<dyn Session>)
    }
}

// ------------------------------------------------------------------ i2c-dev --

struct I2cTransport {
    fd: libc::c_int,
}

impl I2cTransport {
    fn open(bus: u32) -> Option<I2cTransport> {
        let path = CString::new(format!("/dev/i2c-{bus}")).ok()?;
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR) };
        if fd < 0 {
            log::warn!("cannot open /dev/i2c-{bus}: is the i2c-dev module loaded and are you in the i2c group?");
            return None;
        }
        let ok = unsafe { libc::ioctl(fd, I2C_SLAVE, DDC_CHIP as libc::c_ulong) } >= 0
            || unsafe { libc::ioctl(fd, I2C_SLAVE_FORCE, DDC_CHIP as libc::c_ulong) } >= 0;
        if !ok {
            unsafe { libc::close(fd) };
            return None;
        }
        Some(I2cTransport { fd })
    }

    /// DDC/CI request framing on the wire: `51 <0x80|len> payload chk`.
    fn write_frame(&self, body: &[u8]) -> bool {
        // the build_* helpers produce `<payload> chk` for the Apple register
        // layout; rebuild with the on-the-wire host address and its checksum
        let payload = &body[..body.len() - 1];
        let mut frame = Vec::with_capacity(payload.len() + 2);
        frame.push(0x51);
        frame.push(0x80 | payload.len() as u8);
        frame.extend_from_slice(payload);
        let chk = frame.iter().fold(0x6Eu8, |a, b| a ^ b);
        frame.push(chk);
        let n = unsafe { libc::write(self.fd, frame.as_ptr() as *const libc::c_void, frame.len()) };
        n == frame.len() as isize
    }
}

impl Transport for I2cTransport {
    fn get(&mut self, code: u8) -> Option<VcpReply> {
        for _ in 0..3 {
            if self.write_frame(&build_get_request(code)) {
                sleep(Duration::from_millis(40));
                let mut buf = [0u8; 12];
                let n = unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut libc::c_void, 11) };
                if n >= 11 {
                    if let Some(r) = parse_vcp_reply(&buf[..11], code) {
                        return Some(r);
                    }
                }
            }
            sleep(Duration::from_millis(40));
        }
        None
    }

    fn set(&mut self, code: u8, value: u16) -> bool {
        for _ in 0..2 {
            if self.write_frame(&build_set_request(code, value)) {
                sleep(Duration::from_millis(50));
                return true;
            }
            sleep(Duration::from_millis(40));
        }
        false
    }
}

impl Drop for I2cTransport {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

// ---------------------------------------------------------------- backlight --

struct BacklightTransport {
    dir: PathBuf,
}

impl Transport for BacklightTransport {
    fn get(&mut self, code: u8) -> Option<VcpReply> {
        if code != VCP_BRIGHTNESS {
            return None;
        }
        let cur: f64 = read_trim(&self.dir.join("brightness"))?.parse().ok()?;
        let max: f64 = read_trim(&self.dir.join("max_brightness"))?.parse().ok()?;
        Some(VcpReply { current: (cur / max.max(1.0) * 100.0).round() as u16, max: 100 })
    }

    fn set(&mut self, code: u8, value: u16) -> bool {
        if code != VCP_BRIGHTNESS {
            return false;
        }
        let Some(max) = read_trim(&self.dir.join("max_brightness")).and_then(|m| m.parse::<f64>().ok()) else { return false };
        let raw = (value.min(100) as f64 / 100.0 * max).round() as u64;
        fs::write(self.dir.join("brightness"), raw.to_string()).is_ok()
    }
}

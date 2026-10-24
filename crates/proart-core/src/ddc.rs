//! DDC/CI framing and the VCP opcodes the app knows by name.
//!
//! The macOS backend talks raw I2C (IOAVService) and so builds and parses the
//! packets itself; Windows (dxva2) and Linux (i2c-dev / ddcutil) frame them
//! for us or use the same helpers.

/// DDC/CI 7-bit chip address and the host register offset (m1ddc / MonitorControl).
pub const DDC_CHIP: u32 = 0x37;
pub const DDC_REGISTER: u32 = 0x51;
/// 8-bit write address of the display; seeds every request checksum.
pub const DDC_WRITE_ADDR: u8 = 0x6E;

pub const VCP_BRIGHTNESS: u8 = 0x10;
pub const VCP_CONTRAST: u8 = 0x12;
pub const VCP_INPUT_SOURCE: u8 = 0x60;
pub const VCP_VOLUME: u8 = 0x62;
pub const VCP_MUTE: u8 = 0x8D;
pub const VCP_POWER_MODE: u8 = 0xD6;
pub const VCP_TABLE_VERSION: u8 = 0xEF;

/// MCCS power-mode values (0xD6): 1 = on, 4 = soft off, 5 = power off.
pub const POWER_ON: u16 = 1;
pub const POWER_OFF: u16 = 5;

/// MCCS 0x8D: 1 = mute, 2 = unmute.
pub const MUTE_ON: u16 = 1;
pub const MUTE_OFF: u16 = 2;

/// A VCP feature reply: current value and the maximum the monitor reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VcpReply {
    pub current: u16,
    pub max: u16,
}

fn xor(seed: u8, bytes: &[u8]) -> u8 {
    bytes.iter().fold(seed, |acc, b| acc ^ b)
}

/// "Get VCP feature" request body as written to register 0x51 (checksum last).
pub fn build_get_request(code: u8) -> [u8; 4] {
    let body = [0x82, 0x01, code];
    let chk = xor(DDC_WRITE_ADDR ^ DDC_REGISTER as u8, &body);
    [body[0], body[1], body[2], chk]
}

/// "Set VCP feature" request body (checksum last).
pub fn build_set_request(code: u8, value: u16) -> [u8; 6] {
    let body = [0x84, 0x03, code, (value >> 8) as u8, value as u8];
    let chk = xor(DDC_WRITE_ADDR ^ DDC_REGISTER as u8, &body);
    [body[0], body[1], body[2], body[3], body[4], chk]
}

/// Parse a 11+ byte "Get VCP feature" reply.
///
/// Layout: `6E 88 02 <result> <vcp> <type> <max hi/lo> <cur hi/lo> <chk>`
/// with `chk = 0x50 ^ bytes 0..10`. The echoed opcode and the checksum reject
/// stale or garbled replies (another DDC client talking at the same time).
/// Non-continuous features report max 0 in some firmwares, so a zero max or a
/// "set parameter" type falls back to 0xFF like the Python implementation.
pub fn parse_vcp_reply(raw: &[u8], code: u8) -> Option<VcpReply> {
    if raw.len() < 11 || raw[2] != 0x02 || raw[3] != 0x00 || raw[4] != code {
        return None;
    }
    if xor(0x50, &raw[..10]) != raw[10] {
        return None;
    }
    let vtype = raw[5];
    let max = u16::from_be_bytes([raw[6], raw[7]]);
    let current = u16::from_be_bytes([raw[8], raw[9]]);
    Some(VcpReply { current, max: if vtype == 0 && max != 0 { max } else { 0xFF } })
}

/// Parse a reply whose frame checksum was already verified by the transport
/// (the legacy Intel I2C path checks the whole frame itself).
pub fn parse_vcp_reply_unchecked(raw: &[u8], code: u8) -> Option<VcpReply> {
    if raw.len() < 11 || raw[2] != 0x02 || raw[3] != 0x00 || raw[4] != code {
        return None;
    }
    let vtype = raw[5];
    let max = u16::from_be_bytes([raw[6], raw[7]]);
    let current = u16::from_be_bytes([raw[8], raw[9]]);
    Some(VcpReply { current, max: if vtype == 0 && max != 0 { max } else { 0xFF } })
}

/// Minimal EDID identity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edid {
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial32: u32,
    pub name: String,
    pub serial_text: String,
}

pub fn parse_edid(edid: &[u8]) -> Option<Edid> {
    if edid.len() < 128 {
        return None;
    }
    let mut out = Edid {
        vendor_id: u16::from_be_bytes([edid[8], edid[9]]),
        product_id: u16::from_le_bytes([edid[10], edid[11]]),
        serial32: u32::from_le_bytes([edid[12], edid[13], edid[14], edid[15]]),
        ..Edid::default()
    };
    let text = |d: &[u8]| -> String {
        d[5..18]
            .iter()
            .take_while(|b| **b != 0x0A)
            .filter(|b| (0x20..0x7F).contains(*b))
            .map(|b| *b as char)
            .collect::<String>()
            .trim()
            .to_string()
    };
    for off in [54usize, 72, 90, 108] {
        let d = &edid[off..off + 18];
        if d[0] == 0 && d[1] == 0 && d[2] == 0 {
            match d[3] {
                0xFC if out.name.is_empty() => out.name = text(d),
                0xFF if out.serial_text.is_empty() => out.serial_text = text(d),
                _ => {}
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_request_checksum_matches_m1ddc() {
        // brightness read: 0x82 0x01 0x10, checksum = 0x6E ^ 0x51 ^ 0x82 ^ 0x01 ^ 0x10
        assert_eq!(build_get_request(0x10), [0x82, 0x01, 0x10, 0x6E ^ 0x51 ^ 0x82 ^ 0x01 ^ 0x10]);
    }

    #[test]
    fn set_request_carries_big_endian_value() {
        let r = build_set_request(0x72, 0x7800);
        assert_eq!(&r[..5], &[0x84, 0x03, 0x72, 0x78, 0x00]);
        assert_eq!(r[5], 0x6E ^ 0x51 ^ 0x84 ^ 0x03 ^ 0x72 ^ 0x78);
    }

    fn reply(code: u8, vtype: u8, max: u16, cur: u16) -> Vec<u8> {
        let mut r = vec![0x6E, 0x88, 0x02, 0x00, code, vtype, (max >> 8) as u8, max as u8,
                         (cur >> 8) as u8, cur as u8];
        let chk = r.iter().fold(0x50u8, |a, b| a ^ b);
        r.push(chk);
        r
    }

    #[test]
    fn parses_a_good_reply() {
        let r = parse_vcp_reply(&reply(0x10, 0, 100, 50), 0x10).unwrap();
        assert_eq!((r.current, r.max), (50, 100));
    }

    #[test]
    fn rejects_wrong_opcode_bad_checksum_and_short_frames() {
        assert!(parse_vcp_reply(&reply(0x12, 0, 100, 50), 0x10).is_none());
        let mut bad = reply(0x10, 0, 100, 50);
        bad[8] ^= 1;
        assert!(parse_vcp_reply(&bad, 0x10).is_none());
        assert!(parse_vcp_reply(&[0u8; 5], 0x10).is_none());
        let mut unsupported = reply(0x10, 0, 100, 50);
        unsupported[3] = 0x01; // result: unsupported feature
        assert!(parse_vcp_reply(&unsupported, 0x10).is_none());
    }

    #[test]
    fn non_continuous_or_zero_max_falls_back_to_0xff() {
        assert_eq!(parse_vcp_reply(&reply(0xDC, 1, 35, 13), 0xDC).unwrap().max, 0xFF);
        assert_eq!(parse_vcp_reply(&reply(0xDC, 0, 0, 13), 0xDC).unwrap().max, 0xFF);
    }

    #[test]
    fn edid_identity_and_name() {
        let mut e = vec![0u8; 128];
        e[8] = 0x06;
        e[9] = 0xB3; // ASUS
        e[10] = 0x01;
        e[11] = 0x27; // product 0x2701
        e[54..58].copy_from_slice(&[0, 0, 0, 0xFC]);
        e[59..59 + 7].copy_from_slice(b"PA278QV");
        e[59 + 7] = 0x0A;
        let p = parse_edid(&e).unwrap();
        assert_eq!((p.vendor_id, p.product_id), (0x06B3, 0x2701));
        assert_eq!(p.name, "PA278QV");
    }
}

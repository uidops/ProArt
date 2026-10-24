//! Renders the ProArt app icon set with a tiny signed-distance rasteriser.
//!
//!     cargo run -p make-icons -- assets/icons
//!
//! Writes app-<size>.png, tray-template.png (monochrome, for the macOS menu
//! bar), tray-color.png and ProArt.ico. `packaging/macos/make-icns.sh` turns
//! the PNGs into ProArt.icns with iconutil.
//!
//! The mark: a dark squircle holding a colour-spectrum ring (the colour
//! accuracy motif) around a geometric "P". Original artwork.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

type Rgba = [f32; 4];

// ------------------------------------------------------------------ SDF math --

fn sd_round_box(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

fn sd_ring(px: f32, py: f32, cx: f32, cy: f32, r_in: f32, r_out: f32) -> f32 {
    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
    ((r_in + r_out) / 2.0 - d).abs() - (r_out - r_in) / 2.0
}

fn smooth(d: f32, aa: f32) -> f32 {
    (0.5 - d / aa).clamp(0.0, 1.0)
}

fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = (h.rem_euclid(360.0)) / 60.0;
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r + m, g + m, b + m]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Source-over of `src` (premultiplied-free colour + alpha) onto `dst`.
fn over(dst: Rgba, c: [f32; 3], a: f32) -> Rgba {
    let oa = a + dst[3] * (1.0 - a);
    if oa <= 0.0 {
        return [0.0; 4];
    }
    let f = |s: f32, d: f32| (s * a + d * dst[3] * (1.0 - a)) / oa;
    [f(c[0], dst[0]), f(c[1], dst[1]), f(c[2], dst[2]), oa]
}

// --------------------------------------------------------------------- mark --

/// Distance to the "P" glyph, in a 1024 design space centred on (512, 512).
fn sd_p(x: f32, y: f32) -> f32 {
    // stem 410..470, bowl flush with the stem's left edge, glyph centred on x = 512
    let stem = sd_round_box(x, y, 440.0, 512.0, 30.0, 150.0, 6.0);
    let bowl_outer = sd_round_box(x, y, 512.0, 440.0, 102.0, 80.0, 74.0);
    let bowl_inner = sd_round_box(x, y, 515.0, 440.0, 45.0, 32.0, 26.0);
    let bowl = bowl_outer.max(-bowl_inner);
    stem.min(bowl)
}

/// App icon sample at design coordinates (0..1024).
fn app_sample(x: f32, y: f32, aa: f32) -> Rgba {
    let mut px: Rgba = [0.0; 4];

    // squircle tile (macOS icon grid: 824 px inside the 1024 canvas)
    let tile = sd_round_box(x, y, 512.0, 512.0, 412.0, 412.0, 184.0);
    let t = ((y - 100.0) / 824.0).clamp(0.0, 1.0);
    let bg = mix([0.105, 0.105, 0.135], [0.020, 0.020, 0.030], t);
    px = over(px, bg, smooth(tile, aa));
    // soft inner glow along the top edge
    let rim = smooth(tile + 3.0, aa) * (1.0 - smooth(tile + 8.0, aa)) * (1.0 - t).powi(2);
    px = over(px, [1.0, 1.0, 1.0], rim * 0.10);

    // spectrum ring
    let ring = sd_ring(x, y, 512.0, 512.0, 196.0, 292.0);
    let hue = (y - 512.0).atan2(x - 512.0).to_degrees() + 90.0;
    let lift = ((512.0 - y) / 292.0).clamp(-1.0, 1.0) * 0.10;
    let col = hsv(hue, 0.74, (0.96 + lift).min(1.0));
    px = over(px, col, smooth(ring, aa));
    // faint disc inside the ring, like a lit display
    let disc = ((x - 512.0).powi(2) + (y - 512.0).powi(2)).sqrt() - 190.0;
    px = over(px, [0.16, 0.16, 0.21], smooth(disc, aa) * 0.9);

    // the P
    px = over(px, [0.98, 0.98, 1.0], smooth(sd_p(x, y), aa));
    px
}

/// Menu-bar template: alpha only (macOS tints it), ring with a gap + P.
fn tray_sample(x: f32, y: f32, aa: f32) -> Rgba {
    let ring = sd_ring(x, y, 512.0, 512.0, 400.0, 480.0);
    // open the ring at the top right, like a colour wheel's sector gap
    let ang = (y - 512.0).atan2(x - 512.0).to_degrees();
    let gap = if (-75.0..-25.0).contains(&ang) { 1.0 } else { 0.0 };
    let ring_a = smooth(ring, aa) * (1.0 - gap);
    let p = smooth(sd_p(x, y) , aa);
    let a = (ring_a + p).min(1.0);
    [0.0, 0.0, 0.0, a]
}

/// Coloured small-size mark (Windows / Linux tray): spectrum ring + white P, no tile.
fn tray_color_sample(x: f32, y: f32, aa: f32) -> Rgba {
    let mut px: Rgba = [0.0; 4];
    let ring = sd_ring(x, y, 512.0, 512.0, 360.0, 490.0);
    let hue = (y - 512.0).atan2(x - 512.0).to_degrees() + 90.0;
    px = over(px, hsv(hue, 0.80, 1.0), smooth(ring, aa));
    px = over(px, [0.98, 0.98, 1.0], smooth(sd_p(x, y), aa));
    px
}

fn render(size: u32, f: fn(f32, f32, f32) -> Rgba) -> Vec<u8> {
    let ss = 3u32; // supersampling per axis
    let scale = 1024.0 / size as f32;
    let aa = 1.2 * scale / ss as f32; // edge width in design units
    let mut out = vec![0u8; (size * size * 4) as usize];
    for py in 0..size {
        for px in 0..size {
            let mut acc = [0f32; 4];
            let mut weight = 0.0;
            for sy in 0..ss {
                for sx in 0..ss {
                    let x = (px as f32 + (sx as f32 + 0.5) / ss as f32) * scale;
                    let y = (py as f32 + (sy as f32 + 0.5) / ss as f32) * scale;
                    let c = f(x, y, aa);
                    // average premultiplied
                    acc[0] += c[0] * c[3];
                    acc[1] += c[1] * c[3];
                    acc[2] += c[2] * c[3];
                    acc[3] += c[3];
                    weight += 1.0;
                }
            }
            let a = acc[3] / weight;
            let i = ((py * size + px) * 4) as usize;
            if a > 0.0 {
                out[i] = ((acc[0] / acc[3]) * 255.0).round().clamp(0.0, 255.0) as u8;
                out[i + 1] = ((acc[1] / acc[3]) * 255.0).round().clamp(0.0, 255.0) as u8;
                out[i + 2] = ((acc[2] / acc[3]) * 255.0).round().clamp(0.0, 255.0) as u8;
                out[i + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

fn encode_png(size: u32, rgba: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, size, size);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("png header");
        w.write_image_data(rgba).expect("png data");
    }
    buf
}

fn write_png(dir: &Path, name: &str, size: u32, f: fn(f32, f32, f32) -> Rgba) -> Vec<u8> {
    let png = encode_png(size, &render(size, f));
    fs::write(dir.join(name), &png).expect("write png");
    println!("wrote {name} ({size}x{size}, {} bytes)", png.len());
    png
}

/// ICO with PNG-compressed entries (supported since Windows Vista).
fn write_ico(path: &Path, entries: &[(u32, Vec<u8>)]) {
    let mut f = BufWriter::new(File::create(path).expect("create ico"));
    f.write_all(&[0, 0, 1, 0]).unwrap();
    f.write_all(&(entries.len() as u16).to_le_bytes()).unwrap();
    let mut offset = 6 + 16 * entries.len() as u32;
    for (size, data) in entries {
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        f.write_all(&[dim, dim, 0, 0]).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap(); // planes
        f.write_all(&32u16.to_le_bytes()).unwrap(); // bpp
        f.write_all(&(data.len() as u32).to_le_bytes()).unwrap();
        f.write_all(&offset.to_le_bytes()).unwrap();
        offset += data.len() as u32;
    }
    for (_, data) in entries {
        f.write_all(data).unwrap();
    }
    println!("wrote {}", path.display());
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "assets/icons".into());
    let dir = Path::new(&dir);
    fs::create_dir_all(dir).expect("create output dir");

    let mut ico = Vec::new();
    for size in [1024u32, 512, 256, 128, 64, 48, 32, 16] {
        let png = write_png(dir, &format!("app-{size}.png"), size, app_sample);
        if size <= 256 {
            ico.push((size, png));
        }
    }
    ico.sort_by_key(|(s, _)| *s);
    write_ico(&dir.join("ProArt.ico"), &ico);
    write_png(dir, "tray-template.png", 88, tray_sample);
    write_png(dir, "tray-color.png", 64, tray_color_sample);
}

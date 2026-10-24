//! ProArt look: near-black surfaces, a spectrum accent, one hue per OSD section.

use gpui::{hsla, rgb, Hsla, Rgba, WindowAppearance};
use proart_core::features::Group;

#[derive(Clone, Copy)]
pub struct Theme {
    pub bg: Rgba,
    pub sidebar: Rgba,
    pub card: Rgba,
    pub field: Rgba,
    pub hover: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub dim: Rgba,
    pub faint: Rgba,
    pub accent: Rgba,
    pub danger: Rgba,
    pub warn: Rgba,
}

impl Theme {
    pub fn dark() -> Theme {
        Theme {
            bg: rgb(0x0A0A0E),
            sidebar: rgb(0x101016),
            card: rgb(0x16161E),
            field: rgb(0x20202A),
            hover: rgb(0x2A2A36),
            border: rgb(0x26263A),
            text: rgb(0xEDEDF2),
            dim: rgb(0x9A9AAA),
            faint: rgb(0x62627A),
            accent: rgb(0x558BFF),
            danger: rgb(0xFF5F5F),
            warn: rgb(0xFBBF24),
        }
    }

    pub fn light() -> Theme {
        Theme {
            bg: rgb(0xF2F2F6),
            sidebar: rgb(0xE8E8EE),
            card: rgb(0xFFFFFF),
            field: rgb(0xECECF1),
            hover: rgb(0xDDDDE6),
            border: rgb(0xD9D9E2),
            text: rgb(0x15151C),
            dim: rgb(0x6A6A78),
            faint: rgb(0xA0A0AE),
            accent: rgb(0x2F6BFF),
            danger: rgb(0xE53935),
            warn: rgb(0xD97706),
        }
    }

    /// `setting` is "auto" | "light" | "dark"; auto follows the window appearance.
    pub fn resolve(setting: &str, appearance: WindowAppearance) -> Theme {
        match setting {
            "light" => Theme::light(),
            "dark" => Theme::dark(),
            _ => match appearance {
                WindowAppearance::Light | WindowAppearance::VibrantLight => Theme::light(),
                _ => Theme::dark(),
            },
        }
    }
}

/// One hue per OSD section: a spectrum of accents, like the icon's ring.
pub fn group_color(g: Group) -> Hsla {
    let hue = match g {
        Group::Preset => 275.0,
        Group::Blue => 212.0,
        Group::Palette => 330.0,
        Group::Image => 28.0,
        Group::Sound => 140.0,
        Group::Input => 188.0,
        Group::System => 52.0,
    };
    hsla(hue / 360.0, 0.75, 0.68, 1.0)
}

/// Hue (degrees) to a bright spectrum colour.
pub fn spectrum(deg: f32) -> Hsla {
    hsla(deg.rem_euclid(360.0) / 360.0, 0.78, 0.66, 1.0)
}

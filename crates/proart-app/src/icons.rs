//! Line icons for the sidebar and page titles (`assets/ui/*.svg`, original
//! artwork). GPUI draws an SVG as a mask, so `text_color` picks its colour.

use std::borrow::Cow;

use gpui::{px, svg, AssetSource, Hsla, Result, SharedString, Styled, Svg};
use proart_core::features::Group;

use crate::main_view::Page;

macro_rules! embed {
    ($($name:literal),* $(,)?) => {
        const ICONS: &[(&str, &[u8])] = &[$((concat!("ui/", $name, ".svg"), include_bytes!(concat!("../../../assets/ui/", $name, ".svg")))),*];
    };
}

embed!("overview", "preset", "blue", "palette", "image", "sound", "input", "system", "info", "presets", "displays", "settings", "about", "automation", "brightness");

/// The app's assets, compiled into the binary.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS.iter().find(|(p, _)| *p == path).map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)).collect())
    }
}

pub fn for_page(page: Page) -> &'static str {
    match page {
        Page::Overview => "ui/overview.svg",
        Page::Group(g) => match g {
            Group::Preset => "ui/preset.svg",
            Group::Blue => "ui/blue.svg",
            Group::Palette => "ui/palette.svg",
            Group::Image => "ui/image.svg",
            Group::Sound => "ui/sound.svg",
            Group::Input => "ui/input.svg",
            Group::System => "ui/system.svg",
        },
        Page::Info => "ui/info.svg",
        Page::Presets => "ui/presets.svg",
        Page::Automation => "ui/automation.svg",
        Page::Displays => "ui/displays.svg",
        Page::Settings => "ui/settings.svg",
        Page::About => "ui/about.svg",
    }
}

pub fn icon(path: &'static str, size: f32, color: impl Into<Hsla>) -> Svg {
    svg().path(path).size(px(size)).flex_none().text_color(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_has_an_embedded_icon() {
        let mut pages = vec![Page::Overview, Page::Info, Page::Presets, Page::Automation, Page::Displays, Page::Settings, Page::About];
        pages.extend(Group::ALL.iter().map(|g| Page::Group(*g)));
        for page in pages {
            let bytes = Assets.load(for_page(page)).unwrap().unwrap_or_else(|| panic!("{page:?} has no icon"));
            assert!(bytes.starts_with(b"<svg"), "{page:?}");
        }
    }
}

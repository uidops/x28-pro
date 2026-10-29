//! Built-in icon set, served to gpui's `svg()` element as assets.
//!
//! 24 px line icons (2 px round strokes, in the style of Material Symbols
//! Rounded). gpui renders an SVG as an alpha mask and tints it with the
//! element's text color, so the stroke color here does not matter.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

const ICONS: &[(&str, &str)] = &[
    // network mode: antenna mast with radio waves
    ("tower", r#"<circle cx="12" cy="10" r="2" fill="black" stroke="none"/><path d="M8.5 6.5a5 5 0 0 0 0 7M15.5 6.5a5 5 0 0 1 0 7M5.5 3.5a9 9 0 0 0 0 13M18.5 3.5a9 9 0 0 1 0 13M12 12v9M9 21h6"/>"#),
    // band lock: radio waves
    ("waves", r#"<path d="M2 9c2.5-4 5-4 7.5 0s5 4 7.5 0 3-3 5-1.5M2 16c2.5-4 5-4 7.5 0s5 4 7.5 0 3-3 5-1.5"/>"#),
    // operator: SIM card
    ("sim", r#"<path d="M8 2h7l5 5v13a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2z"/><rect x="9" y="11" width="6" height="7" rx="1"/>"#),
    ("power", r#"<path d="M12 3v8M7.05 6.05a7 7 0 1 0 9.9 0"/>"#),
    ("refresh", r#"<path d="M20 12a8 8 0 1 1-2.34-5.66L20 8.5M20 3.5v5h-5"/>"#),
    ("thermometer", r#"<path d="M10 13.5V5a2 2 0 0 1 4 0v8.5a4 4 0 1 1-4 0z"/><circle cx="12" cy="17" r="1.5" fill="black" stroke="none"/>"#),
    ("cpu", r#"<rect x="6" y="6" width="12" height="12" rx="2"/><rect x="10" y="10" width="4" height="4" rx="0.5"/><path d="M9.5 2.5v3M14.5 2.5v3M9.5 18.5v3M14.5 18.5v3M2.5 9.5h3M2.5 14.5h3M18.5 9.5h3M18.5 14.5h3"/>"#),
    ("clock", r#"<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3.5 2"/>"#),
    ("chevron", r#"<path d="M9 6l6 6-6 6"/>"#),
    ("chevron-down", r#"<path d="M6 9l6 6 6-6"/>"#),
    ("check", r#"<path d="M5 12.5l4.5 4.5L19 7.5"/>"#),
    ("window", r#"<rect x="3" y="4.5" width="18" height="15" rx="2.5"/><path d="M3 9h18"/>"#),
    ("sliders", r#"<path d="M4 7h9M17 7h3M4 17h3M11 17h9"/><circle cx="15" cy="7" r="2"/><circle cx="9" cy="17" r="2"/>"#),
    ("logout", r#"<path d="M14 4h4a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-4M9 16l-4-4 4-4M5 12h10"/>"#),
    // data connection: globe
    ("globe", r#"<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c2.5 2.5 3.8 5.5 3.8 9s-1.3 6.5-3.8 9M12 3c-2.5 2.5-3.8 5.5-3.8 9s1.3 6.5 3.8 9"/>"#),
    ("pause", r#"<rect x="6.5" y="5" width="3.5" height="14" rx="1.2" fill="black" stroke="none"/><rect x="14" y="5" width="3.5" height="14" rx="1.2" fill="black" stroke="none"/>"#),
    ("play", r#"<path d="M8 5.5v13a1 1 0 0 0 1.5.86l10.5-6.5a1 1 0 0 0 0-1.72L9.5 4.64A1 1 0 0 0 8 5.5z" fill="black" stroke="none"/>"#),
    ("alert", r#"<circle cx="12" cy="12" r="9"/><path d="M12 7.5v5.5"/><circle cx="12" cy="16.5" r="1.2" fill="black" stroke="none"/>"#),
];

/// `svg().path(...)` argument for a named icon.
pub fn icon(name: &str) -> SharedString {
    format!("icons/{name}.svg").into()
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let name = path.strip_prefix("icons/").and_then(|p| p.strip_suffix(".svg"));
        Ok(ICONS.iter().find(|(n, _)| Some(*n) == name).map(|(_, body)| {
            Cow::Owned(
                format!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#
                )
                .into_bytes(),
            )
        }))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .map(|(n, _)| icon(n))
            .filter(|p| p.starts_with(path))
            .collect())
    }
}

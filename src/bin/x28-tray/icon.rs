//! Menu bar icon: a network label ("5G"/"4G") beside five signal bars lit by
//! the router's own 0–5 signal level (the bars its web UI shows), drawn as SVG
//! and handed to macOS as a template image so it takes the menu bar's own
//! color. Offline draws dim bars with a slash; paused dims the lit bars.

use gpui::{App, Image, ImageFormat};
use gpui_tray::Icon;
use x28_pro::Status;

/// What the icon shows; compared to skip redundant icon swaps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glyph {
    label: Option<String>,
    bars: usize,
    offline: bool,
    paused: bool,
}

impl Glyph {
    pub fn for_state(status: Option<&Status>, error: bool) -> Self {
        // Generation tag only ("5G" for "5G(NSA)", "4G+" on carrier aggregation).
        let label = status.and_then(|s| s.generation());
        Glyph {
            label,
            bars: status.and_then(|s| s.signal_bars()).unwrap_or(0),
            offline: error,
            paused: false,
        }
    }

    /// Polling is paused: the bars show the last reading, dimmed.
    pub fn paused(mut self, paused: bool) -> Self {
        self.paused = paused;
        self
    }

    /// SVG at 2x, 18 pt tall; as wide as the label (if any) plus the bars.
    fn svg(&self) -> String {
        const H: f32 = 36.0;
        const BAR_W: f32 = 5.0;
        const GAP: f32 = 2.5;
        let bars_w = 5.0 * BAR_W + 4.0 * GAP;
        // ~12 px per bold 20 px glyph, plus a gap before the bars.
        let label_w = self.label.as_ref().map_or(0.0, |l| l.chars().count() as f32 * 12.0 + 5.0);
        let width = label_w + bars_w + 4.0;
        let x0 = width - bars_w - 2.0;

        let mut body = String::new();
        if let Some(label) = &self.label {
            body.push_str(&format!(
                r#"<text x="0" y="26" font-family="Helvetica Neue, Helvetica, Arial" font-size="20" font-weight="bold" letter-spacing="-0.5">{label}</text>"#
            ));
        }
        for i in 0..5 {
            let h = 8.0 + 5.0 * i as f32;
            let lit = !self.offline && i < self.bars;
            body.push_str(&format!(
                r#"<rect x="{x}" y="{y}" width="{BAR_W}" height="{h}" rx="1.5" fill-opacity="{a}"/>"#,
                x = x0 + i as f32 * (BAR_W + GAP),
                y = 32.0 - h,
                a = match (lit, self.paused) {
                    (true, false) => 1.0,
                    (true, true) => 0.55,
                    (false, _) => 0.25,
                },
            ));
        }
        if self.offline {
            body.push_str(&format!(
                r#"<line x1="{}" y1="33" x2="{}" y2="3" stroke="black" stroke-width="3.5" stroke-linecap="round"/>"#,
                x0 - 1.0,
                x0 + bars_w + 1.0
            ));
        }
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{H}" viewBox="0 0 {width} {H}" fill="black">{body}</svg>"#
        )
    }

    pub fn icon(&self, cx: &App) -> Icon {
        let image = Image::from_bytes(ImageFormat::Svg, self.svg().into_bytes());
        Icon::from_gpui(&image, cx)
            .template(true)
            .scale_factor(2.0)
            .build()
            .unwrap_or_else(|e| {
                eprintln!("tray icon: {e}");
                Icon::from_rgba(vec![0, 0, 0, 0], 1, 1).build().expect("bitmap icon")
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_follows_network_and_signal() {
        let status = Status {
            network_type: Some("5G".into()),
            signal_level: Some("5".into()),
            sinr: Some("3".into()),
            ..Status::default()
        };
        let g = Glyph::for_state(Some(&status), false);
        assert_eq!((g.label.as_deref(), g.bars, g.offline), (Some("5G"), 5, false));
        assert!(g.svg().contains(">5G</text>"));

        let ca = Status { network_type: Some("4G+".into()), ..Status::default() };
        assert_eq!(Glyph::for_state(Some(&ca), false).label.as_deref(), Some("4G+"));
        let nsa = Status { network_type: Some("5G(NSA)".into()), ..Status::default() };
        assert_eq!(Glyph::for_state(Some(&nsa), false).label.as_deref(), Some("5G"));

        let g = Glyph::for_state(None, true);
        assert_eq!((g.label.as_deref(), g.bars, g.offline), (None, 0, true));
        assert!(g.svg().contains("<line"));
    }
}

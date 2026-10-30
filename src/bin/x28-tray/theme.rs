//! Material theme.
//!
//! Material Design 3 color roles in black and white (grey surfaces; only
//! status colors and errors keep a hue), shape scale and 4 dp grid, rendered
//! as translucent layers over the window's blurred macOS backdrop: surfaces
//! carry alpha so the desktop shows through, and a hairline edge separates
//! each layer. The scheme follows the system light/dark appearance.

use gpui::prelude::*;
use gpui::{div, px, relative, rgb, rgba, Div, FontWeight, Hsla, WindowAppearance};
use x28_pro::{Carrier, Heat, Quality};

fn solid(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn glass(hex_rgba: u32) -> Hsla {
    rgba(hex_rgba).into()
}

#[derive(Clone, Copy)]
pub struct Theme {
    pub backdrop: Hsla,
    pub hairline: Hsla,
    pub surface_container_low: Hsla,
    pub surface_container: Hsla,
    pub surface_container_high: Hsla,
    pub surface_container_highest: Hsla,
    pub hero_tint: Hsla,
    pub on_surface: Hsla,
    pub on_surface_variant: Hsla,
    pub outline: Hsla,
    pub outline_variant: Hsla,
    pub primary: Hsla,
    pub primary_hover: Hsla,
    pub on_primary: Hsla,
    pub primary_container: Hsla,
    pub primary_container_hover: Hsla,
    pub on_primary_container: Hsla,
    pub secondary_container: Hsla,
    pub secondary_container_hover: Hsla,
    pub on_secondary_container: Hsla,
    pub error_container: Hsla,
    pub on_error_container: Hsla,
    pub on_error: Hsla,
    pub good: Hsla,
    pub warn: Hsla,
    pub bad: Hsla,
}

impl Theme {
    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
        }
    }

    fn dark() -> Self {
        Self {
            backdrop: glass(0x111111b8),
            hairline: glass(0xffffff14),
            surface_container_low: glass(0x1a1a1acc),
            surface_container: glass(0x1e1e1ec7),
            surface_container_high: glass(0x2a2a2ae0),
            surface_container_highest: solid(0x353535),
            hero_tint: glass(0x2c2c2cb3),
            on_surface: solid(0xededed),
            on_surface_variant: solid(0xb8b8b8),
            outline: solid(0x8a8a8a),
            outline_variant: solid(0x444444),
            primary: solid(0xffffff),
            primary_hover: solid(0xdcdcdc),
            on_primary: solid(0x111111),
            primary_container: solid(0xeeeeee),
            primary_container_hover: solid(0xd6d6d6),
            on_primary_container: solid(0x111111),
            secondary_container: solid(0x353535),
            secondary_container_hover: solid(0x424242),
            on_secondary_container: solid(0xe8e8e8),
            error_container: solid(0x93000a),
            on_error_container: solid(0xffdad6),
            on_error: solid(0x690005),
            good: solid(0x9bd67e),
            warn: solid(0xffb95c),
            bad: solid(0xffb4ab),
        }
    }

    fn light() -> Self {
        Self {
            backdrop: glass(0xfafafab8),
            hairline: glass(0x0000000f),
            surface_container_low: glass(0xf4f4f4cc),
            surface_container: glass(0xefefefd9),
            surface_container_high: glass(0xe5e5e5e6),
            surface_container_highest: solid(0xdcdcdc),
            hero_tint: glass(0xe8e8e8e6),
            on_surface: solid(0x111111),
            on_surface_variant: solid(0x4a4a4a),
            outline: solid(0x767676),
            outline_variant: solid(0xc8c8c8),
            primary: solid(0x111111),
            primary_hover: solid(0x333333),
            on_primary: solid(0xffffff),
            primary_container: solid(0x1c1c1c),
            primary_container_hover: solid(0x333333),
            on_primary_container: solid(0xffffff),
            secondary_container: solid(0xe0e0e0),
            secondary_container_hover: solid(0xd2d2d2),
            on_secondary_container: solid(0x1a1a1a),
            error_container: solid(0xffdad6),
            on_error_container: solid(0x410002),
            on_error: solid(0xffffff),
            good: solid(0x3b6d1f),
            warn: solid(0x8a5100),
            bad: solid(0xba1a1a),
        }
    }

    pub fn quality_color(&self, quality: Quality) -> Hsla {
        match quality {
            Quality::Excellent | Quality::Good => self.good,
            Quality::Fair => self.warn,
            Quality::Poor => self.bad,
        }
    }

    pub fn heat_color(&self, heat: Heat) -> Hsla {
        match heat {
            Heat::Cool => self.good,
            Heat::Warm => self.warn,
            Heat::Hot => self.bad,
        }
    }

    /// M3 type: overline — small caps label, medium weight, on-surface-variant.
    /// (gpui 0.2.2 has no letter-spacing, so it is not tracked.)
    pub fn overline(&self, text: &str) -> Div {
        div()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(self.on_surface_variant)
            .child(text.to_string())
    }

    /// Content card — translucent surface container, 16 dp radius.
    pub fn card(&self) -> Div {
        div()
            .rounded_2xl()
            .bg(self.surface_container)
            .border_1()
            .border_color(self.hairline)
            .px(px(14.))
            .py(px(12.))
    }

    /// Hero surface: primary-tinted container, 28 dp radius, soft elevation.
    pub fn hero(&self) -> Div {
        div()
            .rounded_3xl()
            .bg(self.hero_tint)
            .border_1()
            .border_color(self.hairline)
            .shadow_md()
            .px(px(18.))
            .py(px(16.))
    }

    /// M3 filled tonal button/chip.
    pub fn tonal(&self) -> Div {
        let hover = self.secondary_container_hover;
        div()
            .rounded_full()
            .bg(self.secondary_container)
            .text_color(self.on_secondary_container)
            .hover(move |s| s.bg(hover))
    }

    /// M3 filled tonal button/chip in the primary color.
    pub fn primary_tonal(&self) -> Div {
        let hover = self.primary_container_hover;
        div()
            .rounded_full()
            .bg(self.primary_container)
            .text_color(self.on_primary_container)
            .hover(move |s| s.bg(hover))
    }

    /// M3 outlined chip.
    pub fn outlined(&self) -> Div {
        let hover = self.surface_container_high;
        div()
            .rounded_full()
            .border_1()
            .border_color(self.outline_variant)
            .text_color(self.on_surface_variant)
            .hover(move |s| s.bg(hover))
    }

    /// M3 filled button (primary).
    pub fn filled(&self) -> Div {
        let hover = self.primary_hover;
        div()
            .rounded_full()
            .bg(self.primary)
            .text_color(self.on_primary)
            .font_weight(FontWeight::MEDIUM)
            .shadow_sm()
            .hover(move |s| s.bg(hover))
    }

    /// Small round tonal button — used by the poll stepper.
    pub fn round_tonal(&self) -> Div {
        self.tonal().size(px(32.)).flex().items_center().justify_center()
    }

    /// M3 outlined text field box; focus moves the border to primary.
    pub fn field_box(&self, focused: bool) -> Div {
        let base = div()
            .rounded_xl()
            .h(px(38.))
            .px(px(12.))
            .flex()
            .items_center()
            .bg(if focused { self.surface_container_highest } else { self.surface_container_low })
            .border_1()
            .border_color(if focused { self.primary } else { self.outline_variant });
        if focused {
            base.border_2()
        } else {
            let hover = self.outline;
            base.hover(move |s| s.border_color(hover))
        }
    }

    /// M3 switch track; the caller adds the row around it.
    pub fn switch_track(&self, on: bool) -> Div {
        let track = div()
            .w(px(52.))
            .h(px(32.))
            .rounded_full()
            .px(px(4.))
            .flex()
            .items_center()
            .bg(if on { self.primary } else { self.surface_container_highest })
            .border_2()
            .border_color(if on { self.primary } else { self.outline });
        let track = if on { track.justify_end() } else { track.justify_start() };
        // M3 handle grows from 16 dp (off) to 24 dp (on).
        let handle = if on { px(24.) } else { px(16.) };
        track.child(
            div()
                .size(handle)
                .rounded_full()
                .bg(if on { self.on_primary } else { self.outline }),
        )
    }

    /// Stat tile (label over value) used inside the details card.
    pub fn tile(&self, label: &str, value: String) -> Div {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(self.overline(label))
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(value))
    }

    /// Linear progress track filled to `fraction` (0..=1) in `color`.
    pub fn meter(&self, fraction: Option<f32>, color: Hsla) -> Div {
        let track = div()
            .w_full()
            .h(px(6.))
            .rounded_full()
            .bg(self.surface_container_highest);
        match fraction {
            // Keep a visible sliver so a weak reading never looks empty.
            Some(f) => track.child(
                div()
                    .h_full()
                    .w(relative(f.clamp(0.04, 1.0)))
                    .rounded_full()
                    .bg(color),
            ),
            None => track,
        }
    }

    /// Five ascending bars, the first `lit` in the color for that level.
    pub fn signal_bars(&self, lit: usize) -> Div {
        let color = self.signal_color(lit);
        let mut bars = div().flex().flex_row().items_end().gap(px(3.)).h(px(22.));
        for (i, height) in [7., 10.5, 14., 18., 22.].into_iter().enumerate() {
            bars = bars.child(
                div()
                    .w(px(5.))
                    .h(px(height))
                    .rounded_sm()
                    .bg(if i < lit { color } else { self.outline_variant }),
            );
        }
        bars
    }

    /// One row per component carrier: band (accented), width, ARFCN, PCI.
    pub fn carrier_table(&self, carriers: &[Carrier]) -> Div {
        let cell = |w: f32| div().w(px(w)).flex_none();
        let header = div()
            .flex()
            .flex_row()
            .text_xs()
            .text_color(self.on_surface_variant)
            .child(cell(56.).child("BAND"))
            .child(cell(64.).child("WIDTH"))
            .child(div().flex_1().child("ARFCN"))
            .child(cell(44.).flex().justify_end().child("PCI"));
        let mut table = div().flex().flex_col().gap(px(4.)).child(header);
        for (i, c) in carriers.iter().enumerate() {
            let dash = || "—".to_string();
            let band = div()
                .h(px(20.))
                .px(px(6.))
                .rounded_md()
                .flex()
                .items_center()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .bg(if i == 0 { self.primary_container } else { self.secondary_container })
                .text_color(if i == 0 { self.on_primary_container } else { self.on_secondary_container })
                .child(c.band.clone());
            table = table.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .text_xs()
                    .child(cell(56.).flex().child(band))
                    .child(
                        cell(64.)
                            .font_weight(FontWeight::MEDIUM)
                            .child(c.width_mhz.as_ref().map(|w| format!("{w} MHz")).unwrap_or_else(dash)),
                    )
                    .child(div().flex_1().child(c.earfcn.clone().unwrap_or_else(dash)))
                    .child(cell(44.).flex().justify_end().child(c.pci.clone().unwrap_or_else(dash))),
            );
        }
        table
    }

    /// Color for a 0–5 signal level.
    pub fn signal_color(&self, bars: usize) -> Hsla {
        match bars {
            4.. => self.good,
            3 => self.warn,
            1..=2 => self.bad,
            0 => self.outline,
        }
    }

    /// Segmented-button track for the tab bar; segments go inside.
    pub fn segmented(&self) -> Div {
        div()
            .flex()
            .flex_row()
            .p(px(3.))
            .gap(px(2.))
            .rounded_full()
            .bg(self.surface_container)
            .border_1()
            .border_color(self.hairline)
    }

    /// One tab segment; the selected one is a raised secondary container.
    pub fn segment(&self, selected: bool) -> Div {
        let base = div()
            .flex_1()
            .h(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer();
        if selected {
            base.bg(self.secondary_container)
                .text_color(self.on_secondary_container)
                .shadow_sm()
        } else {
            let hover = self.surface_container_high;
            base.text_color(self.on_surface_variant).hover(move |s| s.bg(hover))
        }
    }

    /// M3 radio list row: indicator, label and optional supporting text.
    pub fn radio_row(&self, selected: bool, label: &str, supporting: Option<&str>) -> Div {
        let hover = self.surface_container_high;
        let ring = if selected { self.primary } else { self.outline };
        let indicator = div()
            .size(px(20.))
            .flex_none()
            .rounded_full()
            .border_2()
            .border_color(ring)
            .flex()
            .items_center()
            .justify_center()
            .when(selected, |d| d.child(div().size(px(10.)).rounded_full().bg(ring)));
        let mut text = div()
            .flex()
            .flex_col()
            .child(div().text_sm().child(label.to_string()));
        if let Some(s) = supporting {
            text = text.child(
                div()
                    .text_xs()
                    .text_color(self.on_surface_variant)
                    .child(s.to_string()),
            );
        }
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .px(px(8.))
            .py(px(7.))
            .rounded_lg()
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .child(indicator)
            .child(text)
    }

    /// Filter chip: outlined when off, filled with the primary container
    /// color when on. Same border and content in both states, so selecting a
    /// chip never changes its size.
    pub fn filter_chip(&self, selected: bool, label: String) -> Div {
        let base = div()
            .h(px(30.))
            .px(px(12.))
            .flex()
            .items_center()
            .rounded_lg()
            .border_1()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .child(label);
        if selected {
            let hover = self.primary_container_hover;
            base.bg(self.primary_container)
                .border_color(self.primary_container)
                .text_color(self.on_primary_container)
                .hover(move |s| s.bg(hover).border_color(hover))
        } else {
            let hover = self.surface_container_high;
            base.border_color(self.outline_variant)
                .text_color(self.on_surface_variant)
                .hover(move |s| s.bg(hover))
        }
    }

    /// M3 text button.
    pub fn text_button(&self) -> Div {
        let hover = self.surface_container_high;
        div()
            .rounded_full()
            .px(px(14.))
            .h(px(36.))
            .flex()
            .items_center()
            .justify_center()
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .text_color(self.primary)
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
    }

    /// Filled button in the error role, for destructive actions.
    pub fn danger(&self, armed: bool) -> Div {
        let base = div()
            .rounded_full()
            .h(px(40.))
            .px(px(18.))
            .flex()
            .items_center()
            .justify_center()
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer();
        if armed {
            base.bg(self.bad).text_color(self.on_error)
        } else {
            base.bg(self.error_container).text_color(self.on_error_container)
        }
    }

    /// A disabled look for any button-like element.
    pub fn disabled<E: Styled>(&self, el: E) -> E {
        el.opacity(0.38).cursor_default()
    }
}

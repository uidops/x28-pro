//! The tray popover: what the status item opens instead of a plain-text menu.
//!
//! A frosted Material surface with the live link (network, operator, signal
//! meters), device stats, quick settings that expand in place (network mode,
//! 4G/5G band lock, operator, restart) and footer actions. It closes when it
//! loses focus, like a system menu bar extra.

use std::collections::BTreeSet;
use std::time::Instant;

use gpui::prelude::*;
use gpui::{div, px, svg, Context, Div, FocusHandle, FontWeight, Hsla, Render, Subscription, Window};
use x28_pro::{operator_name, signal_label, Heat, NetworkMode, OperatorSelection, Quality, OPERATORS};

use crate::assets::icon;
use crate::theme::Theme;
use crate::{
    apply_network_mode, apply_operator, open_panel_at, router_task, AppState, PopoverRef, Quit,
    TogglePause, REATTACH_NOTE,
};
use crate::panel::Tab;

pub(crate) const WIDTH: f32 = 360.;
/// Fits everything with the quick settings collapsed (5G row included);
/// expanded sections scroll. Shrunk to fit short screens (see `appkit`).
pub(crate) const HEIGHT: f32 = 940.;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Mode,
    /// Band lock for the 5G radio when true, 4G when false.
    Bands(bool),
    Operator,
}

pub(crate) struct Popover {
    expanded: Option<Section>,
    /// Picked bands while a band section is open (empty = all bands).
    bands: Option<BTreeSet<u32>>,
    confirm_reboot: bool,
    /// Close on focus loss only after the panel has had focus once.
    was_active: bool,
    /// Holds keyboard focus so Escape reaches the popover.
    focus: FocusHandle,
    _activation: Subscription,
    _appearance: Subscription,
}

impl Popover {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        Self {
            focus,
            expanded: None,
            bands: None,
            confirm_reboot: false,
            was_active: false,
            _activation: cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.was_active = true;
                } else if this.was_active {
                    cx.global_mut::<PopoverRef>().closed_at = Some(Instant::now());
                    window.remove_window();
                }
            }),
            _appearance: cx.observe_window_appearance(window, |_, _, cx| cx.notify()),
        }
    }

    fn toggle(&mut self, section: Section, cx: &mut Context<Self>) {
        self.expanded = (self.expanded != Some(section)).then_some(section);
        self.bands = None;
        cx.notify();
    }

    /// The picked bands, or what the router has locked.
    fn current_bands(&self, nr: bool, cx: &Context<Self>) -> BTreeSet<u32> {
        self.bands.clone().unwrap_or_else(|| {
            let router = cx.global::<AppState>().router.as_ref();
            let lock = router.and_then(|r| if nr { r.bands.nr.clone() } else { r.bands.lte.clone() });
            lock.unwrap_or_default().into_iter().collect()
        })
    }

    fn apply_bands(&mut self, nr: bool, cx: &mut Context<Self>) {
        let Some(picked) = self.bands.take() else {
            return;
        };
        let (radio, prefix) = if nr { ("5G", "n") } else { ("4G", "B") };
        let bands: Option<Vec<u32>> = (!picked.is_empty()).then(|| picked.into_iter().collect());
        let label = match &bands {
            Some(b) => b.iter().map(|b| format!("{prefix}{b}")).collect::<Vec<_>>().join(", "),
            None => "all bands".to_string(),
        };
        router_task(cx, format!("Locking {radio} to {label}"), true, move |c| {
            // Re-read so the other radio's lock is kept as is.
            let mut lock = c.router_settings()?.bands;
            if nr {
                lock.nr = bands;
            } else {
                lock.lte = bands;
            }
            c.set_band_lock(&lock)?;
            Ok(format!("{radio}: {label}. {REATTACH_NOTE}"))
        });
        cx.notify();
    }

    fn reboot(&mut self, cx: &mut Context<Self>) {
        if !self.confirm_reboot {
            self.confirm_reboot = true;
        } else {
            self.confirm_reboot = false;
            router_task(cx, "Restarting the router", false, |c| {
                c.reboot()?;
                Ok("Restart sent. The router is back in about a minute.".to_string())
            });
        }
        cx.notify();
    }
}

fn ico(name: &str, size: f32, color: Hsla) -> impl IntoElement {
    svg().path(icon(name)).size(px(size)).flex_none().text_color(color)
}

/// Rounded tonal disc holding a leading icon.
fn leading(t: &Theme, name: &str, color: Hsla, bg: Hsla) -> Div {
    div()
        .size(px(32.))
        .flex_none()
        .rounded_full()
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
        .child(ico(name, 18., color))
        .text_color(t.on_surface)
}

impl Render for Popover {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::for_appearance(window.appearance());
        let state = cx.global::<AppState>();
        let status = state.status.clone();
        let error = state.error.clone();
        let router = state.router.clone();
        let busy = state.router_busy.clone();
        let note = state.router_note.clone();
        let paused = state.paused;
        let age = state.updated.map(|u| u.elapsed().as_secs());

        let bars = status.as_ref().and_then(|s| s.signal_bars());
        let bars_color = t.signal_color(bars.unwrap_or(0));
        let dash = || "—".to_string();

        let mut root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.backdrop)
            .text_color(t.on_surface)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, event: &gpui::KeyDownEvent, window, _| {
                if event.keystroke.key == "escape" {
                    window.remove_window();
                }
            }));

        // ---- header: network badge, operator --------------------------------------
        // "5G" large, the firmware's extra detail ("NSA") small beneath it.
        let network = status
            .as_ref()
            .map(|s| s.generation().unwrap_or_else(|| s.network_label()))
            .unwrap_or_else(dash);
        let network_detail = status.as_ref().and_then(|s| s.network_detail());
        let operator = status
            .as_ref()
            .and_then(|s| s.operator.clone())
            .filter(|o| !o.is_empty() && o != "NULL")
            .unwrap_or_else(|| "X28 PRO".to_string());
        // Serving cell as chips: band(s) accented, then width and cell ids.
        let chip = |label: Option<&str>, value: String, accent: bool| {
            let (bg, fg) = if accent {
                (t.secondary_container, t.on_secondary_container)
            } else {
                (t.surface_container_high, t.on_surface)
            };
            let mut c = div()
                .h(px(22.))
                .px(px(8.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.))
                .rounded_md()
                .bg(bg)
                .text_color(fg)
                .text_xs();
            if let Some(label) = label {
                c = c.child(div().text_color(t.on_surface_variant).child(label.to_string()));
            }
            c.child(div().font_weight(FontWeight::SEMIBOLD).child(value))
        };
        // One carrier: band, width, PCI and ARFCN as chips. Carrier
        // aggregation: a band chip per carrier plus the total width; the
        // per-carrier details go in a table in the signal card.
        let carriers = status.as_ref().map(|s| s.carriers()).unwrap_or_default();
        let mut chips = div().flex().flex_row().flex_wrap().gap(px(4.)).pt(px(4.));
        match (&status, carriers.as_slice()) {
            (None, _) => {
                chips = chips.child(div().text_xs().text_color(t.on_surface_variant).child("waiting for the router…"))
            }
            (Some(_), []) => {}
            (Some(_), [c]) => {
                chips = chips.child(chip(None, c.band.clone(), true));
                if let Some(w) = &c.width_mhz {
                    chips = chips.child(chip(None, format!("{w} MHz"), false));
                }
                if let Some(pci) = &c.pci {
                    chips = chips.child(chip(Some("PCI"), pci.clone(), false));
                }
                if let Some(f) = &c.earfcn {
                    chips = chips.child(chip(Some("ARFCN"), f.clone(), false));
                }
            }
            (Some(_), many) => {
                for c in many {
                    chips = chips.child(chip(None, c.band.clone(), true));
                }
                let total: f32 = many
                    .iter()
                    .filter_map(|c| c.width_mhz.as_deref()?.parse::<f32>().ok())
                    .sum();
                let summary = if total > 0. { format!("{total} MHz") } else { "CA".to_string() };
                chips = chips.child(chip(Some(&format!("{} carriers", many.len())), summary, false));
            }
        }

        let header = div()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(12.))
            .child(
                div()
                    .h(px(44.))
                    .min_w(px(52.))
                    .px(px(10.))
                    .flex_none()
                    .rounded_xl()
                    .bg(t.primary_container)
                    .text_color(t.on_primary_container)
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .child(div().text_lg().line_height(px(20.)).font_weight(FontWeight::BOLD).child(network))
                    .when_some(network_detail, |badge, detail| {
                        badge.child(
                            div()
                                .text_size(px(9.))
                                .line_height(px(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(detail),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .child(div().text_base().font_weight(FontWeight::SEMIBOLD).truncate().child(operator))
                    .child(chips),
            )
            .child({
                // Pause / resume background polling; filled while paused.
                let hover = if paused { t.primary_container_hover } else { t.surface_container_high };
                let (bg, fg) = if paused {
                    (t.primary_container, t.on_primary_container)
                } else {
                    (t.surface_container, t.on_surface_variant)
                };
                div()
                    .id("btn-pause")
                    .size(px(32.))
                    .flex_none()
                    .rounded_full()
                    .bg(bg)
                    .border_1()
                    .border_color(t.hairline)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(ico(if paused { "play" } else { "pause" }, 14., fg))
                    .on_click(cx.listener(|_, _, _, cx| cx.dispatch_action(&TogglePause)))
            });

        // ---- signal hero -----------------------------------------------------------
        let meter_row = |name: &str, value: Option<f32>, unit: &str, grade: fn(f32) -> Quality, range: (f32, f32)| {
            let color = value.map(|v| t.quality_color(grade(v))).unwrap_or(t.outline);
            let frac = value.map(|v| ((v - range.0) / (range.1 - range.0)).clamp(0., 1.));
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .child(div().w(px(52.)).text_xs().text_color(t.on_surface_variant).child(name.to_string()))
                .child(div().flex_1().child(t.meter(frac, color)))
                .child(
                    div()
                        .w(px(64.))
                        .flex()
                        .justify_end()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(value.map(|v| format!("{v} {unit}")).unwrap_or_else(dash)),
                )
        };
        let hero = t
            .hero()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.))
                    .child(t.signal_bars(bars.unwrap_or(0)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(bars_color)
                                    .child(bars.map(|b| format!("{} signal", signal_label(b))).unwrap_or_else(|| "No signal data".into())),
                            )
                            .child(div().text_xs().text_color(t.on_surface_variant).child(match (age, paused) {
                                (Some(s), true) if s < 60 => format!("Paused · updated {s}s ago"),
                                (Some(s), true) => format!("Paused · updated {}m ago", s / 60),
                                (None, true) => "Paused".to_string(),
                                (Some(s), false) if s < 60 => format!("updated {s}s ago"),
                                (Some(s), false) => format!("updated {}m ago", s / 60),
                                (None, false) => "not updated yet".to_string(),
                            })),
                    ),
            )
            .child(meter_row("RSRP", status.as_ref().and_then(|s| s.rsrp_dbm()), "dBm", Quality::rsrp, (-120., -70.)))
            .child(meter_row("RSRQ", status.as_ref().and_then(|s| s.rsrq_db()), "dB", Quality::rsrq, (-20., -3.)))
            .child(meter_row("SINR", status.as_ref().and_then(|s| s.sinr_db()), "dB", Quality::sinr, (-5., 30.)))
            // NR readings while attached to a 5G cell (NSA keeps the LTE anchor above).
            .when(status.as_ref().is_some_and(|s| s.on_nr()), |hero| {
                let s = status.as_ref().expect("checked above");
                hero.child(meter_row("5G RSRP", s.rsrp_5g_dbm(), "dBm", Quality::rsrp, (-120., -70.)))
                    .child(meter_row("5G SINR", s.sinr_5g_db(), "dB", Quality::sinr, (-5., 30.)))
            })
            .when(carriers.len() > 1, |hero| {
                hero.child(div().h(px(1.)).bg(t.hairline)).child(t.carrier_table(&carriers))
            });

        // ---- stat tiles --------------------------------------------------------------
        let heat = status.as_ref().and_then(|s| s.temperature_c()).map(Heat::from_celsius);
        let stat = |name: &str, color: Hsla, value: String, caption: &str| {
            div()
                .flex_1()
                .min_w(px(0.))
                .rounded_xl()
                .bg(t.surface_container)
                .border_1()
                .border_color(t.hairline)
                .px(px(10.))
                .py(px(10.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(ico(name, 18., color))
                .child(div().text_base().font_weight(FontWeight::SEMIBOLD).truncate().child(value))
                .child(div().text_xs().text_color(t.on_surface_variant).child(caption.to_string()))
        };
        let stats = div()
            .flex()
            .flex_row()
            .gap(px(8.))
            .child(stat(
                "thermometer",
                heat.map(|h| t.heat_color(h)).unwrap_or(t.outline),
                status.as_ref().map(|s| s.temperature_label()).unwrap_or_else(dash),
                heat.map(Heat::label).unwrap_or("Temperature"),
            ))
            .child(stat(
                "cpu",
                t.primary,
                status.as_ref().and_then(|s| s.cpu_load.clone()).map(|c| format!("{c}%")).unwrap_or_else(dash),
                "CPU",
            ))
            .child(stat(
                "clock",
                t.primary,
                status
                    .as_ref()
                    .and_then(|s| s.uptime_secs)
                    .map(|s| if s >= 86400 { format!("{}d {}h", s / 86400, s % 86400 / 3600) } else { format!("{}h {}m", s / 3600, s % 3600 / 60) })
                    .unwrap_or_else(dash),
                "Uptime",
            ));

        // ---- data connection (APN): addresses and DNS ---------------------------------
        let wan = status.as_ref().and_then(|s| s.wan.clone());
        let connection = wan.map(|wan| {
            let line = |label: &str, value: String| {
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(12.))
                    .text_xs()
                    .child(div().w(px(40.)).flex_none().text_color(t.on_surface_variant).child(label.to_string()))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .justify_end()
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(value),
                    )
            };
            let mut card = div()
                .rounded_xl()
                .bg(t.surface_container)
                .border_1()
                .border_color(t.hairline)
                .px(px(12.))
                .py(px(10.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .child(ico("globe", 16., t.primary))
                        .child(div().flex_1().text_sm().font_weight(FontWeight::MEDIUM).child("Data connection"))
                        .when_some(wan.apn.clone(), |row, apn| {
                            row.child(
                                div()
                                    .h(px(22.))
                                    .px(px(8.))
                                    .rounded_md()
                                    .bg(t.secondary_container)
                                    .text_color(t.on_secondary_container)
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .text_xs()
                                    .child(div().text_color(t.on_surface_variant).child("APN"))
                                    .child(div().font_weight(FontWeight::SEMIBOLD).child(apn)),
                            )
                        }),
                );
            if let Some(ip) = wan.ipv4.clone() {
                card = card.child(line("IPv4", ip));
            }
            if let Some(ip) = wan.ipv6.clone() {
                card = card.child(line("IPv6", ip));
            }
            for (i, dns) in wan.dns.iter().enumerate() {
                card = card.child(line(if i == 0 { "DNS" } else { "" }, dns.clone()));
            }
            card
        });

        // ---- banners ---------------------------------------------------------------------
        let banner = |bg: Hsla, fg: Hsla, name: &str, text: String| {
            div()
                .rounded_lg()
                .bg(bg)
                .text_color(fg)
                .px(px(10.))
                .py(px(8.))
                .flex()
                .flex_row()
                .items_start()
                .gap(px(8.))
                .child(ico(name, 16., fg))
                .child(div().flex_1().min_w(px(0.)).text_xs().child(text))
        };
        let mut banners = div().flex().flex_col().gap(px(6.));
        if let Some(err) = &error {
            banners = banners.child(banner(t.error_container, t.on_error_container, "alert", crate::ellipsize(err, 140)));
        }
        if let Some(b) = &busy {
            banners = banners.child(banner(t.primary_container, t.on_primary_container, "refresh", format!("{b}…")));
        } else if let Some(n) = &note {
            banners = banners.child(match n {
                Ok(m) => banner(t.secondary_container, t.on_secondary_container, "check", m.clone()),
                Err(e) => banner(t.error_container, t.on_error_container, "alert", crate::ellipsize(e, 140)),
            });
        }

        // ---- quick settings --------------------------------------------------------------
        let mode_code = router.as_ref().and_then(|r| r.network_mode.clone());
        let mode_label = mode_code
            .as_deref()
            .map(|c| NetworkMode::from_code(c).map(|m| m.label().to_string()).unwrap_or(c.to_string()))
            .unwrap_or_else(|| "—".into());
        let lock_label = |lock: Option<&Option<Vec<u32>>>, prefix: &str| match lock {
            Some(Some(b)) => b.iter().map(|b| format!("{prefix}{b}")).collect::<Vec<_>>().join(", "),
            Some(None) => "All bands".to_string(),
            None => "—".to_string(),
        };
        let op_now = router.as_ref().and_then(|r| r.operator.clone());
        let op_label = match &op_now {
            Some(OperatorSelection::Automatic) => "Automatic".to_string(),
            Some(OperatorSelection::Manual(p)) => operator_name(p).map(str::to_string).unwrap_or(p.clone()),
            None => "—".into(),
        };
        let can_change = router.is_some() && busy.is_none();

        let row = |id: &'static str, name: &str, title: &str, value: String, open: bool| {
            let hover = t.surface_container_high;
            div()
                .id(id)
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.))
                .px(px(10.))
                .py(px(8.))
                .rounded_xl()
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .when(open, |d| d.bg(t.surface_container_high))
                .child(leading(&t, name, t.on_secondary_container, t.secondary_container))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(title.to_string()))
                        .child(div().text_xs().text_color(t.on_surface_variant).truncate().child(value)),
                )
                .child(ico(if open { "chevron-down" } else { "chevron" }, 16., t.on_surface_variant))
        };

        let mut settings = div()
            .rounded_2xl()
            .bg(t.surface_container)
            .border_1()
            .border_color(t.hairline)
            .p(px(4.))
            .flex()
            .flex_col()
            .gap(px(2.));

        // network mode
        let open = self.expanded == Some(Section::Mode);
        settings = settings.child(
            row("qs-mode", "tower", "Network mode", mode_label, open)
                .on_click(cx.listener(|this, _, _, cx| this.toggle(Section::Mode, cx))),
        );
        if open {
            let mut list = div().flex().flex_col().px(px(6.)).pb(px(6.));
            for mode in NetworkMode::ALL {
                let selected = mode_code.as_deref() == Some(mode.code());
                let item = t.radio_row(selected, mode.label(), None).id(mode.code());
                list = list.child(if can_change && !selected {
                    item.on_click(cx.listener(move |this, _, _, cx| {
                        this.expanded = None;
                        apply_network_mode(mode, cx);
                    }))
                } else {
                    item
                });
            }
            settings = settings.child(list);
        }

        // band locks: 4G always, 5G when the network mode can use it
        let show_nr = router.as_ref().is_some_and(|r| r.allows_nr() && !r.nr_supported.is_empty());
        for nr in [false, true] {
            if nr && !show_nr {
                continue;
            }
            let (id, title, prefix, other) = if nr {
                ("qs-bands-nr", "5G band lock", "n", "4G")
            } else {
                ("qs-bands-lte", "4G band lock", "B", "5G")
            };
            let lock = router.as_ref().map(|r| if nr { &r.bands.nr } else { &r.bands.lte });
            let supported: Vec<u32> = router
                .as_ref()
                .map(|r| if nr { r.nr_supported.clone() } else { r.lte_supported.clone() })
                .unwrap_or_default();
            let open = self.expanded == Some(Section::Bands(nr));
            settings = settings.child(
                row(id, "waves", title, lock_label(lock, prefix), open)
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(Section::Bands(nr), cx))),
            );
            if !open {
                continue;
            }
            let picked = self.current_bands(nr, cx);
            let dirty = self.bands.is_some();
            let mut chips = div().flex().flex_row().flex_wrap().gap(px(6.));
            chips = chips.child(
                t.filter_chip(picked.is_empty(), "All".to_string())
                    .id(("band-all", nr as usize))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.bands = Some(BTreeSet::new());
                        cx.notify();
                    })),
            );
            for band in supported {
                chips = chips.child(
                    t.filter_chip(picked.contains(&band), format!("{prefix}{band}"))
                        .id((if nr { "band-nr" } else { "band-lte" }, band as usize))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let mut set = this.current_bands(nr, cx);
                            if !set.remove(&band) {
                                set.insert(band);
                            }
                            this.bands = Some(set);
                            cx.notify();
                        })),
                );
            }
            let apply = t
                .filled()
                .id(("band-apply", nr as usize))
                .h(px(32.))
                .px(px(16.))
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .child("Apply");
            let apply = if dirty && can_change {
                apply.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.apply_bands(nr, cx)))
            } else {
                t.disabled(apply)
            };
            settings = settings.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .px(px(10.))
                    .pt(px(4.))
                    .pb(px(10.))
                    .child(chips)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.on_surface_variant)
                                    .child(format!("{other} lock is kept as is")),
                            )
                            .child(apply),
                    ),
            );
        }

        // operator
        let open = self.expanded == Some(Section::Operator);
        settings = settings.child(
            row("qs-operator", "sim", "Operator", op_label, open)
                .on_click(cx.listener(|this, _, _, cx| this.toggle(Section::Operator, cx))),
        );
        if open {
            let mut list = div().flex().flex_col().px(px(6.)).pb(px(6.));
            let choices = std::iter::once((OperatorSelection::Automatic, "Automatic".to_string()))
                .chain(OPERATORS.into_iter().map(|(p, n)| (OperatorSelection::Manual(p.to_string()), format!("{n}  ·  {p}"))));
            for (i, (selection, label)) in choices.enumerate() {
                let selected = op_now.as_ref() == Some(&selection);
                let item = t.radio_row(selected, &label, None).id(("op", i));
                list = list.child(if can_change && !selected {
                    item.on_click(cx.listener(move |this, _, _, cx| {
                        this.expanded = None;
                        apply_operator(selection.clone(), cx);
                    }))
                } else {
                    item
                });
            }
            settings = settings.child(list);
        }

        // restart
        let armed = self.confirm_reboot;
        let restart = div()
            .id("qs-restart")
            .h(px(30.))
            .px(px(12.))
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .map(|d| if armed { d.bg(t.bad).text_color(t.on_error) } else { d.bg(t.error_container).text_color(t.on_error_container) })
            .child(if armed { "Confirm" } else { "Restart" });
        let restart = if busy.is_none() {
            restart.cursor_pointer().on_click(cx.listener(|this, _, _, cx| this.reboot(cx)))
        } else {
            t.disabled(restart)
        };
        settings = settings.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.))
                .px(px(10.))
                .py(px(8.))
                .child(leading(&t, "power", t.on_error_container, t.error_container))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("Restart router"))
                        .child(div().text_xs().text_color(t.on_surface_variant).child(if armed { "Click Confirm to restart now" } else { "About a minute offline" })),
                )
                .child(restart),
        );

        // ---- footer ------------------------------------------------------------------------
        let foot_button = |id: &'static str, name: &str, label: &str| {
            let hover = t.surface_container_high;
            div()
                .id(id)
                .h(px(34.))
                .px(px(10.))
                .rounded_full()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(ico(name, 16., t.primary))
                .child(label.to_string())
        };
        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.))
            .child(foot_button("ft-refresh", "refresh", "Refresh").on_click(cx.listener(|_, _, _, cx| {
                cx.global_mut::<AppState>().pending_refresh = true;
                cx.notify();
            })))
            .child(foot_button("ft-open", "window", "Open app").on_click(cx.listener(|_, _, window, cx| {
                window.remove_window();
                open_panel_at(Tab::Status, cx);
            })))
            .child(foot_button("ft-settings", "sliders", "Settings").on_click(cx.listener(|_, _, window, cx| {
                window.remove_window();
                open_panel_at(Tab::App, cx);
            })))
            .child(div().flex_1())
            .child(foot_button("ft-quit", "logout", "Quit").on_click(cx.listener(|_, _, _, cx| cx.dispatch_action(&Quit))));

        root = root.child(
            div()
                .id("popover-scroll")
                .flex_1()
                .overflow_y_scroll()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .px(px(14.))
                        .pt(px(16.))
                        .pb(px(12.))
                        .child(header)
                        .child(hero)
                        .child(stats)
                        .children(connection)
                        .child(banners)
                        .child(t.overline("QUICK SETTINGS").pl(px(4.)))
                        .child(settings),
                ),
        );
        root.child(
            div()
                .flex_none()
                .border_t_1()
                .border_color(t.hairline)
                .px(px(8.))
                .py(px(6.))
                .child(footer),
        )
    }
}

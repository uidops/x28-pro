//! The window opened from the tray, in four tabs:
//!   * Status  — temperature hero, signal meters, cell and device details
//!   * Network — network mode, 4G/5G band lock and operator, applied together
//!   * Device  — router restart and the web UI capture for unmapped settings
//!   * App     — router address, login, poll interval and menu bar text

use std::collections::BTreeSet;

use gpui::prelude::*;
use gpui::{
    div, px, App, Context, Div, FocusHandle, FontWeight, Hsla, KeyDownEvent, MouseButton, Render,
    Subscription, Window,
};
use x28_pro::{
    operator_name, validate_lte_cells, validate_nr_cells, BandLock, CellLock, Client, Config, Heat,
    LockState, LteCell, NetworkMode, NrCell, OperatorSelection, Quality, RouterSettings,
    mode_allows_nr, signal_label, OPERATORS,
};

use crate::theme::Theme;
use crate::{apply_lte_cell_lock, apply_nr_cell_lock, read_router_settings, router_task, AppState, REATTACH_NOTE};

/// Element ids for the text fields (ElementId needs static strs): the App
/// tab's connection settings, then the Network tab's cell lock values.
const FIELD_IDS: [&str; 8] = [
    "field-router",
    "field-user",
    "field-pass",
    "field-lte-earfcn",
    "field-lte-pci",
    "field-nr-band",
    "field-nr-arfcn",
    "field-nr-pci",
];
/// Indexes into `Panel::fields`.
const LTE_EARFCN: usize = 3;
const LTE_PCI: usize = 4;
const NR_BAND: usize = 5;
const NR_ARFCN: usize = 6;
const NR_PCI: usize = 7;

/// `(value - min) / (max - min)`, for placing a reading on a meter.
fn fraction(value: Option<f32>, min: f32, max: f32) -> Option<f32> {
    value.map(|v| ((v - min) / (max - min)).clamp(0.0, 1.0))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    Status,
    Network,
    Device,
    App,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Status, Tab::Network, Tab::Device, Tab::App];

    fn label(self) -> &'static str {
        match self {
            Tab::Status => "Status",
            Tab::Network => "Network",
            Tab::Device => "Device",
            Tab::App => "App",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Tab::Status => "tab-status",
            Tab::Network => "tab-network",
            Tab::Device => "tab-device",
            Tab::App => "tab-app",
        }
    }
}

/// Unapplied edits on the Network tab, in the shape the controls use.
#[derive(Clone, Debug, PartialEq, Eq)]
struct NetworkDraft {
    mode: Option<String>,
    lte_lock: bool,
    lte: BTreeSet<u32>,
    nr_lock: bool,
    nr: BTreeSet<u32>,
    operator: Option<OperatorSelection>,
}

impl NetworkDraft {
    fn from_settings(r: &RouterSettings) -> Self {
        let set = |b: &Option<Vec<u32>>| b.iter().flatten().copied().collect::<BTreeSet<u32>>();
        NetworkDraft {
            mode: r.network_mode.clone(),
            lte_lock: r.bands.lte.is_some(),
            lte: set(&r.bands.lte),
            nr_lock: r.bands.nr.is_some(),
            nr: set(&r.bands.nr),
            operator: r.operator.clone(),
        }
    }

    fn band_lock(&self) -> BandLock {
        let side = |on: bool, bands: &BTreeSet<u32>| on.then(|| bands.iter().copied().collect());
        BandLock {
            lte: side(self.lte_lock, &self.lte),
            nr: side(self.nr_lock, &self.nr),
        }
    }

    /// A lock switched on with no band picked would cut the radio off.
    fn problem(&self) -> Option<&'static str> {
        if self.lte_lock && self.lte.is_empty() {
            Some("Pick at least one 4G band, or turn the 4G lock off.")
        } else if self.nr_lock && self.nr.is_empty() {
            Some("Pick at least one 5G band, or turn the 5G lock off.")
        } else {
            None
        }
    }
}

/// One editable settings row: a label, a buffer with a cursor, and the focus
/// handle that routes keystrokes to it.
struct Field {
    label: &'static str,
    chars: Vec<char>,
    cursor: usize,
    focus: FocusHandle,
    masked: bool,
    /// Accepts digits only.
    numeric: bool,
}

impl Field {
    fn new(label: &'static str, value: &str, masked: bool, cx: &mut App) -> Self {
        let chars: Vec<char> = value.chars().collect();
        Self {
            label,
            cursor: chars.len(),
            chars,
            focus: cx.focus_handle(),
            masked,
            numeric: false,
        }
    }

    fn numeric(mut self) -> Self {
        self.numeric = true;
        self
    }

    fn set(&mut self, value: &str) {
        self.chars = value.chars().collect();
        self.cursor = self.chars.len();
    }

    fn value(&self) -> String {
        self.chars.iter().collect()
    }

    fn display(&self) -> String {
        if self.masked && !self.chars.is_empty() {
            "•".repeat(self.chars.len())
        } else {
            self.value()
        }
    }

    fn handle_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Panel>,
    ) {
        let stroke = &event.keystroke;
        let command = stroke.modifiers.platform || stroke.modifiers.control;

        match stroke.key.as_str() {
            "backspace" => {
                if command {
                    // cmd-backspace clears the field
                    self.chars.clear();
                    self.cursor = 0;
                } else if self.cursor > 0 {
                    self.cursor -= 1;
                    self.chars.remove(self.cursor);
                }
            }
            "delete" => {
                if self.cursor < self.chars.len() {
                    self.chars.remove(self.cursor);
                }
            }
            "left" => self.cursor = self.cursor.saturating_sub(1),
            "right" => self.cursor = (self.cursor + 1).min(self.chars.len()),
            "home" | "up" => self.cursor = 0,
            "end" | "down" => self.cursor = self.chars.len(),
            "escape" => window.blur(),
            "tab" | "enter" => {} // handled by the caller
            _ => {
                if !command {
                    if let Some(ch) = stroke.key_char.as_deref() {
                        let allowed = |c: char| !c.is_control() && (!self.numeric || c.is_ascii_digit());
                        if ch.chars().all(allowed) {
                            let insertion: Vec<char> = ch.chars().collect();
                            let added = insertion.len();
                            let at = self.cursor;
                            self.chars.splice(at..at, insertion);
                            self.cursor = at + added;
                        }
                    }
                }
            }
        }
        cx.notify();
    }
}

pub(crate) struct Panel {
    tab: Tab,
    fields: Vec<Field>,
    pub(crate) poll_secs: u64,
    pub(crate) menu_title: bool,
    note: Option<String>,
    /// Network tab edits; `None` means "show what the router has".
    draft: Option<NetworkDraft>,
    /// First click on Restart arms it; the second sends it.
    confirm_reboot: bool,
    capturing: bool,
    capture_note: Option<Result<String, String>>,
    /// Cell lock radio choice per radio; `None` = what the router has.
    lte_cell_lock: Option<bool>,
    nr_cell_lock: Option<bool>,
    /// The cell lock fields were filled from the router's lock once.
    cell_fields_loaded: bool,
    /// Re-renders with the matching scheme when the system goes light/dark.
    _appearance: Subscription,
}

impl Panel {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let config = cx.global::<AppState>().config.clone();
        Self {
            tab: Tab::Status,
            fields: vec![
                Field::new("Router", &config.host, false, cx),
                Field::new("User", &config.user, false, cx),
                Field::new("Password", &config.pass, true, cx),
                Field::new("EARFCN", "", false, cx).numeric(),
                Field::new("PCI", "", false, cx).numeric(),
                Field::new("Band", "", false, cx).numeric(),
                Field::new("ARFCN", "", false, cx).numeric(),
                Field::new("PCI", "", false, cx).numeric(),
            ],
            poll_secs: config.poll_secs,
            menu_title: config.menu_title,
            note: None,
            draft: None,
            confirm_reboot: false,
            capturing: false,
            capture_note: None,
            lte_cell_lock: None,
            nr_cell_lock: None,
            cell_fields_loaded: false,
            _appearance: cx.observe_window_appearance(window, |_, _, cx| cx.notify()),
        }
    }

    pub(crate) fn select_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.confirm_reboot = false;
        let state = cx.global::<AppState>();
        if tab == Tab::Network && state.router.is_none() && state.router_busy.is_none() {
            read_router_settings(cx);
        }
        cx.notify();
    }

    fn focus_field(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.fields[index].focus.clone();
        window.focus(&handle);
        cx.notify();
    }

    /// One Material outlined text field: overline label above the input box,
    /// with a caret at the cursor while focused.
    fn field_cell(&self, t: &Theme, index: usize, window: &Window, cx: &mut Context<Self>) -> Div {
        let field = &self.fields[index];
        let shown: Vec<char> = field.display().chars().collect();
        let focused = field.focus.is_focused(window);
        let focus_handle = field.focus.clone();

        let content = if shown.is_empty() && !focused {
            div()
                .text_sm()
                .text_color(t.outline)
                .child(if field.masked { "••••••" } else { "—" })
        } else if focused {
            // Masked text has one bullet per char, so the cursor index holds.
            let cursor = field.cursor.min(shown.len());
            let before: String = shown[..cursor].iter().collect();
            let after: String = shown[cursor..].iter().collect();
            div()
                .flex()
                .flex_row()
                .items_center()
                .text_sm()
                .text_color(t.on_surface)
                .child(before)
                .child(div().w(px(2.)).h(px(16.)).rounded_full().bg(t.primary))
                .child(after)
        } else {
            div()
                .text_sm()
                .text_color(t.on_surface)
                .truncate()
                .child(shown.into_iter().collect::<String>())
        };

        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                t.overline(&field.label.to_uppercase())
                    .text_color(if focused { t.primary } else { t.on_surface_variant }),
            )
            .child(
                t.field_box(focused)
                    .id(FIELD_IDS[index])
                    .track_focus(&focus_handle)
                    .cursor_text()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| this.focus_field(index, window, cx)),
                    )
                    .child(content),
            )
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Each tab cycles through its own fields.
        let group = match self.tab {
            Tab::App => 0..LTE_EARFCN,
            Tab::Network => LTE_EARFCN..FIELD_IDS.len(),
            Tab::Status | Tab::Device => return,
        };
        let key = event.keystroke.key.as_str();
        if key == "tab" {
            let len = group.len();
            let current = group
                .clone()
                .position(|i| self.fields[i].focus.is_focused(window))
                .unwrap_or(0);
            let next = if event.keystroke.modifiers.shift {
                current.checked_sub(1).unwrap_or(len - 1)
            } else {
                (current + 1) % len
            };
            let handle = self.fields[group.start + next].focus.clone();
            window.focus(&handle);
            cx.notify();
            return;
        }
        if let Some(index) = group.clone().find(|&i| self.fields[i].focus.is_focused(window)) {
            self.fields[index].handle_key(event, window, cx);
        }
    }

    /// Fill the cell lock fields from the router's lock the first time it is
    /// known, so a locked cell shows its values.
    fn sync_cell_fields(&mut self, cx: &mut Context<Self>) {
        if self.cell_fields_loaded {
            return;
        }
        let Some(lock) = cx.global::<AppState>().router.as_ref().and_then(|r| r.cell_lock.clone()) else {
            return;
        };
        if let Some(c) = lock.lte.as_ref().and_then(|c| c.first()) {
            self.fields[LTE_EARFCN].set(&c.earfcn);
            self.fields[LTE_PCI].set(&c.pci);
        }
        if let Some(c) = lock.nr.as_ref().and_then(|c| c.first()) {
            self.fields[NR_BAND].set(&c.band);
            self.fields[NR_ARFCN].set(&c.arfcn);
            self.fields[NR_PCI].set(&c.pci);
        }
        self.cell_fields_loaded = true;
    }

    fn lte_cell(&self) -> LteCell {
        LteCell { earfcn: self.fields[LTE_EARFCN].value(), pci: self.fields[LTE_PCI].value() }
    }

    fn nr_cell(&self) -> NrCell {
        NrCell {
            band: self.fields[NR_BAND].value(),
            arfcn: self.fields[NR_ARFCN].value(),
            pci: self.fields[NR_PCI].value(),
        }
    }

    /// Copy the serving cell into the fields.
    fn use_current_cell(&mut self, nr: bool, cx: &mut Context<Self>) {
        let status = cx.global::<AppState>().status.clone();
        if nr {
            if let Some(c) = status.and_then(|s| s.current_nr_cell()) {
                self.fields[NR_BAND].set(&c.band);
                self.fields[NR_ARFCN].set(&c.arfcn);
                self.fields[NR_PCI].set(&c.pci);
                self.nr_cell_lock = Some(true);
            }
        } else if let Some(c) = status.and_then(|s| s.current_lte_cell()) {
            self.fields[LTE_EARFCN].set(&c.earfcn);
            self.fields[LTE_PCI].set(&c.pci);
            self.lte_cell_lock = Some(true);
        }
        cx.notify();
    }

    fn apply_cell_lock(&mut self, nr: bool, cx: &mut Context<Self>) {
        let lock = cx.global::<AppState>().router.as_ref().and_then(|r| r.cell_lock.clone()).unwrap_or_default();
        if nr {
            let on = self.nr_cell_lock.unwrap_or(lock.nr.is_some());
            apply_nr_cell_lock(on.then(|| self.nr_cell()), cx);
            self.nr_cell_lock = None;
        } else {
            let on = self.lte_cell_lock.unwrap_or(lock.lte.is_some());
            apply_lte_cell_lock(on.then(|| self.lte_cell()), cx);
            self.lte_cell_lock = None;
        }
        // Show the router's lock values once it has been re-read.
        self.cell_fields_loaded = false;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let mut config = cx.global::<AppState>().config.clone();
        config.host = self.fields[0].value();
        config.user = self.fields[1].value();
        config.pass = self.fields[2].value();
        config.poll_secs = self.poll_secs;
        config.menu_title = self.menu_title;
        config.normalize();

        match config.save() {
            Ok(()) => {
                // Reload the buffers so normalization (scheme, bounds) shows.
                self.fields[0].chars = config.host.chars().collect();
                self.fields[0].cursor = self.fields[0].chars.len();
                self.fields[1].chars = config.user.chars().collect();
                self.fields[1].cursor = self.fields[1].chars.len();
                self.poll_secs = config.poll_secs;

                let state = cx.global_mut::<AppState>();
                state.config = config;
                state.session = None; // credentials may have changed
                state.pending_refresh = true;
                self.note = Some(format!("Saved to {}", Config::path().display()));
            }
            Err(e) => self.note = Some(format!("Save failed: {e}")),
        }
        cx.notify();
    }

    fn adjust_poll(&mut self, delta: i64, cx: &mut Context<Self>) {
        let next = (self.poll_secs as i64 + delta).clamp(5, 3600);
        self.poll_secs = next as u64;
        self.note = Some("press Save to apply".to_string());
        cx.notify();
    }

    /// Change the Network tab draft, starting it from the router's settings.
    fn edit(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut NetworkDraft)) {
        let Some(router) = cx.global::<AppState>().router.as_ref() else {
            return;
        };
        let draft = self.draft.get_or_insert_with(|| NetworkDraft::from_settings(router));
        change(draft);
        cx.notify();
    }

    /// Send only what changed, in one background task.
    fn apply_network(&mut self, cx: &mut Context<Self>) {
        let Some(router) = cx.global::<AppState>().router.clone() else {
            return;
        };
        let Some(draft) = self.draft.take() else {
            return;
        };
        let base = NetworkDraft::from_settings(&router);
        let mode = draft.mode.clone().filter(|_| draft.mode != base.mode);
        let bands = (draft.band_lock() != base.band_lock()).then(|| draft.band_lock());
        let operator = draft.operator.clone().filter(|_| draft.operator != base.operator);

        router_task(cx, "Applying network settings", true, move |c| {
            let mut done = Vec::new();
            if let Some(mode) = mode {
                c.set_network_mode(&mode)?;
                done.push("network mode");
            }
            if let Some(bands) = bands {
                c.set_band_lock(&bands)?;
                done.push("band lock");
            }
            if let Some(operator) = operator {
                c.set_operator(&operator)?;
                done.push("operator");
            }
            Ok(format!("Updated {}. {REATTACH_NOTE}", done.join(", ")))
        });
        cx.notify();
    }

    fn reboot(&mut self, cx: &mut Context<Self>) {
        if !self.confirm_reboot {
            self.confirm_reboot = true;
            cx.notify();
            return;
        }
        self.confirm_reboot = false;
        router_task(cx, "Restarting the router", false, |c| {
            c.reboot()?;
            Ok("Restart sent. The router is back in about a minute.".to_string())
        });
        cx.notify();
    }

    fn capture_web_ui(&mut self, cx: &mut Context<Self>) {
        if self.capturing {
            return;
        }
        self.capturing = true;
        self.capture_note = None;
        let config = cx.global::<AppState>().config.clone();
        cx.spawn(async move |this, cx| {
            let dir = Config::web_ui_dir();
            let target = dir.clone();
            let result = cx
                .background_executor()
                .spawn(async move { Client::new(&config).capture_web_ui(&target) })
                .await;
            this.update(cx, |this, cx| {
                this.capturing = false;
                this.capture_note = Some(result.map(|c| {
                    format!(
                        "Saved {} files with {} API commands to {}",
                        c.files,
                        c.commands,
                        dir.display()
                    )
                }));
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

/// Section heading: overline label followed by a hairline rule.
fn section(t: &Theme, title: &str) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .pt(px(4.))
        .child(t.overline(title))
        .child(div().flex_1().h(px(1.)).bg(t.outline_variant))
}

/// Inline banner: an icon disc and wrapping text on a tinted container.
fn banner(bg: Hsla, fg: Hsla, icon: &'static str, text: String) -> Div {
    div()
        .rounded_xl()
        .bg(bg)
        .text_color(fg)
        .px(px(12.))
        .py(px(10.))
        .flex()
        .flex_row()
        .items_start()
        .gap(px(10.))
        .child(
            div()
                .size(px(18.))
                .flex_none()
                .rounded_full()
                .border_1()
                .border_color(fg)
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .child(icon),
        )
        // min width 0 lets the message wrap instead of overflowing.
        .child(div().text_xs().flex_1().min_w(px(0.)).child(text))
}

/// Body text in the supporting color.
fn supporting(t: &Theme, text: impl Into<String>) -> Div {
    div().text_xs().text_color(t.on_surface_variant).child(text.into())
}

impl Panel {
    fn status_tab(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let state = cx.global::<AppState>();
        let status = state.status.clone();
        let error = state.error.clone();
        let updated_line = match state.updated.map(|u| u.elapsed().as_secs()) {
            Some(s) if s < 60 => format!("updated {s}s ago"),
            Some(s) => format!("updated {}m ago", s / 60),
            None => "waiting for first reading…".to_string(),
        };

        let dash = || "—".to_string();
        let temp = status.as_ref().map(|s| s.temperature_label()).unwrap_or_else(dash);
        let temp_c = status.as_ref().and_then(|s| s.temperature_c());
        let heat = temp_c.map(Heat::from_celsius);
        let net = status.as_ref().map(|s| s.network_label()).unwrap_or_else(dash);
        let band = status.as_ref().map(|s| s.band_label()).unwrap_or_else(dash);
        let operator = status
            .as_ref()
            .and_then(|s| s.operator.clone())
            .filter(|o| !o.is_empty() && o != "NULL")
            .unwrap_or_else(dash);
        // With carrier aggregation the fields hold one value per carrier;
        // the tiles show the primary carrier and a table lists them all.
        let carriers = status.as_ref().map(|s| s.carriers()).unwrap_or_default();
        let primary = carriers.first();
        let earfcn = primary.and_then(|c| c.earfcn.clone()).unwrap_or_else(dash);
        let width = primary
            .and_then(|c| c.width_mhz.clone())
            .map(|w| format!("{w} MHz"))
            .unwrap_or_else(dash);
        let cell = status.as_ref().and_then(|s| s.cell_id.clone()).unwrap_or_else(dash);
        let pci = primary.and_then(|c| c.pci.clone()).unwrap_or_else(dash);
        let cpu = status
            .as_ref()
            .and_then(|s| s.cpu_load.clone())
            .map(|c| format!("{c}%"))
            .unwrap_or_else(dash);
        let uptime = status.as_ref().map(|s| s.uptime_label()).unwrap_or_else(dash);
        let bars = status.as_ref().and_then(|s| s.signal_bars());
        let rsrp = status.as_ref().and_then(|s| s.rsrp_dbm());
        let rsrq = status.as_ref().and_then(|s| s.rsrq_db());
        let sinr = status.as_ref().and_then(|s| s.sinr_db());

        // A pill chip on top of one of the theme's chip styles.
        let chip = |base: Div, text: String| {
            base.h(px(26.))
                .px(px(12.))
                .flex()
                .items_center()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .child(text)
        };

        // One RF reading: name, value and grade over a meter.
        let reading = |name: &str, value: Option<f32>, unit: &str, grade: fn(f32) -> Quality, range: (f32, f32)| {
            let quality = value.map(grade);
            let color = quality.map(|q| t.quality_color(q)).unwrap_or(t.outline);
            let value_text = match value {
                Some(v) => format!("{v} {unit}"),
                None => "—".to_string(),
            };
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .child(t.overline(name).w(px(40.)))
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(value_text),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(color)
                                .child(quality.map(Quality::label).unwrap_or("")),
                        ),
                )
                .child(t.meter(fraction(value, range.0, range.1), color))
        };

        let mut content = div().flex().flex_col().gap(px(12.));

        // ---- hero: link quality + temperature gauge --------------------------
        let heat_color = heat.map(|h| t.heat_color(h)).unwrap_or(t.outline);
        let bars_color = t.signal_color(bars.unwrap_or(0));
        content = content.child(
            t.hero()
                .id("hero")
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_start()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(t.overline("MODEM TEMPERATURE"))
                                .child(
                                    div()
                                        .text_size(px(48.))
                                        .line_height(px(56.))
                                        .font_weight(FontWeight::BOLD)
                                        .child(temp),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_end()
                                .gap(px(6.))
                                .pt(px(4.))
                                .child(t.signal_bars(bars.unwrap_or(0)))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(bars_color)
                                        .child(match bars {
                                            Some(b) => format!("{} signal", signal_label(b)),
                                            None => "no signal data".to_string(),
                                        }),
                                ),
                        ),
                )
                // Gauge spans 30–90 °C, the modem's realistic operating range.
                .child(t.meter(fraction(temp_c, 30.0, 90.0), heat_color))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .text_xs()
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(heat_color)
                                .child(heat.map(Heat::label).unwrap_or("—")),
                        )
                        .child(div().text_color(t.on_surface_variant).child(updated_line)),
                )
                .when(status.is_some(), |hero| {
                    hero.child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.))
                            .child(chip(t.primary_tonal(), net))
                            .child(chip(t.tonal(), band.clone()))
                            .child(chip(t.outlined(), operator)),
                    )
                }),
        );

        if let Some(err) = error {
            content = content.child(banner(t.error_container, t.on_error_container, "!", err));
        }

        // ---- signal ------------------------------------------------------------
        content = content.child(section(t, "SIGNAL"));
        content = content.child(
            t.card()
                .id("signal")
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(reading("RSRP", rsrp, "dBm", Quality::rsrp, (-120.0, -70.0)))
                .child(reading("RSRQ", rsrq, "dB", Quality::rsrq, (-20.0, -3.0)))
                .child(reading("SINR", sinr, "dB", Quality::sinr, (-5.0, 30.0))),
        );

        // ---- cell & device -----------------------------------------------------
        content = content.child(section(t, "CELL & DEVICE"));
        content.child(
            t.card()
                .id("details")
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(if carriers.len() > 1 {
                    t.carrier_table(&carriers)
                } else {
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(10.))
                        .child(t.tile("BAND", band.clone()))
                        .child(t.tile("EARFCN", earfcn))
                        .child(t.tile("WIDTH", width))
                })
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(10.))
                        .child(t.tile("CELL", cell))
                        .child(t.tile("PCI", pci))
                        .child(t.tile("CPU", cpu)),
                )
                .child(div().h(px(1.)).bg(t.hairline))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(t.overline("UPTIME"))
                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(uptime)),
                ),
        )
    }

    /// Busy / result banners for router settings tasks.
    fn router_banners(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let state = cx.global::<AppState>();
        let mut col = div().flex().flex_col().gap(px(8.));
        if let Some(busy) = &state.router_busy {
            col = col.child(banner(t.primary_container, t.on_primary_container, "…", format!("{busy}…")));
        }
        match &state.router_note {
            Some(Ok(msg)) => {
                col = col.child(banner(t.secondary_container, t.on_secondary_container, "✓", msg.clone()))
            }
            Some(Err(err)) => {
                col = col.child(banner(t.error_container, t.on_error_container, "!", err.clone()))
            }
            None => {}
        }
        col
    }

    fn network_tab(&self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> Div {
        let state = cx.global::<AppState>();
        let busy = state.router_busy.is_some();
        let Some(router) = state.router.clone() else {
            let message = if busy {
                "Reading the router's settings…"
            } else {
                "The router's settings haven't been read yet. They load once the router is reachable; use Reload to try now."
            };
            return div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(self.router_banners(t, cx))
                .child(t.card().child(supporting(t, message)));
        };
        let base = NetworkDraft::from_settings(&router);
        let draft = self.draft.clone().unwrap_or_else(|| base.clone());
        let dirty = draft != base;
        let problem = draft.problem();

        let mut content = div().flex().flex_col().gap(px(12.)).child(self.router_banners(t, cx));

        // ---- network mode ----------------------------------------------------
        let mut modes = t.card().id("modes").flex().flex_col().gap(px(2.)).py(px(8.));
        for mode in NetworkMode::ALL {
            let selected = draft.mode.as_deref() == Some(mode.code());
            let code = mode.code().to_string();
            modes = modes.child(
                t.radio_row(selected, mode.label(), None)
                    .id(mode.code())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let code = code.clone();
                        this.edit(cx, |d| d.mode = Some(code));
                    })),
            );
        }
        if let Some(code) = draft.mode.as_deref().filter(|c| NetworkMode::from_code(c).is_none()) {
            modes = modes.child(supporting(t, format!("Router reports an unlisted mode: {code}")).px(px(8.)));
        }
        content = content.child(section(t, "NETWORK MODE")).child(modes);

        // ---- band locks --------------------------------------------------------
        content = content
            .child(section(t, "4G BAND LOCK"))
            .child(self.band_card(t, cx, &router.lte_supported, false, draft.lte_lock, &draft.lte));
        // 5G only matters once the (picked) network mode can use it.
        if draft.mode.as_deref().is_some_and(mode_allows_nr) && !router.nr_supported.is_empty() {
            content = content
                .child(section(t, "5G BAND LOCK"))
                .child(self.band_card(t, cx, &router.nr_supported, true, draft.nr_lock, &draft.nr));
        }

        // ---- operator ------------------------------------------------------------
        let mut ops = t.card().id("operators").flex().flex_col().gap(px(2.)).py(px(8.));
        let choices = std::iter::once((OperatorSelection::Automatic, "Automatic".to_string(), "Let the modem choose".to_string()))
            .chain(OPERATORS.into_iter().map(|(plmn, name)| {
                (OperatorSelection::Manual(plmn.to_string()), name.to_string(), format!("PLMN {plmn} · no roaming fallback"))
            }));
        for (i, (selection, label, note)) in choices.enumerate() {
            let selected = draft.operator.as_ref() == Some(&selection);
            ops = ops.child(
                t.radio_row(selected, &label, Some(&note))
                    .id(("operator", i))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let selection = selection.clone();
                        this.edit(cx, |d| d.operator = Some(selection));
                    })),
            );
        }
        if draft.operator.is_none() {
            ops = ops.child(supporting(t, "Current selection unknown (the modem didn't answer AT+COPS?).").px(px(8.)));
        } else if let Some(OperatorSelection::Manual(plmn)) = &draft.operator {
            if operator_name(plmn).is_none() {
                ops = ops.child(supporting(t, format!("Pinned to PLMN {plmn}.")).px(px(8.)));
            }
        }
        content = content.child(section(t, "OPERATOR")).child(ops);

        // ---- apply bar -------------------------------------------------------
        let can_apply = dirty && !busy && problem.is_none();
        let apply = t
            .filled()
            .id("btn-apply")
            .flex_1()
            .h(px(44.))
            .flex()
            .items_center()
            .justify_center()
            .text_sm()
            .child(if busy { "Working…" } else { "Apply changes" });
        let apply = if can_apply {
            apply
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.apply_network(cx)))
        } else {
            t.disabled(apply)
        };
        let mut bar = div().flex().flex_row().items_center().gap(px(8.)).child(apply);
        if dirty {
            bar = bar.child(
                t.text_button()
                    .id("btn-discard")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.draft = None;
                        cx.notify();
                    }))
                    .child("Discard"),
            );
        }
        content = content.child(bar);
        content = content.child(supporting(
            t,
            problem.unwrap_or("Changes go to the router only when you apply them. Mode, band and operator changes make the modem re-attach, which drops the connection for up to a minute."),
        ));

        // ---- physical cell lock (applied per radio, like the web UI) ---------------
        match &router.cell_lock {
            Some(lock) => {
                let nr = router.allows_nr();
                content = content.child(section(t, "4G LOCKED CELL")).child(self.cell_card(t, window, cx, false, lock, busy));
                if nr {
                    content = content
                        .child(section(t, "5G LOCKED CELL"))
                        .child(self.cell_card(t, window, cx, true, lock, busy))
                        .child(banner(
                            t.surface_container_high,
                            t.on_surface,
                            "!",
                            "In NSA mode, lock the 4G and the 5G cell at the same time.".to_string(),
                        ));
                }
                content
            }
            None => content
                .child(section(t, "CELL LOCK"))
                .child(t.card().child(supporting(t, "The router doesn't show the cell lock to this login."))),
        }
    }

    /// One radio's physical cell lock: status, unlock / lock choice, the cell
    /// values (with "Use current cell") and its own Apply.
    fn cell_card(&self, t: &Theme, window: &Window, cx: &mut Context<Self>, nr: bool, lock: &CellLock, busy: bool) -> Div {
        let (state, router_on, choice) = if nr {
            (lock.nr_state, lock.nr.is_some(), self.nr_cell_lock)
        } else {
            (lock.lte_state, lock.lte.is_some(), self.lte_cell_lock)
        };
        let on = choice.unwrap_or(router_on);
        let key = if nr { "nr" } else { "lte" };

        let state_color = match state {
            LockState::Locked => t.good,
            LockState::Failed => t.bad,
            LockState::Unlocked => t.on_surface_variant,
        };
        let status = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(div().text_sm().child("Locked status"))
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(state_color).child(state.label()));

        let radio = |id: &'static str, selected: bool, label: &str, lock_on: bool| {
            t.radio_row(selected, label, None).id(id).on_click(cx.listener(move |this, _, _, cx| {
                if nr {
                    this.nr_cell_lock = Some(lock_on);
                } else {
                    this.lte_cell_lock = Some(lock_on);
                }
                cx.notify();
            }))
        };
        let (unlock_id, lock_id) = if nr { ("nr-unlock", "nr-lock-cell") } else { ("lte-unlock", "lte-lock-cell") };
        let mut card = t
            .card()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(status)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(radio(unlock_id, !on, "Unlock", false))
                    .child(radio(lock_id, on, "Lock specific physical cell", true)),
            );

        let problem = if on {
            let fields: Vec<usize> = if nr { vec![NR_BAND, NR_ARFCN, NR_PCI] } else { vec![LTE_EARFCN, LTE_PCI] };
            let mut row = div().flex().flex_row().gap(px(8.));
            for i in fields {
                row = row.child(self.field_cell(t, i, window, cx).flex_1());
            }
            let current = cx.global::<AppState>().status.as_ref().is_some_and(|s| {
                if nr { s.current_nr_cell().is_some() } else { s.current_lte_cell().is_some() }
            });
            let use_current = t
                .text_button()
                .id((key, 1usize))
                .child(if nr { "Use current 5G cell" } else { "Use current 4G cell" });
            let use_current = if current {
                use_current.on_click(cx.listener(move |this, _, _, cx| this.use_current_cell(nr, cx)))
            } else {
                t.disabled(use_current)
            };
            card = card.child(row).child(div().flex().flex_row().child(use_current));
            if nr { validate_nr_cells(&[self.nr_cell()]).err() } else { validate_lte_cells(&[self.lte_cell()]).err() }
        } else {
            None
        };

        let apply = t
            .filled()
            .id((key, 2usize))
            .h(px(36.))
            .px(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .text_sm()
            .child("Apply");
        let apply = if !busy && problem.is_none() {
            apply.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.apply_cell_lock(nr, cx)))
        } else {
            t.disabled(apply)
        };
        let mut footer = div().flex().flex_row().items_center().gap(px(12.)).child(apply);
        if let Some(p) = problem {
            footer = footer.child(div().flex_1().text_xs().text_color(t.bad).child(p));
        }
        card.child(footer)
    }

    /// Lock switch plus a filter chip per band.
    fn band_card(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
        bands: &[u32],
        nr: bool,
        locked: bool,
        picked: &BTreeSet<u32>,
    ) -> impl IntoElement {
        let (prefix, id) = if nr { ("n", "nr-lock") } else { ("B", "lte-lock") };
        let switch = div()
            .id(id)
            .cursor_pointer()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_sm().child(if locked { "Locked to selected bands" } else { "All bands allowed" }))
                    .child(supporting(t, if nr { "5G (NR) radio" } else { "4G (LTE) radio" })),
            )
            .child(t.switch_track(locked))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.edit(cx, |d| {
                    if nr {
                        d.nr_lock = !d.nr_lock;
                    } else {
                        d.lte_lock = !d.lte_lock;
                    }
                })
            }));

        let mut chips = div().flex().flex_row().flex_wrap().gap(px(6.));
        for &band in bands {
            let on = picked.contains(&band);
            chips = chips.child(
                t.filter_chip(on, format!("{prefix}{band}"))
                    .id((id, band as usize))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.edit(cx, |d| {
                            let (lock, set) = if nr { (&mut d.nr_lock, &mut d.nr) } else { (&mut d.lte_lock, &mut d.lte) };
                            if !set.remove(&band) {
                                set.insert(band);
                            }
                            // Picking a band implies locking to the picked set.
                            *lock = true;
                        })
                    })),
            );
        }

        t.card()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(switch)
            .child(div().when(!locked, |d| d.opacity(0.5)).child(chips))
    }

    fn device_tab(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let state = cx.global::<AppState>();
        let busy = state.router_busy.is_some();
        let status = state.status.clone();
        let dash = || "—".to_string();
        let board = status.as_ref().and_then(|s| s.board.clone()).unwrap_or_else(dash);
        let uptime = status.as_ref().map(|s| s.uptime_label()).unwrap_or_else(dash);
        let cpu = status
            .as_ref()
            .and_then(|s| s.cpu_load.clone())
            .map(|c| format!("{c}%"))
            .unwrap_or_else(dash);
        let temp = status.as_ref().map(|s| s.temperature_label()).unwrap_or_else(dash);

        let mut content = div().flex().flex_col().gap(px(12.)).child(self.router_banners(t, cx));

        content = content.child(section(t, "ROUTER")).child(
            t.card()
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(10.))
                        .child(t.tile("BOARD", board))
                        .child(t.tile("UPTIME", uptime)),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(10.))
                        .child(t.tile("CPU", cpu))
                        .child(t.tile("TEMPERATURE", temp)),
                ),
        );

        // ---- restart -----------------------------------------------------------
        let armed = self.confirm_reboot;
        let restart = t
            .danger(armed)
            .id("btn-reboot")
            .child(if armed { "Click again to restart" } else { "Restart router" });
        let restart = if busy {
            t.disabled(restart)
        } else {
            restart.on_click(cx.listener(|this, _, _, cx| this.reboot(cx)))
        };
        let mut row = div().flex().flex_row().items_center().gap(px(8.)).child(restart);
        if armed {
            row = row.child(
                t.text_button()
                    .id("btn-reboot-cancel")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.confirm_reboot = false;
                        cx.notify();
                    }))
                    .child("Cancel"),
            );
        }
        content = content.child(section(t, "POWER")).child(
            t.card()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(supporting(t, "Restarting takes the internet down for about a minute. Settings are kept."))
                .child(row),
        );

        // ---- capture -------------------------------------------------------------
        let capture = t
            .tonal()
            .id("btn-capture")
            .h(px(36.))
            .px(px(16.))
            .flex()
            .items_center()
            .justify_center()
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .child(if self.capturing { "Capturing…" } else { "Capture web UI" });
        let capture = if self.capturing {
            t.disabled(capture)
        } else {
            capture
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.capture_web_ui(cx)))
        };
        let mut card = t
            .card()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(supporting(
                t,
                "Wi-Fi, APN, LAN and SMS settings aren't mapped yet. While connected to the router, capture its web UI: the app saves the router's own scripts and indexes every API command they use, which is what's needed to add those settings.",
            ))
            .child(div().flex().flex_row().child(capture));
        match &self.capture_note {
            Some(Ok(msg)) => card = card.child(supporting(t, msg.clone())),
            Some(Err(err)) => card = card.child(div().text_xs().text_color(t.bad).child(err.clone())),
            None => {}
        }
        content.child(section(t, "MORE SETTINGS")).child(card)
    }

    fn app_tab(&self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> Div {
        let mut settings = t.card().id("settings").flex().flex_col().gap(px(14.)).py(px(14.));
        settings = settings.child(self.field_cell(t, 0, window, cx));
        settings = settings.child(
            div()
                .flex()
                .flex_row()
                .gap(px(10.))
                .child(self.field_cell(t, 1, window, cx).flex_1())
                .child(self.field_cell(t, 2, window, cx).flex_1()),
        );

        // poll interval stepper
        let stepper = |id: &'static str, label: &'static str| {
            t.round_tonal()
                .id(id)
                .cursor_pointer()
                .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(label))
        };
        settings = settings.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(t.overline("POLL EVERY"))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(format!("{} seconds", self.poll_secs)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            stepper("poll-minus", "−")
                                .on_click(cx.listener(|this, _, _, cx| this.adjust_poll(-5, cx))),
                        )
                        .child(
                            stepper("poll-plus", "+")
                                .on_click(cx.listener(|this, _, _, cx| this.adjust_poll(5, cx))),
                        ),
                ),
        );

        // menu bar text switch
        let menu_title = self.menu_title;
        settings = settings.child(
            div()
                .id("menu-title")
                .cursor_pointer()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(10.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(t.overline("TEMPERATURE IN MENU BAR"))
                        .child(supporting(t, if menu_title { "shown next to the icon" } else { "hidden" })),
                )
                .child(t.switch_track(menu_title))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.menu_title = !this.menu_title;
                    this.note = Some("press Save to apply".to_string());
                    cx.notify();
                })),
        );

        let mut content = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(section(t, "CONNECTION & DISPLAY"))
            .child(settings)
            .child(
                t.filled()
                    .id("btn-save")
                    .cursor_pointer()
                    .h(px(44.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                    .child("Save changes"),
            );
        if let Some(note) = self.note.clone() {
            content = content.child(supporting(t, note).flex().justify_center());
        }
        content
    }
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::for_appearance(window.appearance());
        let (refreshing, reading) = {
            let state = cx.global::<AppState>();
            (state.pending_refresh, state.router_busy.is_some())
        };

        // ---- top strip: room for the traffic lights ---------------------------
        let strip = div().h(px(44.)).flex_none();

        // ---- headline + per-tab action -------------------------------------------
        type Listener = Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut App)>;
        let action: Option<(&'static str, &'static str, Listener)> = match self.tab {
            Tab::Status => Some((
                "btn-refresh",
                if refreshing { "Refreshing…" } else { "Refresh" },
                Box::new(cx.listener(|_, _, _, cx| {
                    cx.global_mut::<AppState>().pending_refresh = true;
                    cx.notify();
                })),
            )),
            Tab::Network => Some((
                "btn-reload",
                if reading { "Working…" } else { "Reload" },
                Box::new(cx.listener(|this, _, _, cx| {
                    this.draft = None;
                    read_router_settings(cx);
                })),
            )),
            Tab::Device | Tab::App => None,
        };
        let mut headline = div()
            .flex()
            .flex_row()
            .items_end()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(t.overline("X28 PRO").text_color(t.primary))
                    .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child(self.tab.label())),
            );
        if let Some((id, label, listener)) = action {
            headline = headline.child(
                t.tonal()
                    .id(id)
                    .cursor_pointer()
                    .h(px(34.))
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .on_click(move |event, window, cx| listener(event, window, cx))
                    .child(div().text_base().child("↻"))
                    .child(label),
            );
        }

        // ---- tab bar ---------------------------------------------------------------
        let mut tabs = t.segmented();
        for tab in Tab::ALL {
            tabs = tabs.child(
                t.segment(tab == self.tab)
                    .id(tab.id())
                    .on_click(cx.listener(move |this, _, _, cx| this.select_tab(tab, cx)))
                    .child(tab.label()),
            );
        }

        let body = match self.tab {
            Tab::Status => self.status_tab(&t, cx),
            Tab::Network => {
                self.sync_cell_fields(cx);
                self.network_tab(&t, window, cx)
            }
            Tab::Device => self.device_tab(&t, cx),
            Tab::App => self.app_tab(&t, window, cx),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.backdrop)
            .text_color(t.on_surface)
            .on_key_down(cx.listener(|this, event, window, cx| this.on_key(event, window, cx)))
            .child(strip)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .px(px(16.))
                    .pb(px(12.))
                    .child(headline)
                    .child(tabs),
            )
            .child(
                div()
                    .id("scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(div().px(px(16.)).pb(px(18.)).child(body)),
            )
    }
}

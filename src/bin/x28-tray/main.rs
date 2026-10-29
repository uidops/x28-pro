//! X28 PRO menu bar tray — live router status and router settings.
//!
//! Layout of the program:
//!   * `AppState`  — global config, last status and router settings
//!   * tray        — signal-bars icon (see `icon`) with the live temperature
//!     as its optional title; a click opens the `Popover` (see `popover`):
//!     live link, stats and quick settings on a Material surface
//!   * `Panel`     — the full window (see `panel`): tabs for status, network
//!     settings, device actions and app settings
//!
//! Polling runs in one detached GPUI task: sleep in 1 s slices (so interval
//! changes and manual refreshes take effect quickly), fetch on the background
//! executor, then push the result into the globals, the tray and the windows.
//! Router settings changes run as one-at-a-time background tasks
//! (`router_task`) that re-read the settings afterwards.
//!
//! The app lives in the menu bar: it shows in the Dock and Stage Manager only
//! while the panel is open, and Cmd+Q / Cmd+W close the panel rather than
//! quitting. Quitting for real is in the popover (or Option-Cmd+Q).

mod appkit;
mod assets;
mod icon;
mod panel;
mod popover;
mod theme;

use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{
    actions, point, px, size, AnyWindowHandle, App, Application, AsyncApp, Bounds, Entity, Global,
    KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions,
};
use gpui_tray::Tray;
use x28_pro::{
    fetch_once, operator_name, with_client, Client, Config, NetworkMode, OperatorSelection,
    RouterSettings, Status,
};

use icon::Glyph;
use panel::{Panel, Tab};
use popover::Popover;

actions!(x28_tray, [Refresh, ShowPanel, TogglePopover, TogglePause, CloseToTray, Quit]);

/// Shown after any change that makes the modem re-attach to the network.
const REATTACH_NOTE: &str = "The connection drops for up to a minute while the modem re-attaches.";

struct AppState {
    config: Config,
    session: Option<String>,
    status: Option<Status>,
    error: Option<String>,
    updated: Option<Instant>,
    pending_refresh: bool,
    /// Background polling is paused (manual refreshes still run).
    paused: bool,
    /// Network mode, band lock and operator as last read from the router.
    router: Option<RouterSettings>,
    /// Set once the first automatic read of `router` has been started.
    router_requested: bool,
    /// What the running settings task is doing; `None` when idle.
    router_busy: Option<String>,
    /// Outcome of the last settings task, shown in the windows until
    /// `show_note`'s timer clears it.
    router_note: Option<Result<String, String>>,
    /// Bumped per note, so an old note's timer leaves a newer note alone.
    router_note_seq: u64,
}

impl Global for AppState {}

struct AppTray {
    tray: Tray,
    /// Icon currently shown, so it is only replaced on change.
    glyph: Glyph,
}
impl Global for AppTray {}

#[derive(Default)]
struct PanelRef(Option<(AnyWindowHandle, Entity<Panel>)>);
impl Global for PanelRef {}

#[derive(Default)]
struct PopoverRef {
    open: Option<(AnyWindowHandle, Entity<Popover>)>,
    /// Watches for clicks outside the app while the popover is open.
    monitor: Option<appkit::ClickMonitor>,
    /// When the popover last closed by losing focus (see `toggle_popover`).
    closed_at: Option<Instant>,
}
impl Global for PopoverRef {}

/// Close the popover if it is open. `dismissed` marks a close caused by a
/// click elsewhere, which `toggle_popover` must not undo.
fn close_popover(cx: &mut App, dismissed: bool) {
    let popover = cx.global_mut::<PopoverRef>();
    popover.monitor = None;
    if dismissed {
        popover.closed_at = Some(Instant::now());
    }
    if let Some((handle, _)) = popover.open.take() {
        handle.update(cx, |_, window, _| window.remove_window()).ok();
    }
}

/// Forget windows once they close. The callback doesn't say which window
/// closed, so check which ones are still open. The subscription is kept for
/// the app's lifetime (subscriptions unsubscribe on drop).
fn keep_window_subscription(cx: &mut App) {
    std::mem::forget(cx.on_window_closed(|cx| {
        let open = cx.windows();
        let alive = |h: &AnyWindowHandle| open.iter().any(|w| w.window_id() == h.window_id());
        if cx.global::<PanelRef>().0.as_ref().is_some_and(|(h, _)| !alive(h)) {
            cx.global_mut::<PanelRef>().0 = None;
            // Back to living in the menu bar only.
            appkit::set_in_dock(false);
        }
        if cx.global::<PopoverRef>().open.as_ref().is_some_and(|(h, _)| !alive(h)) {
            let popover = cx.global_mut::<PopoverRef>();
            popover.open = None;
            popover.monitor = None;
        }
    }));
}

fn main() {
    Application::new().with_assets(assets::Assets).run(|cx: &mut App| {
        cx.set_global(AppState {
            config: Config::load(),
            session: None,
            status: None,
            error: None,
            updated: None,
            pending_refresh: true,
            paused: false,
            router: None,
            router_requested: false,
            router_busy: None,
            router_note: None,
            router_note_seq: 0,
        });
        cx.set_global(PanelRef::default());
        cx.set_global(PopoverRef::default());

        cx.on_action(refresh_now);
        cx.on_action(show_panel);
        cx.on_action(toggle_popover);
        cx.on_action(toggle_pause);
        cx.on_action(close_to_tray);
        cx.on_action(quit_app);
        keep_window_subscription(cx);

        // Cmd+Q and Cmd+W close the panel and keep the tray running; the menu
        // carries the shortcuts so they work whatever has focus.
        cx.bind_keys([
            KeyBinding::new("cmd-q", CloseToTray, None),
            KeyBinding::new("cmd-w", CloseToTray, None),
            KeyBinding::new("alt-cmd-q", Quit, None),
        ]);
        cx.set_menus(vec![Menu {
            name: "X28 PRO".into(),
            items: vec![
                MenuItem::action("Refresh", Refresh),
                MenuItem::separator(),
                MenuItem::action("Close to Menu Bar", CloseToTray),
                MenuItem::action("Quit X28 PRO Completely", Quit),
            ],
        }]);

        // No native menu: a click on the status item opens the popover.
        let glyph = Glyph::for_state(None, false);
        let tray = Tray::builder()
            .icon(glyph.icon(cx))
            .tooltip("X28 PRO — connecting")
            .on_activate(TogglePopover)
            .build(cx)
            .unwrap_or_else(|e| panic!("creating tray icon: {e}"));
        cx.set_global(AppTray { tray, glyph });
        eprintln!(
            "x28-tray: tray ready, polling every {}s → {}",
            cx.global::<AppState>().config.poll_secs,
            Config::path().display()
        );

        start_polling(cx);

        if std::env::args().any(|a| a == "--open-panel") {
            show_panel(&ShowPanel, cx);
        } else {
            appkit::set_in_dock(false);
        }
    });
}

fn ellipsize(text: &str, max: usize) -> String {
    let mut short: String = text.chars().take(max).collect();
    if short.len() < text.len() {
        short.push('…');
    }
    short
}

// ---------------------------------------------------------------------------
// Router settings
// ---------------------------------------------------------------------------

/// Run one router settings operation in the background, then re-read the
/// settings (when `reread`) so the tray and panel show what the router really
/// has. Only one runs at a time; a request while busy is dropped. `op` returns
/// the message to show on success (empty for a plain read).
fn router_task(
    cx: &mut App,
    busy: impl Into<String>,
    reread: bool,
    op: impl FnOnce(&mut Client) -> Result<String, String> + Send + 'static,
) {
    let (config, session) = {
        let state = cx.global_mut::<AppState>();
        if state.router_busy.is_some() {
            return;
        }
        state.router_busy = Some(busy.into());
        (state.config.clone(), state.session.clone())
    };
    redraw(cx);

    cx.spawn(async move |cx: &mut AsyncApp| {
        let (result, session) = cx
            .background_executor()
            .spawn(async move {
                with_client(&config, session, |client| {
                    let message = op(client)?;
                    let settings = reread.then(|| client.router_settings());
                    Ok((message, settings))
                })
            })
            .await;

        cx.update(|cx| {
            let note = {
                let state = cx.global_mut::<AppState>();
                state.router_busy = None;
                state.session = session;
                match result {
                    Ok((message, settings)) => {
                        let read_error = match settings {
                            Some(Ok(settings)) => {
                                state.router = Some(settings);
                                None
                            }
                            // A failed read-back after a write is expected while
                            // the modem re-attaches; only report it for reads.
                            Some(Err(e)) if message.is_empty() => Some(Err(e)),
                            _ => None,
                        };
                        if message.is_empty() { read_error } else { Some(Ok(message)) }
                    }
                    Err(e) => Some(Err(e)),
                }
            };
            match note {
                Some(note) => show_note(cx, note),
                None => redraw(cx),
            }
        })
        .ok();
    })
    .detach();
}

/// Show a settings outcome, then clear it: confirmations after a few
/// seconds, errors after long enough to read them.
fn show_note(cx: &mut App, note: Result<String, String>) {
    let linger = Duration::from_secs(if note.is_ok() { 6 } else { 15 });
    let seq = {
        let state = cx.global_mut::<AppState>();
        state.router_note = Some(note);
        state.router_note_seq += 1;
        state.router_note_seq
    };
    redraw(cx);
    cx.spawn(async move |cx: &mut AsyncApp| {
        cx.background_executor().timer(linger).await;
        cx.update(|cx| {
            let state = cx.global_mut::<AppState>();
            if state.router_note_seq == seq && state.router_note.is_some() {
                state.router_note = None;
                redraw(cx);
            }
        })
        .ok();
    })
    .detach();
}

fn read_router_settings(cx: &mut App) {
    router_task(cx, "Reading router settings", true, |_| Ok(String::new()));
}

fn apply_network_mode(mode: NetworkMode, cx: &mut App) {
    router_task(cx, format!("Switching to {}", mode.label()), true, move |c| {
        c.set_network_mode(mode.code())?;
        Ok(format!("Network mode set to {}. {REATTACH_NOTE}", mode.label()))
    });
}

fn apply_operator(selection: OperatorSelection, cx: &mut App) {
    let name = match &selection {
        OperatorSelection::Automatic => "automatic".to_string(),
        OperatorSelection::Manual(plmn) => operator_name(plmn).unwrap_or(plmn).to_string(),
    };
    router_task(cx, format!("Selecting {name} network"), true, move |c| {
        c.set_operator(&selection)?;
        Ok(format!("Network selection: {name}. {REATTACH_NOTE}"))
    });
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

fn refresh_now(_: &Refresh, cx: &mut App) {
    cx.global_mut::<AppState>().pending_refresh = true;
}

/// Stop or resume background polling. Resuming fetches straight away.
fn toggle_pause(_: &TogglePause, cx: &mut App) {
    let state = cx.global_mut::<AppState>();
    state.paused = !state.paused;
    if !state.paused {
        state.pending_refresh = true;
    }
    redraw(cx);
}

fn show_panel(_: &ShowPanel, cx: &mut App) {
    // A window of its own: show in the Dock and Stage Manager while it's open.
    appkit::set_in_dock(true);
    cx.activate(true);
    if let Some((handle, _)) = cx.global::<PanelRef>().0.clone() {
        handle.update(cx, |_, window, _| window.activate_window()).ok();
        return;
    }

    let bounds = Bounds::centered(None, size(px(440.), px(820.)), cx);
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // Content runs under a transparent titlebar; the traffic lights sit
            // inside the panel's own top strip.
            titlebar: Some(TitlebarOptions {
                title: Some("X28 PRO".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(16.), px(16.))),
            }),
            // Translucent surfaces are layered over the blurred desktop.
            window_background: WindowBackgroundAppearance::Blurred,
            window_min_size: Some(size(px(400.), px(480.))),
            kind: WindowKind::Normal,
            focus: true,
            show: true,
            is_movable: true,
            is_resizable: true,
            ..Default::default()
        },
        |window, cx| cx.new(|cx| Panel::new(window, cx)),
    );
    match opened.and_then(|handle| handle.update(cx, |_, _, cx| (handle.into(), cx.entity()))) {
        Ok(refs) => cx.global_mut::<PanelRef>().0 = Some(refs),
        Err(e) => eprintln!("could not open panel: {e}"),
    }
}

fn open_panel_at(tab: Tab, cx: &mut App) {
    show_panel(&ShowPanel, cx);
    if let Some((_, panel)) = cx.global::<PanelRef>().0.clone() {
        panel.update(cx, |panel, cx| panel.select_tab(tab, cx));
    }
}

/// Open the popover under the status item, or close it if it is open.
fn toggle_popover(_: &TogglePopover, cx: &mut App) {
    if cx.global::<PopoverRef>().open.is_some() {
        close_popover(cx, false);
        return;
    }
    // A click on the status item first takes focus from an open popover,
    // which closes it; that same click must not reopen it.
    let just_closed = cx
        .global::<PopoverRef>()
        .closed_at
        .is_some_and(|t| t.elapsed() < Duration::from_millis(400));
    if just_closed {
        return;
    }

    let popover_size = size(px(popover::WIDTH), px(popover::HEIGHT));
    let (display_id, bounds) = match appkit::popover_bounds(popover::WIDTH, popover::HEIGHT, cx) {
        Some((display, bounds)) => (Some(display), bounds),
        // No AppKit position (should not happen): top-right of the main screen.
        None => {
            let display = cx.primary_display();
            let width = display.as_ref().map(|d| d.bounds().size.width).unwrap_or(px(1440.));
            let origin = point(width - px(popover::WIDTH + 8.), px(40.));
            (display.map(|d| d.id()), Bounds { origin, size: popover_size })
        }
    };
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            kind: WindowKind::PopUp,
            window_background: WindowBackgroundAppearance::Blurred,
            display_id,
            focus: true,
            show: true,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            ..Default::default()
        },
        |window, cx| cx.new(|cx| Popover::new(window, cx)),
    );
    let refs = opened.and_then(|handle| {
        handle.update(cx, |_, window, cx| {
            // Take focus so a click anywhere else closes the popover.
            window.activate_window();
            (handle.into(), cx.entity())
        })
    });
    let refs = match refs {
        Ok(refs) => refs,
        Err(e) => return eprintln!("could not open popover: {e}"),
    };

    // A click in another app or on the menu bar doesn't always take focus
    // from the panel, so watch for those clicks too. Dropping the monitor
    // (on close) drops the sender, which ends the task.
    let (clicked, outside_click) = async_channel::bounded::<()>(1);
    let monitor = appkit::watch_outside_clicks(move || {
        let _ = clicked.try_send(());
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        if outside_click.recv().await.is_ok() {
            cx.update(|cx| close_popover(cx, true)).ok();
        }
    })
    .detach();

    let popover = cx.global_mut::<PopoverRef>();
    popover.open = Some(refs);
    popover.monitor = monitor;
}

/// Close the panel (and popover) but keep running in the menu bar; closing
/// the panel drops the app from the Dock (see `keep_window_subscription`).
fn close_to_tray(_: &CloseToTray, cx: &mut App) {
    // Deferred: the shortcut arrives while gpui is dispatching to the panel
    // window, which can't be updated (closed) from inside that dispatch.
    cx.defer(|cx| {
        close_popover(cx, false);
        if let Some((handle, _)) = cx.global::<PanelRef>().0.clone() {
            handle.update(cx, |_, window, _| window.remove_window()).ok();
        }
    });
}

fn quit_app(_: &Quit, cx: &mut App) {
    if cx.has_global::<AppTray>() {
        let tray = cx.global::<AppTray>().tray.clone();
        if let Err(e) = tray.close(cx) {
            eprintln!("closing tray: {e}");
        }
    }
    cx.quit();
}

// ---------------------------------------------------------------------------
// Polling
// ---------------------------------------------------------------------------

fn start_polling(cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            let interval = match cx.read_global::<AppState, u64>(|state, _| state.config.poll_secs) {
                Ok(v) => v,
                Err(_) => return, // app is shutting down
            };

            // Sleep until the interval elapses or a manual refresh arrives.
            let mut slept = 0u64;
            let mut interval = interval.max(1);
            while slept < interval {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                slept += 1;
                match cx.read_global::<AppState, (u64, bool)>(|s, _| {
                    (s.config.poll_secs, s.pending_refresh)
                }) {
                    Ok((new_interval, pending)) => {
                        interval = new_interval.max(1);
                        if pending {
                            break;
                        }
                    }
                    Err(_) => return,
                }
            }

            // While paused, only a manual refresh reaches the router.
            let (config, session, skip) = match cx.read_global::<AppState, (Config, Option<String>, bool)>(
                |state, _| (state.config.clone(), state.session.clone(), state.paused && !state.pending_refresh),
            ) {
                Ok(v) => v,
                Err(_) => return,
            };
            if skip {
                continue;
            }

            // Blocking network I/O runs off the main thread.
            let (result, new_session) = cx
                .background_executor()
                .spawn(async move { fetch_once(&config, session) })
                .await;

            let updated = cx.update(|cx| {
                let first_success = {
                    let state = cx.global_mut::<AppState>();
                    state.pending_refresh = false;
                    state.session = new_session;
                    match result {
                        Ok(status) => {
                            state.status = Some(status);
                            state.error = None;
                            state.updated = Some(Instant::now());
                        }
                        Err(e) => {
                            if e.contains("login failed") {
                                state.session = None;
                            }
                            state.error = Some(e);
                        }
                    }
                    let first = state.error.is_none() && !state.router_requested;
                    state.router_requested |= first;
                    first
                };
                if std::env::var_os("X28_TRAY_LOG").is_some() {
                    let state = cx.global::<AppState>();
                    match (&state.status, &state.error) {
                        (_, Some(e)) => eprintln!("x28-tray: error: {e}"),
                        (Some(s), None) => eprintln!(
                            "x28-tray: {} · {} · {}",
                            s.temperature_label(),
                            s.summary(),
                            s.rf_line()
                        ),
                        (None, None) => {}
                    }
                }
                // The router is reachable now: load its settings for the menus.
                if first_success {
                    read_router_settings(cx);
                }
                redraw(cx);
            });
            if updated.is_err() {
                return;
            }
        }
    })
    .detach();
}

/// Push `AppState` into the tray (icon, title, tooltip, menu) and the panel.
fn redraw(cx: &mut App) {
    if cx.has_global::<AppTray>() {
        update_tray(cx);
    }
    // Notify by id rather than `update`: this also runs from inside window
    // event handlers, where that window's entity is already borrowed.
    if let Some((_, panel)) = cx.global::<PanelRef>().0.clone() {
        cx.notify(panel.entity_id());
    }
    if let Some((_, popover)) = cx.global::<PopoverRef>().open.clone() {
        cx.notify(popover.entity_id());
    }
}

fn update_tray(cx: &mut App) {
    let state = cx.global::<AppState>();
    let status = state.status.clone();
    let error = state.error.clone();
    let menu_title = state.config.menu_title;
    let paused = state.paused;
    let tray = cx.global::<AppTray>().tray.clone();

    let glyph = Glyph::for_state(status.as_ref(), error.is_some()).paused(paused);
    if glyph != cx.global::<AppTray>().glyph {
        match tray.set_icon(glyph.icon(cx), cx) {
            Ok(()) => cx.global_mut::<AppTray>().glyph = glyph,
            Err(e) => eprintln!("tray icon: {e}"),
        }
    }

    let title = if menu_title {
        status.as_ref().map(|s| format!(" {}", s.temperature_label()))
    } else {
        None
    };
    if let Err(e) = tray.set_title(title, cx) {
        eprintln!("tray title: {e}");
    }

    let tooltip = match (&status, &error) {
        (_, Some(err)) => format!("X28 PRO — {err}"),
        // Set once per poll, so no "updated Ns ago" (it would always read 0).
        (Some(s), None) => match s.signal_bars() {
            Some(bars) => format!(
                "X28 PRO — {} {} · {} signal · {}",
                s.network_label(),
                s.band_label(),
                x28_pro::signal_label(bars),
                s.temperature_label()
            ),
            None => format!("X28 PRO — {} · {}", s.band_label(), s.temperature_label()),
        },
        (None, None) => "X28 PRO — waiting for first reading".to_string(),
    };
    let tooltip = if paused { format!("{tooltip} · paused") } else { tooltip };
    if let Err(e) = tray.set_tooltip(Some(tooltip), cx) {
        eprintln!("tray tooltip: {e}");
    }
}

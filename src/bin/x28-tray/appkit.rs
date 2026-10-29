//! AppKit glue for what gpui and gpui-tray don't expose: where to open the
//! popover (centered under the status item, just below the menu bar of its
//! screen), clicks outside it (to close it like a system menu), and whether
//! the app shows in the Dock, Stage Manager and Cmd-Tab.

use std::ptr::NonNull;

use block2::RcBlock;
use gpui::{point, px, size, App, Bounds, DisplayId, Pixels};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEvent, NSEventMask, NSScreen};
use objc2_foundation::NSRect;

/// Show the app in the Dock, Stage Manager and Cmd-Tab (a regular app, while
/// its window is open) or only in the menu bar (an accessory app). gpui makes
/// every app regular at launch, so the tray switches this itself.
pub fn set_in_dock(in_dock: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let policy = if in_dock {
        NSApplicationActivationPolicy::Regular
    } else {
        NSApplicationActivationPolicy::Accessory
    };
    NSApplication::sharedApplication(mtm).setActivationPolicy(policy);
}

/// A registered global click monitor; removed when dropped.
pub struct ClickMonitor(Retained<AnyObject>);

impl Drop for ClickMonitor {
    fn drop(&mut self) {
        // SAFETY: the token came from addGlobalMonitorForEventsMatchingMask
        // and is removed exactly once.
        unsafe { NSEvent::removeMonitor(&self.0) };
    }
}

/// Call `on_click` for every mouse-down outside this app, the menu bar
/// included (AppKit doesn't report this app's own clicks to global monitors).
pub fn watch_outside_clicks(on_click: impl Fn() + 'static) -> Option<ClickMonitor> {
    let block = RcBlock::new(move |_: NonNull<NSEvent>| on_click());
    let mask = NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown;
    NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &block).map(ClickMonitor)
}

/// Gap between the menu bar and the popover, as in system menu bar extras.
const GAP: f64 = 6.0;
/// Distance kept from the screen edges.
const MARGIN: f64 = 8.0;

/// Bounds for a popover of up to `width` x `height`, relative to the returned
/// display; the height shrinks to fit short screens.
pub fn popover_bounds(width: f32, height: f32, cx: &App) -> Option<(DisplayId, Bounds<Pixels>)> {
    let mtm = MainThreadMarker::new()?;
    // Prefer the status item's own window (AppKit gives each status item an
    // NSStatusBarWindow); fall back to the pointer, which is on the item
    // after a mouse click.
    let item = NSApplication::sharedApplication(mtm)
        .windows()
        .iter()
        .find(|w| w.class().name().to_bytes().ends_with(b"StatusBarWindow"))
        .map(|w| w.frame());
    let anchor_x = match item {
        Some(f) => f.origin.x + f.size.width / 2.0,
        None => NSEvent::mouseLocation().x,
    };
    let anchor_y = match item {
        Some(f) => f.origin.y + f.size.height / 2.0,
        None => NSEvent::mouseLocation().y,
    };

    // AppKit's global space has its origin at the bottom-left of the primary
    // screen with y up; gpui's has it at the top-left with y down.
    let screens = NSScreen::screens(mtm);
    let primary_height = screens.firstObject()?.frame().size.height;
    let contains = |f: NSRect| {
        (f.origin.x..f.origin.x + f.size.width).contains(&anchor_x)
            && (f.origin.y..=f.origin.y + f.size.height).contains(&anchor_y)
    };
    let screen = screens.iter().find(|s| contains(s.frame()))?;
    let frame = screen.frame();
    let visible = screen.visibleFrame();
    let menu_bar = (frame.origin.y + frame.size.height) - (visible.origin.y + visible.size.height);

    // Match the screen to gpui's display by its top-left corner.
    let top = primary_height - frame.origin.y - frame.size.height;
    let display = cx.displays().into_iter().find(|d| {
        let o = d.bounds().origin;
        (f64::from(o.x) - frame.origin.x).abs() < 1.0 && (f64::from(o.y) - top).abs() < 1.0
    })?;

    let (width, height) = (width as f64, height as f64);
    let x = (anchor_x - frame.origin.x - width / 2.0)
        .clamp(MARGIN, (frame.size.width - width - MARGIN).max(MARGIN));
    let height = height.min(visible.size.height - GAP - MARGIN);
    Some((
        display.id(),
        Bounds {
            origin: point(px(x as f32), px((menu_bar + GAP) as f32)),
            size: size(px(width as f32), px(height as f32)),
        },
    ))
}

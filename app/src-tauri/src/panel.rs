//! The tray panel (Windows): instead of a movable window, Ferry opens as a fixed, borderless
//! panel next to its tray icon and hides when it loses focus, like a tray flyout.
//!
//! Losing focus would also hide it while the user picks files in a native dialog, or clicks into
//! Explorer to drag files in. So the UI holds it open around its dialogs (`panel_hold`), and the
//! user can pin it (`set_panel_pinned`).

use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use tauri::{
    AppHandle, LogicalSize, Manager, PhysicalPosition, PhysicalSize, Rect, State, WebviewWindow,
    Window,
    tray::{MouseButton, MouseButtonState, TrayIconEvent},
};

use crate::{AppState, PanelSize, Settings, panel_enabled};

/// Gap between the panel and the edges of the work area, in logical pixels.
const MARGIN: f64 = 12.0;
const REGULAR_SIZE: (f64, f64) = (1040.0, 720.0);
const REGULAR_MIN: (f64, f64) = (760.0, 540.0);
const FIT_WIDTH: f64 = 440.0;
const FIT_MAX_HEIGHT: f64 = 960.0;
/// A panel never gets narrower or shorter than these, unless the screen itself is.
const MIN_WIDTH: f64 = 320.0;
const MIN_HEIGHT: f64 = 320.0;
/// A press on the tray icon takes focus from the panel, hiding it, and arrives as a tray event
/// right after. A hide waits this long for its press to arrive; only a backstop for a press
/// that never reports, as the toggle itself follows the press, not the time.
const PRESS_MATCH: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct Panel {
    pinned: AtomicBool,
    /// Native dialogs the UI has open; the panel stays while any is up.
    holds: Mutex<usize>,
    /// When losing focus hid the panel with the pointer on the tray icon: a press on the icon
    /// took the focus, and that press's tray event is still to come.
    hidden_by_tray_press: Mutex<Option<Instant>>,
    /// The left press on the tray icon under way: whether it found the panel open.
    press_found_open: Mutex<Option<bool>>,
}

impl Panel {
    /// Whether the panel stays when it loses focus.
    fn stays_open(&self) -> bool {
        self.pinned.load(Ordering::SeqCst) || self.dialog_open()
    }

    fn dialog_open(&self) -> bool {
        *self.holds.lock().unwrap() > 0
    }

    fn hold(&self, held: bool) {
        let mut holds = self.holds.lock().unwrap();
        *holds = if held {
            *holds + 1
        } else {
            holds.saturating_sub(1)
        };
    }

    /// Losing focus hid the panel at `at` while the pointer was on the tray icon.
    fn hidden_by_tray_press(&self, at: Instant) {
        *self.hidden_by_tray_press.lock().unwrap() = Some(at);
    }

    /// A left press on the tray icon began at `now`, with the panel showing or not. It found the
    /// panel open if it was showing, or if this press is what just hid it.
    fn left_pressed(&self, visible: bool, now: Instant) {
        let hid_by_this_press = self
            .hidden_by_tray_press
            .lock()
            .unwrap()
            .take()
            .is_some_and(|at| now.saturating_duration_since(at) < PRESS_MATCH);
        *self.press_found_open.lock().unwrap() = Some(visible || hid_by_this_press);
    }

    /// The left button came up: whether its press found the panel open, so the click closes
    /// it rather than opening it, however long the button was held. A release without a press
    /// (none reported) goes by whether the panel shows.
    fn left_released(&self, visible: bool) -> bool {
        // A hide by this press, if it came after the press's own event, is answered too.
        self.hidden_by_tray_press.lock().unwrap().take();
        self.press_found_open
            .lock()
            .unwrap()
            .take()
            .unwrap_or(visible)
    }

    /// Another button on the tray icon (e.g. for its menu): that press, not a left click, is
    /// what hid the panel.
    fn other_button(&self) {
        self.hidden_by_tray_press.lock().unwrap().take();
    }

    /// A reloaded page starts unpinned, with no dialogs open, whatever the old one had.
    fn reset(&self) {
        self.pinned.store(false, Ordering::SeqCst);
        *self.holds.lock().unwrap() = 0;
    }
}

/// The panel size in use, or None when Ferry is a regular window.
pub fn current(app: &AppHandle) -> Option<PanelSize> {
    let state = app.try_state::<AppState>()?;
    let settings = state.settings.lock().unwrap();
    panel_enabled(&settings).then_some(settings.panel_size)
}

/// Turns the main window into the panel or back into a regular window. At startup a regular
/// window is left as configured; `switching` restores its size and frame after a panel.
pub fn apply_style(window: &WebviewWindow, settings: &Settings, switching: bool) {
    let panel = panel_enabled(settings);
    if !panel && !switching {
        return;
    }
    if panel {
        window.unmaximize().ok();
    }
    window.set_decorations(!panel).ok();
    // Undecorated, the shadow also gives the panel Windows 11's rounded corners and border.
    window.set_shadow(true).ok();
    window.set_resizable(!panel).ok();
    window.set_maximizable(!panel).ok();
    window.set_minimizable(!panel).ok();
    window.set_skip_taskbar(panel).ok();
    window.set_always_on_top(panel).ok();
    if panel {
        window.set_min_size(None::<LogicalSize<f64>>).ok();
        place(window, settings.panel_size, None);
    } else {
        window
            .set_min_size(Some(LogicalSize::new(REGULAR_MIN.0, REGULAR_MIN.1)))
            .ok();
        window
            .set_size(LogicalSize::new(REGULAR_SIZE.0, REGULAR_SIZE.1))
            .ok();
        window.center().ok();
    }
}

/// Sizes the panel and puts it in the corner of the screen by the tray icon. `icon` is where
/// the tray icon is, when a click told us; otherwise it's looked up.
pub fn place(window: &WebviewWindow, size: PanelSize, icon: Option<Rect>) {
    let icon = icon.or_else(|| {
        window
            .app_handle()
            .tray_by_id("main")
            .and_then(|tray| tray.rect().ok().flatten())
    });
    let center = icon.map(|rect| {
        let pos = rect.position.to_physical::<f64>(1.0);
        let size = rect.size.to_physical::<f64>(1.0);
        (pos.x + size.width / 2.0, pos.y + size.height / 2.0)
    });
    let monitor = center
        .and_then(|(x, y)| window.monitor_from_point(x, y).ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten())
        .or_else(|| window.current_monitor().ok().flatten());
    let Some(monitor) = monitor else { return };
    let scale = monitor.scale_factor();
    let work = monitor.work_area();
    let work = Area {
        x: work.position.x as f64,
        y: work.position.y as f64,
        w: work.size.width as f64,
        h: work.size.height as f64,
    };
    let screen = Area {
        x: monitor.position().x as f64,
        y: monitor.position().y as f64,
        w: monitor.size().width as f64,
        h: monitor.size().height as f64,
    };
    let (x, y, w, h) = bounds(&screen, &work, center, size, scale);
    let at = PhysicalPosition::new(x.round() as i32, y.round() as i32);
    let target = PhysicalSize::new(w as u32, h as u32);
    // Moving first puts the window on the target monitor (and its scale), so the size set next
    // is final; moving again pins the corner in case the size changed it. A window that
    // straddled a monitor of another scale on the way is rescaled by Windows as it lands, so
    // that one gets its size again, now on the right monitor.
    for _ in 0..2 {
        window.set_position(at).ok();
        window.set_size(target).ok();
        window.set_position(at).ok();
        let rescaled = matches!(window.inner_size(), Ok(now) if now != target);
        if !rescaled {
            break;
        }
    }
}

/// Mouse events on the tray icon. A left click closes the panel if its press found it open
/// (pressing the icon may already have hidden it, by taking its focus), and otherwise opens it
/// by the icon.
pub fn on_tray_event(app: &AppHandle, event: &TrayIconEvent) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let panel = app.state::<Panel>();
    let visible = window.is_visible().unwrap_or(false);
    match *event {
        // Windows reports a quick second press as a double click instead of a press.
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Down,
            ..
        }
        | TrayIconEvent::DoubleClick {
            button: MouseButton::Left,
            ..
        } => panel.left_pressed(visible, Instant::now()),
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            rect,
            ..
        } => {
            if !panel.left_released(visible) {
                crate::show_main_at(app, Some(rect));
            } else if visible && !panel.dialog_open() {
                // With a dialog open it stays: hiding it would leave the dialog on its own.
                window.hide().ok();
            }
        }
        TrayIconEvent::Click { .. } | TrayIconEvent::DoubleClick { .. } => panel.other_button(),
        _ => {}
    }
}

/// Whether the pointer is on the tray icon, i.e. a press there is what took the focus.
fn pointer_on_tray_icon(window: &Window) -> bool {
    let Some(rect) = window
        .app_handle()
        .tray_by_id("main")
        .and_then(|tray| tray.rect().ok().flatten())
    else {
        return false;
    };
    let Ok(pointer) = window.cursor_position() else {
        return false;
    };
    let pos = rect.position.to_physical::<f64>(1.0);
    let size = rect.size.to_physical::<f64>(1.0);
    let icon = Area {
        x: pos.x,
        y: pos.y,
        w: size.width,
        h: size.height,
    };
    icon.contains(pointer.x, pointer.y)
}

/// The panel lost focus: it hides, unless it's pinned or a dialog of its own took the focus.
pub fn on_blur(window: &Window) {
    if window.label() != "main" || current(window.app_handle()).is_none() {
        return;
    }
    let panel = window.state::<Panel>();
    if panel.stays_open() {
        return;
    }
    if window.is_visible().unwrap_or(false) {
        if pointer_on_tray_icon(window) {
            panel.hidden_by_tray_press(Instant::now());
        }
        window.hide().ok();
    }
}

/// The page (re)loaded: it starts unpinned with no dialogs open, so the panel does too.
pub fn reset(app: &AppHandle) {
    if let Some(panel) = app.try_state::<Panel>() {
        panel.reset();
    }
}

#[tauri::command]
pub fn set_panel_pinned(panel: State<'_, Panel>, pinned: bool) {
    panel.pinned.store(pinned, Ordering::SeqCst);
}

/// The UI opens (`held`) or closes a native dialog, during which the panel stays.
#[tauri::command]
pub fn panel_hold(panel: State<'_, Panel>, held: bool) {
    panel.hold(held);
}

/// Escape in the UI: puts the panel away. A regular window stays.
#[tauri::command]
pub fn hide_panel(window: Window) {
    if current(window.app_handle()).is_some() {
        window.hide().ok();
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Area {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Area {
    /// Whether the point is inside, edges included (a pointer on the icon's last pixel is on it).
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

/// The panel's size in logical pixels, for a work area `work_width` × `work_height` logical
/// pixels. It never runs past the work area: the panel can't be moved or resized, so whatever
/// fell off the screen would be out of reach.
fn logical_size(size: PanelSize, work_width: f64, work_height: f64) -> (f64, f64) {
    let room = |work: f64, min: f64| (work - 2.0 * MARGIN).max(min.min(work));
    let (w, h) = match size {
        PanelSize::Compact => (420.0, 680.0),
        PanelSize::Roomy => (480.0, 720.0),
        PanelSize::Fit => (FIT_WIDTH, FIT_MAX_HEIGHT),
    };
    (
        w.min(room(work_width, MIN_WIDTH)),
        h.min(room(work_height, MIN_HEIGHT)),
    )
}

/// The panel's bounds (x, y, width, height) in physical pixels, on a monitor with this scale
/// factor, its work area `work` within `screen`, and the tray icon at `icon`.
fn bounds(
    screen: &Area,
    work: &Area,
    icon: Option<(f64, f64)>,
    size: PanelSize,
    scale: f64,
) -> (f64, f64, f64, f64) {
    let (w, h) = logical_size(size, work.w / scale, work.h / scale);
    // Rounded down, so scaling up can't add a pixel past the work area.
    let (w, h) = ((w * scale).floor(), (h * scale).floor());
    let (x, y) = corner(screen, work, icon, w, h, MARGIN * scale);
    (x, y, w, h)
}

/// Where the panel's top-left corner goes: in the corner of the work area on the taskbar's
/// side, nearest the tray icon `icon`, `margin` from the edges. All in physical pixels.
fn corner(
    screen: &Area,
    work: &Area,
    icon: Option<(f64, f64)>,
    w: f64,
    h: f64,
    margin: f64,
) -> (f64, f64) {
    // The taskbar is on the side where the work area stops short of the screen.
    let gap_left = work.x - screen.x;
    let gap_top = work.y - screen.y;
    let gap_right = (screen.x + screen.w) - (work.x + work.w);
    let gap_bottom = (screen.y + screen.h) - (work.y + work.h);
    let (cx, cy) = icon.unwrap_or((work.x + work.w, work.y + work.h));
    let right = if gap_left > 0.0 && gap_left > gap_right {
        false
    } else if gap_right > 0.0 {
        true
    } else {
        cx >= work.x + work.w / 2.0
    };
    let bottom = if gap_top > 0.0 && gap_top > gap_bottom {
        false
    } else if gap_bottom > 0.0 {
        true
    } else {
        cy >= work.y + work.h / 2.0
    };
    let x = if right {
        work.x + work.w - w - margin
    } else {
        work.x + margin
    };
    let y = if bottom {
        work.y + work.h - h - margin
    } else {
        work.y + margin
    };
    (
        x.clamp(work.x, (work.x + work.w - w).max(work.x)),
        y.clamp(work.y, (work.y + work.h - h).max(work.y)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Area = Area {
        x: 0.0,
        y: 0.0,
        w: 1920.0,
        h: 1080.0,
    };

    #[test]
    fn sits_above_a_bottom_taskbar_on_the_right() {
        let work = Area {
            h: 1032.0,
            ..SCREEN
        };
        let at = corner(&SCREEN, &work, Some((1800.0, 1056.0)), 420.0, 680.0, 12.0);
        assert_eq!(at, (1920.0 - 420.0 - 12.0, 1032.0 - 680.0 - 12.0));
    }

    #[test]
    fn follows_the_taskbar_to_other_edges() {
        let left = Area {
            x: 60.0,
            w: 1860.0,
            ..SCREEN
        };
        assert_eq!(
            corner(&SCREEN, &left, Some((30.0, 1000.0)), 400.0, 600.0, 10.0),
            (70.0, 1080.0 - 600.0 - 10.0)
        );
        let top = Area {
            y: 48.0,
            h: 1032.0,
            ..SCREEN
        };
        assert_eq!(
            corner(&SCREEN, &top, Some((1800.0, 24.0)), 400.0, 600.0, 10.0),
            (1920.0 - 400.0 - 10.0, 58.0)
        );
    }

    #[test]
    fn a_hidden_taskbar_goes_by_the_icon_or_bottom_right() {
        assert_eq!(
            corner(&SCREEN, &SCREEN, Some((100.0, 50.0)), 400.0, 600.0, 10.0),
            (10.0, 10.0)
        );
        assert_eq!(
            corner(&SCREEN, &SCREEN, None, 400.0, 600.0, 10.0),
            (1510.0, 470.0)
        );
    }

    #[test]
    fn works_on_a_second_monitor() {
        let screen = Area {
            x: -1280.0,
            y: 0.0,
            w: 1280.0,
            h: 1024.0,
        };
        let work = Area { h: 984.0, ..screen };
        assert_eq!(
            corner(&screen, &work, Some((-50.0, 1000.0)), 400.0, 600.0, 10.0),
            (-410.0, 374.0)
        );
    }

    #[test]
    fn stays_inside_a_small_work_area() {
        let small = Area {
            x: 0.0,
            y: 0.0,
            w: 300.0,
            h: 400.0,
        };
        assert_eq!(corner(&small, &small, None, 420.0, 680.0, 12.0), (0.0, 0.0));
    }

    #[test]
    fn stays_open_while_pinned_or_a_dialog_is_up() {
        let panel = Panel::default();
        assert!(!panel.stays_open());
        panel.pinned.store(true, Ordering::SeqCst);
        assert!(panel.stays_open());
        panel.pinned.store(false, Ordering::SeqCst);
        // Two dialogs at once (e.g. a question over a picker): open until both are gone.
        panel.hold(true);
        panel.hold(true);
        panel.hold(false);
        assert!(panel.stays_open() && panel.dialog_open());
        panel.hold(false);
        assert!(!panel.stays_open());
        // A stray release can't leave a hold that never ends.
        panel.hold(false);
        panel.hold(true);
        panel.hold(false);
        assert!(!panel.dialog_open());
    }

    #[test]
    fn a_reloaded_page_unpins_and_drops_holds() {
        let panel = Panel::default();
        panel.pinned.store(true, Ordering::SeqCst);
        panel.hold(true);
        panel.reset();
        assert!(!panel.stays_open());
    }

    /// A left click on the tray icon: what it does to a panel that is `visible` when the button
    /// goes down, where the press itself hides it (by taking its focus) if `press_hides`.
    /// Returns whether the click closes the panel. When the button comes up plays no part.
    fn click(panel: &Panel, visible: bool, press_hides: bool) -> bool {
        let start = Instant::now();
        if press_hides {
            panel.hidden_by_tray_press(start);
        }
        panel.left_pressed(visible, start);
        panel.left_released(visible && !press_hides)
    }

    #[test]
    fn a_tray_click_closes_an_open_panel_however_long_it_is_held() {
        let panel = Panel::default();
        // The press takes the panel's focus and hides it before the click arrives; the release
        // (however late) closes rather than reopens.
        assert!(click(&panel, false, true));
        // Pinned, the press doesn't hide it; the click still closes it.
        assert!(click(&panel, true, false));
        // The press's own event can come before the focus loss it caused.
        panel.left_pressed(true, Instant::now());
        panel.hidden_by_tray_press(Instant::now());
        assert!(panel.left_released(false));
        // ...and that hide doesn't make the next click think the panel was open.
        assert!(!click(&panel, false, false));
    }

    #[test]
    fn a_tray_click_opens_a_closed_panel_however_soon_it_comes() {
        let panel = Panel::default();
        // Clicking elsewhere hid it (the pointer wasn't on the icon): a click right after opens.
        assert!(!click(&panel, false, false));
        // A right click (the menu) that hid it doesn't count as a left press later.
        panel.hidden_by_tray_press(Instant::now());
        panel.other_button();
        assert!(!click(&panel, false, false));
        // A hide whose press never reported doesn't swallow a later click.
        let long_ago = Instant::now();
        panel.hidden_by_tray_press(long_ago);
        panel.left_pressed(false, long_ago + PRESS_MATCH);
        assert!(!panel.left_released(false));
        // A release without a press goes by whether the panel shows.
        assert!(panel.left_released(true));
        assert!(!panel.left_released(false));
    }

    #[test]
    fn sizes_fit_the_screen() {
        assert_eq!(
            logical_size(PanelSize::Compact, 1920.0, 1032.0),
            (420.0, 680.0)
        );
        assert_eq!(
            logical_size(PanelSize::Roomy, 1920.0, 1032.0),
            (480.0, 720.0)
        );
        assert_eq!(logical_size(PanelSize::Fit, 1920.0, 1032.0), (440.0, 960.0));
        assert_eq!(logical_size(PanelSize::Fit, 1920.0, 800.0), (440.0, 776.0));
        // A small screen shrinks every size; a tiny one still gets a usable panel.
        assert_eq!(
            logical_size(PanelSize::Roomy, 1920.0, 600.0),
            (480.0, 576.0)
        );
        assert_eq!(
            logical_size(PanelSize::Roomy, 400.0, 1032.0),
            (376.0, 720.0)
        );
        assert_eq!(
            logical_size(PanelSize::Compact, 300.0, 300.0),
            (300.0, 300.0)
        );
    }

    #[test]
    fn the_panel_stays_inside_the_work_area() {
        let check = |screen: Area, work: Area, icon: Option<(f64, f64)>, scale: f64| {
            for size in [PanelSize::Compact, PanelSize::Roomy, PanelSize::Fit] {
                let (x, y, w, h) = bounds(&screen, &work, icon, size, scale);
                assert!(
                    x >= work.x && y >= work.y,
                    "{size:?} at {scale}x starts outside"
                );
                assert!(
                    x + w <= work.x + work.w && y + h <= work.y + work.h,
                    "{size:?} at {scale}x runs past the work area: {x},{y} {w}x{h} in {work:?}"
                );
            }
        };
        // A 768-pixel-wide display at 200%: Compact alone would be 840 pixels wide.
        let narrow = Area {
            x: 0.0,
            y: 0.0,
            w: 768.0,
            h: 1024.0,
        };
        check(
            narrow,
            Area { h: 944.0, ..narrow },
            Some((740.0, 990.0)),
            2.0,
        );
        // A portrait tablet at 250%, taskbar at the bottom, and odd scales that don't divide.
        let tablet = Area {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 1280.0,
        };
        check(
            tablet,
            Area {
                h: 1160.0,
                ..tablet
            },
            None,
            2.5,
        );
        check(
            SCREEN,
            Area {
                h: 1032.0,
                ..SCREEN
            },
            Some((1800.0, 1056.0)),
            1.25,
        );
        check(
            SCREEN,
            Area {
                h: 1032.0,
                ..SCREEN
            },
            Some((1800.0, 1056.0)),
            1.75,
        );
        // A second monitor left of the primary, taskbar on its left edge.
        let second = Area {
            x: -1280.0,
            y: 0.0,
            w: 1280.0,
            h: 720.0,
        };
        check(
            second,
            Area {
                x: -1232.0,
                w: 1232.0,
                ..second
            },
            Some((-1256.0, 700.0)),
            1.5,
        );
        // At 100% on a common screen, nothing changes: Compact is 420 × 680.
        let (_, _, w, h) = bounds(
            &SCREEN,
            &Area {
                h: 1032.0,
                ..SCREEN
            },
            None,
            PanelSize::Compact,
            1.0,
        );
        assert_eq!((w, h), (420.0, 680.0));
    }

    #[test]
    fn the_pointer_is_on_the_icon_up_to_its_edges() {
        let icon = Area {
            x: 1800.0,
            y: 1040.0,
            w: 32.0,
            h: 40.0,
        };
        assert!(icon.contains(1816.0, 1060.0));
        assert!(icon.contains(1800.0, 1040.0) && icon.contains(1832.0, 1080.0));
        assert!(!icon.contains(1799.0, 1060.0) && !icon.contains(1816.0, 1081.0));
    }
}

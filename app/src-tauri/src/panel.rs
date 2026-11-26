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
};

use crate::{AppState, PanelSize, Settings, panel_enabled};

/// Gap between the panel and the edges of the work area, in logical pixels.
const MARGIN: f64 = 12.0;
const REGULAR_SIZE: (f64, f64) = (1040.0, 720.0);
const REGULAR_MIN: (f64, f64) = (760.0, 540.0);
const FIT_WIDTH: f64 = 440.0;
const FIT_MAX_HEIGHT: f64 = 960.0;
/// A panel never gets shorter than this, unless the screen itself is.
const MIN_HEIGHT: f64 = 320.0;
/// Clicking the tray icon takes focus from the panel (hiding it) just before the click arrives.
/// A click this soon after that hide closes the panel rather than reopening it.
const REOPEN_GUARD: Duration = Duration::from_millis(400);

#[derive(Default)]
pub struct Panel {
    pinned: AtomicBool,
    /// Native dialogs the UI has open; the panel stays while any is up.
    holds: Mutex<usize>,
    auto_hidden_at: Mutex<Option<Instant>>,
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
    let (w, h) = logical_size(size, work.h / scale);
    let (w, h) = ((w * scale).round(), (h * scale).round());
    let (x, y) = corner(&screen, &work, center, w, h, MARGIN * scale);
    let at = PhysicalPosition::new(x.round() as i32, y.round() as i32);
    // Moving first puts the window on the target monitor (and its scale), so the size set next
    // is final; moving again pins the corner in case the size changed it.
    window.set_position(at).ok();
    window.set_size(PhysicalSize::new(w as u32, h as u32)).ok();
    window.set_position(at).ok();
}

/// A left click on the tray icon: closes an open panel, otherwise opens it by the icon.
pub fn toggle(app: &AppHandle, icon: Rect) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let recently_hidden = app
        .state::<Panel>()
        .auto_hidden_at
        .lock()
        .unwrap()
        .take()
        .is_some_and(|at| at.elapsed() < REOPEN_GUARD);
    if window.is_visible().unwrap_or(false) {
        window.hide().ok();
    } else if !recently_hidden {
        crate::show_main_at(app, Some(icon));
    }
}

/// The panel lost focus: it hides, unless it's pinned or a dialog of its own took the focus.
pub fn on_blur(window: &Window) {
    if window.label() != "main" || current(window.app_handle()).is_none() {
        return;
    }
    let panel = window.state::<Panel>();
    if panel.pinned.load(Ordering::SeqCst) || *panel.holds.lock().unwrap() > 0 {
        return;
    }
    if window.is_visible().unwrap_or(false) {
        *panel.auto_hidden_at.lock().unwrap() = Some(Instant::now());
        window.hide().ok();
    }
}

/// A reloaded page has no dialogs open, whatever it held before.
pub fn reset_holds(app: &AppHandle) {
    if let Some(panel) = app.try_state::<Panel>() {
        *panel.holds.lock().unwrap() = 0;
    }
}

#[tauri::command]
pub fn set_panel_pinned(panel: State<'_, Panel>, pinned: bool) {
    panel.pinned.store(pinned, Ordering::SeqCst);
}

/// The UI opens (`held`) or closes a native dialog, during which the panel stays.
#[tauri::command]
pub fn panel_hold(panel: State<'_, Panel>, held: bool) {
    let mut holds = panel.holds.lock().unwrap();
    *holds = if held {
        *holds + 1
    } else {
        holds.saturating_sub(1)
    };
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

/// The panel's size in logical pixels, for a work area `work_height` logical pixels tall.
fn logical_size(size: PanelSize, work_height: f64) -> (f64, f64) {
    let room = (work_height - 2.0 * MARGIN).max(MIN_HEIGHT.min(work_height));
    let (w, h) = match size {
        PanelSize::Compact => (420.0, 680.0),
        PanelSize::Roomy => (480.0, 720.0),
        PanelSize::Fit => (FIT_WIDTH, FIT_MAX_HEIGHT),
    };
    (w, h.min(room))
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
    fn sizes_fit_the_screen() {
        assert_eq!(logical_size(PanelSize::Compact, 1032.0), (420.0, 680.0));
        assert_eq!(logical_size(PanelSize::Roomy, 1032.0), (480.0, 720.0));
        assert_eq!(logical_size(PanelSize::Fit, 1032.0), (440.0, 960.0));
        assert_eq!(logical_size(PanelSize::Fit, 800.0), (440.0, 776.0));
        // A short screen shrinks every size; a tiny one still gets a usable panel.
        assert_eq!(logical_size(PanelSize::Roomy, 600.0), (480.0, 576.0));
        assert_eq!(logical_size(PanelSize::Compact, 300.0), (420.0, 300.0));
    }
}

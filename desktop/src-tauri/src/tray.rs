//! Menu bar (macOS) or notification area (Windows) icon: a left click opens the usage popover (subscription limits and
//! key budgets), a right click the menu to switch a project's profile without opening the window. Closing the window
//! keeps KeyZapper running here.

use crate::model::TrayInfo;
use crate::{changed, refresh, refresh_usage, show_window, with_model};
use keyzapper_core::l;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, Rect, WebviewWindow, Window, WindowEvent};

const TRAY_ID: &str = "main";
const OPEN: &str = "open";
const QUIT: &str = "quit";
const BIND: &str = "bind";

/// The content shown last (main thread only); the menu is only rebuilt when it changes, so it does not flicker while open.
static SHOWN: Mutex<Option<TrayInfo>> = Mutex::new(None);

/// Label of the usage popover window (tauri.conf.json).
pub const POPOVER: &str = "usage";
const POPOVER_WIDTH: f64 = 340.0;
/// Values older than this are reloaded when the popover opens.
const USAGE_MAX_AGE: Duration = Duration::from_secs(120);
/// Tray icon the popover belongs to, in physical pixels: x, y, width, height.
static ANCHOR: Mutex<Option<(f64, f64, f64, f64)>> = Mutex::new(None);
/// Logical height the popover's content asked for.
static HEIGHT: Mutex<f64> = Mutex::new(320.0);
/// When losing focus last hid the popover: the click on the tray icon that caused it must not open it again.
static HIDDEN_AT: Mutex<Option<Instant>> = Mutex::new(None);

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    // The bolt of the app logo. macOS: a black template image that the menu bar tints; Windows: the colored logo tile.
    #[cfg(target_os = "macos")]
    let icon = tauri::include_image!("icons/tray.png");
    #[cfg(not(target_os = "macos"))]
    let icon = tauri::include_image!("icons/tray-windows.png");
    let builder = TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .show_menu_on_left_click(false);
    // No tooltip on macOS: tray-icon covers the status-bar button with its own view that takes the mouse events, so
    // AppKit never sees the pointer leave the button and the tooltip stays on screen.
    #[cfg(not(target_os = "macos"))]
    let builder = builder.tooltip("KeyZapper");
    builder
        .on_menu_event(|app, event| handle(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, rect, .. } = event {
                toggle_popover(tray.app_handle(), rect);
            }
        })
        .build(app)?;
    update(app);
    Ok(())
}

/// Rebuilds the menu if budgets, profiles or projects changed. Runs on the main thread, where the menu lives, so a
/// caller never waits for it.
pub fn update(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let info = with_model(&handle, |m| m.tray_info());
        let mut shown = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
        if shown.as_ref() == Some(&info) {
            return;
        }
        let Some(tray) = handle.tray_by_id(TRAY_ID) else { return };
        if let Ok(menu) = build_menu(&handle, &info) {
            let _ = tray.set_menu(Some(menu));
            #[cfg(not(target_os = "macos"))]
            let _ = tray.set_tooltip(Some(tooltip(&info)));
            *shown = Some(info);
        }
    });
}

fn money(value: f64) -> String {
    if keyzapper_core::i18n::language() == "de" {
        format!("{value:.2} $").replace('.', ",")
    } else {
        format!("${value:.2}")
    }
}

fn budget_text(name: &str, (spend, max): (f64, Option<f64>)) -> String {
    match max {
        Some(max) => l!("%@: %@ übrig von %@", name, money((max - spend).max(0.0)), money(max)),
        None => l!("%@: %@ verbraucht", name, money(spend)),
    }
}

#[cfg(not(target_os = "macos"))]
fn tooltip(info: &TrayInfo) -> String {
    let attention = info.projects.iter().filter(|p| !p.healthy).count();
    if info.disabled {
        l!("KeyZapper ist deaktiviert")
    } else if attention > 0 {
        l!("KeyZapper – %@ Projekt(e) brauchen Aufmerksamkeit", attention)
    } else {
        "KeyZapper".to_string()
    }
}

fn build_menu(app: &AppHandle, info: &TrayInfo) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    for profile in &info.profiles {
        for budget in &profile.budgets {
            let name = match &budget.tenant {
                Some(tenant) => format!("{} · {tenant}", profile.name),
                None => profile.name.clone(),
            };
            menu.append(&MenuItem::new(app, budget_text(&name, (budget.spend, budget.max)), false, None::<&str>)?)?;
        }
    }
    if info.profiles.iter().any(|p| !p.budgets.is_empty()) {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    if info.disabled {
        menu.append(&MenuItem::new(app, l!("KeyZapper ist deaktiviert"), false, None::<&str>)?)?;
    } else if !info.projects.is_empty() {
        menu.append(&MenuItem::new(app, l!("Profil je Projekt"), false, None::<&str>)?)?;
        for project in &info.projects {
            let title = if project.healthy { project.name.clone() } else { format!("⚠︎ {}", project.name) };
            let submenu = Submenu::new(app, title, true)?;
            for profile in &info.profiles {
                let label = if profile.usable { profile.name.clone() } else { l!("%@ (kein Zugang)", profile.name) };
                let id = format!("{BIND}:{}:{}", project.id, profile.id);
                submenu.append(&CheckMenuItem::with_id(app, id, label, profile.usable, profile.id == project.profile_id, None::<&str>)?)?;
            }
            menu.append(&submenu)?;
        }
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, OPEN, l!("KeyZapper öffnen"), true, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, QUIT, l!("KeyZapper beenden"), true, None::<&str>)?)?;
    Ok(menu)
}

fn handle(app: &AppHandle, id: &str) {
    match id.split(':').collect::<Vec<_>>().as_slice() {
        [OPEN] => show_window(app),
        [QUIT] => app.exit(0),
        [BIND, binding_id, profile_id] => {
            let (app, binding_id, profile_id) = (app.clone(), binding_id.to_string(), profile_id.to_string());
            // The click toggled the check mark; rebuild so the menu shows the real assignment again.
            *SHOWN.lock().unwrap_or_else(|e| e.into_inner()) = None;
            std::thread::spawn(move || {
                with_model(&app, |m| {
                    if let Some(path) = m.state.bindings.iter().find(|b| b.id == binding_id).map(|b| b.path.clone()) {
                        m.bind(&path, &profile_id);
                    }
                });
                refresh(&app);
                changed(&app);
            });
        }
        _ => {}
    }
}

pub fn exists(app: &AppHandle) -> bool {
    app.tray_by_id(TRAY_ID).is_some()
}

// MARK: Usage popover

fn popover(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(POPOVER)
}

fn toggle_popover(app: &AppHandle, rect: Rect) {
    let Some(window) = popover(app) else { return show_window(app) };
    if window.is_visible().unwrap_or(false) {
        return hide_popover(app);
    }
    if HIDDEN_AT.lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|at| at.elapsed() < Duration::from_millis(400)) {
        return;
    }
    let scale = window.scale_factor().unwrap_or(1.0);
    let (position, size) = (rect.position.to_physical::<f64>(scale), rect.size.to_physical::<f64>(scale));
    *ANCHOR.lock().unwrap_or_else(|e| e.into_inner()) = Some((position.x, position.y, size.width, size.height));
    place(&window);
    let _ = window.show();
    let _ = window.set_focus();
    // `window.emit` would reach every webview, hidden ones too (see `changed`).
    let _ = app.emit_to(POPOVER, "popover-shown", ());
    if with_model(app, |m| m.usage_refreshed_at.is_none_or(|at| at.elapsed() > USAGE_MAX_AGE)) {
        refresh_usage(app);
    }
}

pub fn hide_popover(app: &AppHandle) {
    if let Some(window) = popover(app) {
        let _ = window.hide();
    }
}

/// Fits the popover to its content (logical height) and keeps it at the tray icon.
pub fn fit_popover(app: &AppHandle, height: f64) {
    let height = height.clamp(120.0, 640.0);
    *HEIGHT.lock().unwrap_or_else(|e| e.into_inner()) = height;
    if let Some(window) = popover(app) {
        let _ = window.set_size(LogicalSize::new(POPOVER_WIDTH, height));
        place(&window);
    }
}

/// Centered below the tray icon (menu bar at the top), or above it when the icon is in the lower half of its screen
/// (Windows taskbar), kept inside that screen's work area.
fn place(window: &WebviewWindow) {
    let Some((x, y, width, height)) = *ANCHOR.lock().unwrap_or_else(|e| e.into_inner()) else { return };
    let (center_x, center_y) = (x + width / 2.0, y + height / 2.0);
    let monitor = window.available_monitors().unwrap_or_default().into_iter().find(|m| {
        let (p, s) = (m.position(), m.size());
        (p.x as f64..p.x as f64 + s.width as f64).contains(&center_x) && (p.y as f64..p.y as f64 + s.height as f64).contains(&center_y)
    });
    let scale = monitor.as_ref().map_or_else(|| window.scale_factor().unwrap_or(1.0), |m| m.scale_factor());
    let (w, h, gap) = (POPOVER_WIDTH * scale, *HEIGHT.lock().unwrap_or_else(|e| e.into_inner()) * scale, 6.0 * scale);
    let (mut left, mut top) = (center_x - w / 2.0, y + height + gap);
    if let Some(monitor) = monitor {
        let area = monitor.work_area();
        let (ax, ay) = (area.position.x as f64, area.position.y as f64);
        let (aw, ah) = (area.size.width as f64, area.size.height as f64);
        if center_y > ay + ah / 2.0 {
            top = y - h - gap;
        }
        left = left.clamp(ax + gap, (ax + aw - w - gap).max(ax + gap));
        top = top.clamp(ay, (ay + ah - h).max(ay));
    }
    let _ = window.set_position(PhysicalPosition::new(left.round() as i32, top.round() as i32));
}

/// The popover closes when it loses focus (a click elsewhere) and is hidden instead of closed.
pub fn popover_event(window: &Window, event: &WindowEvent) {
    match event {
        WindowEvent::Focused(false) if window.is_visible().unwrap_or(false) => {
            *HIDDEN_AT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
            let _ = window.hide();
        }
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = window.hide();
        }
        _ => {}
    }
}

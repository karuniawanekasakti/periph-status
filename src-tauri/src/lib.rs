pub mod alerts;
pub mod model;
mod monitor;
pub mod provider;
pub mod settings;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, PhysicalPosition, RunEvent, State, WebviewWindow, WindowEvent, Wry};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

use monitor::{lock, Monitor, Snapshot, TRAY_ID};
use settings::Settings;

/// Argument passed when launched automatically at login (the popup is not opened).
const AUTOSTART_ARG: &str = "--autostart";
const POPUP_LABEL: &str = "main";

/// Tray icon position and size in physical pixels: (x, y, w, h).
type Anchor = (f64, f64, f64, f64);

struct AppState {
    monitor: Arc<Monitor>,
    settings_path: PathBuf,
    /// When the popup was last hidden because it lost focus.
    last_hidden: Mutex<Option<Instant>>,
    autostart_item: Mutex<Option<CheckMenuItem<Wry>>>,
}

// ---- commands --------------------------------------------------------------

#[tauri::command]
fn get_snapshot(state: State<'_, AppState>) -> Snapshot {
    state.monitor.snapshot()
}

#[tauri::command]
fn refresh_now(state: State<'_, AppState>) {
    state.monitor.refresh();
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.monitor.settings()
}

#[tauri::command]
fn set_settings(state: State<'_, AppState>, settings: Settings) -> Result<Settings, String> {
    let settings = settings.sanitized();
    settings
        .save(&state.settings_path)
        .map_err(|e| format!("failed to save settings: {e:#}"))?;
    state.monitor.set_settings(settings.clone());
    state.monitor.refresh();
    Ok(settings)
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    apply_autostart(&app, enabled)?;
    if let Some(item) = lock(&app.state::<AppState>().autostart_item).as_ref() {
        let _ = item.set_checked(enabled);
    }
    Ok(enabled)
}

#[tauri::command]
fn hide_popup(window: WebviewWindow) {
    let _ = window.hide();
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

// ---- app -------------------------------------------------------------------

pub fn run() {
    tauri::Builder::default()
        // Must be the first plugin: a second instance just opens the first instance's popup.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_popup(app, None);
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![AUTOSTART_ARG]),
        ))
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            refresh_now,
            get_settings,
            set_settings,
            get_autostart,
            set_autostart,
            hide_popup,
            quit_app,
        ])
        .setup(|app| {
            let settings_path = app.path().app_config_dir()?.join("settings.json");
            let monitor = Monitor::new(Settings::load(&settings_path));
            app.manage(AppState {
                monitor: Arc::clone(&monitor),
                settings_path,
                last_hidden: Mutex::new(None),
                autostart_item: Mutex::new(None),
            });

            build_tray(app.handle())?;
            monitor.start(app.handle().clone());

            // Launched manually (not via autostart) → show the popup so the user knows the app
            // is running, since new tray icons are often hidden in the Windows 11 overflow.
            if !std::env::args().any(|a| a == AUTOSTART_ARG) {
                show_popup(app.handle(), None);
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::Focused(false) if window.label() == POPUP_LABEL => {
                let _ = window.hide();
                *lock(&window.state::<AppState>().last_hidden) = Some(Instant::now());
            }
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("failed to build the Tauri application")
        .run(|_app, event| {
            // Keep the app alive in the tray even when no window is visible.
            if let RunEvent::ExitRequested { code: None, api, .. } = event {
                api.prevent_exit();
            }
        });
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let autostart_on = app.autolaunch().is_enabled().unwrap_or(false);

    let open_i = MenuItem::with_id(app, "open", "Open", true, None::<&str>)?;
    let refresh_i = MenuItem::with_id(app, "refresh", "Refresh now", true, None::<&str>)?;
    let autostart_i = CheckMenuItem::with_id(
        app,
        "autostart",
        "Start at login",
        true,
        autostart_on,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open_i, &refresh_i, &autostart_i, &separator, &quit_i])?;
    *lock(&app.state::<AppState>().autostart_item) = Some(autostart_i.clone());

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Periph Status")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => show_popup(app, None),
            "refresh" => app.state::<AppState>().monitor.refresh(),
            "autostart" => {
                // The CheckMenuItem has already toggled itself when clicked.
                let wanted = autostart_i.is_checked().unwrap_or(false);
                if let Err(e) = apply_autostart(app, wanted) {
                    log::error!("{e}");
                    let _ = autostart_i.set_checked(!wanted);
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                let pos = rect.position.to_physical::<f64>(1.0);
                let size = rect.size.to_physical::<f64>(1.0);
                toggle_popup(tray.app_handle(), Some((pos.x, pos.y, size.width, size.height)));
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

fn apply_autostart(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let launcher = app.autolaunch();
    let res = if enabled {
        launcher.enable()
    } else {
        launcher.disable()
    };
    res.map_err(|e| format!("failed to change autostart: {e}"))
}

fn toggle_popup(app: &AppHandle, anchor: Option<Anchor>) {
    let Some(win) = app.get_webview_window(POPUP_LABEL) else {
        return;
    };
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
        return;
    }
    // Clicking the tray icon makes the popup lose focus first (and hide).
    // Don't reopen it from that same click.
    let recently_hidden = lock(&app.state::<AppState>().last_hidden)
        .is_some_and(|t| t.elapsed() < Duration::from_millis(300));
    if !recently_hidden {
        show_popup(app, anchor);
    }
}

fn show_popup(app: &AppHandle, anchor: Option<Anchor>) {
    let Some(win) = app.get_webview_window(POPUP_LABEL) else {
        return;
    };
    if let Err(e) = position_popup(&win, anchor) {
        log::warn!("failed to position the popup: {e}");
    }
    let _ = win.unminimize();
    let _ = win.show();
    let _ = win.set_focus();
}

/// Place the popup right above (or below) the taskbar, aligned with the tray icon,
/// and keep it inside the monitor's work area.
fn position_popup(win: &WebviewWindow, anchor: Option<Anchor>) -> tauri::Result<()> {
    let size = win.outer_size()?;
    let (w, h) = (f64::from(size.width), f64::from(size.height));

    let monitor = match anchor {
        Some((x, y, _, _)) => win.monitor_from_point(x, y)?,
        None => None,
    };
    let Some(monitor) = monitor.or(win.primary_monitor()?) else {
        return Ok(());
    };
    let area = monitor.work_area();
    let (ax, ay) = (f64::from(area.position.x), f64::from(area.position.y));
    let (aw, ah) = (f64::from(area.size.width), f64::from(area.size.height));
    let margin = 12.0 * monitor.scale_factor();

    let (x, y) = match anchor {
        Some((tx, ty, tw, th)) => {
            let x = tx + tw / 2.0 - w / 2.0;
            let tray_at_bottom = ty + th / 2.0 > ay + ah / 2.0;
            let y = if tray_at_bottom { ay + ah - h - margin } else { ay + margin };
            (x, y)
        }
        None => (ax + aw - w - margin, ay + ah - h - margin),
    };
    let x = x.clamp(ax + margin, (ax + aw - w - margin).max(ax + margin));
    win.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32))
}

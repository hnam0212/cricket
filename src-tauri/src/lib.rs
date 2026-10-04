//! Tauri app: wires the core, the audio backend and the UI together.

mod service;
mod store;
mod strings;

use std::sync::Mutex;

use cricket_bridge::Bridge;
use cricket_core::audio::AppId;
use cricket_core::config::AppConfig;
use cricket_core::settings::Settings;
use cricket_core::source::Source;
use service::{BridgeStatus, Service, Status, Tabs};
use store::ConfigStore;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, LogicalSize, Manager, RunEvent, State, WindowEvent, Wry};

const MAIN_WINDOW: &str = "main";
const TRAY_ID: &str = "main";
const FULL_SIZE: LogicalSize<f64> = LogicalSize::new(460.0, 680.0);
const MINI_SIZE: LogicalSize<f64> = LogicalSize::new(460.0, 150.0);

struct App {
    service: Service,
    bridge: Option<Bridge>,
    store: ConfigStore,
    /// The tray's "Enabled" item, kept so it can follow the window's switch.
    tray_enabled: Mutex<Option<CheckMenuItem<Wry>>>,
}

#[tauri::command]
fn core_info() -> String {
    cricket_core::describe()
}

#[tauri::command]
fn get_status(app: State<App>) -> Status {
    app.service.status()
}

#[tauri::command]
fn get_config(app: State<App>) -> AppConfig {
    app.store.get()
}

#[tauri::command]
fn set_source(app: State<App>, source: Option<Source>) -> AppConfig {
    // An app source without a name is no source.
    let source = source.filter(|source| match source {
        Source::App { app } => *app != AppId::new(""),
        Source::Tab { .. } => true,
    });
    // A tab id only means something in the browser run it was picked in:
    // remember which one, so a restart is noticed instead of trusting an id
    // that may now be another tab.
    let source = match source {
        Some(Source::Tab {
            id,
            title,
            session: None,
        }) => Some(Source::Tab {
            id,
            title,
            session: app.bridge.as_ref().and_then(Bridge::current_session),
        }),
        other => other,
    };
    app.service.set_source(source.clone());
    app.store.update(|config| config.source = source)
}

#[tauri::command]
fn set_enabled(handle: AppHandle, enabled: bool) -> AppConfig {
    apply_enabled(&handle, enabled)
}

#[tauri::command]
fn set_settings(app: State<App>, settings: Settings) -> AppConfig {
    let settings = settings.sanitized();
    app.service.set_settings(settings.clone());
    app.store.update(|config| config.settings = settings)
}

/// Replaces the pairing token. The extension is disconnected and has to be
/// given the new one.
#[tauri::command]
fn regenerate_token(app: State<App>) -> Result<AppConfig, String> {
    let token = cricket_bridge::generate_token().map_err(|error| error.to_string())?;
    if let Some(bridge) = &app.bridge {
        bridge.set_token(token.clone());
    }
    Ok(app.store.update(|config| config.bridge_token = token))
}

/// Pins the browser executable when detection picks the wrong one; `None`
/// (or empty) goes back to detection.
#[tauri::command]
fn set_browser_override(app: State<App>, value: Option<String>) -> AppConfig {
    let value = value
        .map(|name| name.trim().to_lowercase())
        .filter(|name| !name.is_empty());
    if let Some(bridge) = &app.bridge {
        bridge.set_browser_override(value.as_deref().map(AppId::new));
    }
    app.store.update(|config| config.browser_override = value)
}

#[tauri::command]
fn set_minimize_to_tray(app: State<App>, value: bool) -> AppConfig {
    app.store.update(|config| config.minimize_to_tray = value)
}

#[tauri::command]
fn set_show_diagnostics(app: State<App>, value: bool) -> AppConfig {
    app.store.update(|config| config.show_diagnostics = value)
}

#[tauri::command]
fn set_mini_mode(handle: AppHandle, value: bool) -> AppConfig {
    resize_window(&handle, value);
    handle
        .state::<App>()
        .store
        .update(|config| config.mini_mode = value)
}

/// The one place the on/off switch is applied, so the window and the tray
/// cannot disagree.
fn apply_enabled(handle: &AppHandle, enabled: bool) -> AppConfig {
    let app = handle.state::<App>();
    app.service.set_enabled(enabled);
    if let Some(item) = app.tray_enabled.lock().expect("tray lock").as_ref() {
        let _ = item.set_checked(enabled);
    }
    app.store.update(|config| config.enabled = enabled)
}

fn resize_window(handle: &AppHandle, mini: bool) {
    if let Some(window) = handle.get_webview_window(MAIN_WINDOW) {
        let _ = window.set_size(if mini { MINI_SIZE } else { FULL_SIZE });
    }
}

fn show_window(handle: &AppHandle) {
    if let Some(window) = handle.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn quit(handle: &AppHandle) {
    // Hand the music source back before the process goes away.
    handle.state::<App>().service.shutdown();
    handle.exit(0);
}

fn build_tray(handle: &AppHandle, enabled: bool) -> tauri::Result<CheckMenuItem<Wry>> {
    let show = MenuItem::with_id(handle, "show", strings::TRAY_SHOW, true, None::<&str>)?;
    let enabled_item = CheckMenuItem::with_id(
        handle,
        "enabled",
        strings::TRAY_ENABLED,
        true,
        enabled,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(handle, "quit", strings::TRAY_QUIT, true, None::<&str>)?;
    let menu = Menu::with_items(
        handle,
        &[
            &show,
            &enabled_item,
            &PredefinedMenuItem::separator(handle)?,
            &quit_item,
        ],
    )?;

    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(strings::APP_NAME)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|handle, event| match event.id.as_ref() {
            "show" => show_window(handle),
            "enabled" => {
                // The item has already flipped itself; follow it.
                let app = handle.state::<App>();
                let checked = app
                    .tray_enabled
                    .lock()
                    .expect("tray lock")
                    .as_ref()
                    .and_then(|item| item.is_checked().ok());
                if let Some(checked) = checked {
                    apply_enabled(handle, checked);
                }
            }
            "quit" => quit(handle),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_window(tray.app_handle());
            }
        });
    if let Some(icon) = handle.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(handle)?;
    Ok(enabled_item)
}

/// How the bridge learns which browser an extension connection comes from.
#[cfg(windows)]
fn peer_resolver() -> Option<cricket_bridge::PeerResolver> {
    Some(std::sync::Arc::new(|peer, server_port| {
        cricket_audio_win::client_process(peer, server_port)
    }))
}

#[cfg(not(windows))]
fn peer_resolver() -> Option<cricket_bridge::PeerResolver> {
    None
}

pub fn run() {
    tauri::Builder::default()
        // Must be the first plugin. Two copies would both drive the same
        // music source, and switching one off would leave the other
        // pausing it. A second launch shows the running window instead.
        .plugin(tauri_plugin_single_instance::init(|handle, _args, _cwd| {
            show_window(handle);
        }))
        .setup(|app| {
            let handle = app.handle().clone();
            let store = ConfigStore::load(app.path().app_config_dir()?.join("config.json"));
            let mut config = store.get();
            if config.bridge_token.is_empty() {
                // First start: create the secret the extension pairs with.
                let token = cricket_bridge::generate_token()?;
                config = store.update(|config| config.bridge_token = token);
            }

            // The app works without the bridge (desktop app sources), so a
            // port that is already taken is reported, not fatal.
            let port = cricket_bridge::DEFAULT_PORT;
            let resolver = peer_resolver();
            let (bridge, bridge_error) =
                match Bridge::start_with(port, config.bridge_token.clone(), resolver) {
                    Ok(bridge) => {
                        bridge.set_browser_override(
                            config.browser_override.as_deref().map(AppId::new),
                        );
                        (Some(bridge), None)
                    }
                    Err(error) => {
                        eprintln!("bridge: could not listen on 127.0.0.1:{port}: {error}");
                        (None, Some(error.to_string()))
                    }
                };

            let tray_handle = handle.clone();
            let service = Service::start(
                &config,
                Tabs(bridge.clone()),
                BridgeStatus {
                    connected: false,
                    port,
                    error: bridge_error,
                    browser: None,
                    browser_source: None,
                },
                move |state, has_source| {
                    if let Some(tray) = tray_handle.tray_by_id(TRAY_ID) {
                        let _ = tray.set_tooltip(Some(strings::tray_tooltip(state, has_source)));
                    }
                },
            );

            app.manage(App {
                service,
                bridge,
                store,
                tray_enabled: Mutex::new(None),
            });
            let enabled_item = build_tray(&handle, config.enabled)?;
            *handle
                .state::<App>()
                .tray_enabled
                .lock()
                .expect("tray lock") = Some(enabled_item);

            resize_window(&handle, config.mini_mode);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let handle = window.app_handle();
                if handle.state::<App>().store.get().minimize_to_tray {
                    // Keep running in the tray.
                    api.prevent_close();
                    let _ = window.hide();
                } else {
                    api.prevent_close();
                    quit(handle);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            core_info,
            get_status,
            get_config,
            set_source,
            set_enabled,
            set_settings,
            set_minimize_to_tray,
            set_show_diagnostics,
            set_mini_mode,
            set_browser_override,
            regenerate_token,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the Cricket app")
        .run(|handle, event| {
            // Safety net for exits that do not go through `quit` (logoff,
            // a kill request): still hand the music source back.
            if let RunEvent::Exit = event {
                handle.state::<App>().service.shutdown();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_info_comes_from_core() {
        assert_eq!(core_info(), cricket_core::describe());
    }
}

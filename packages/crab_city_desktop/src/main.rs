// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod daemon;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::Manager;
use tauri::menu::{AboutMetadata, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

use crate::config::CrabCityConfig;
use crate::daemon::DaemonInfo;

/// Loading page HTML, embedded at compile time. Injected into the webview
/// via JS rather than Tauri's asset embedding (which requires a writable
/// OUT_DIR that Bazel doesn't provide).
const LOADING_HTML: &str = include_str!("../loading.html");

struct AppState {
    daemon_info: Arc<Mutex<Option<DaemonInfo>>>,
    config: CrabCityConfig,
    we_started: AtomicBool,
}

fn main() {
    tracing_subscriber::fmt::init();

    let config = CrabCityConfig::new(None).expect("Failed to initialize config");

    let state = AppState {
        daemon_info: Arc::new(Mutex::new(None)),
        config,
        we_started: AtomicBool::new(false),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(state)
        .invoke_handler(tauri::generate_handler![retry_daemon_startup])
        .setup(|app| {
            setup_menu(app)?;
            setup_tray(app)?;

            if let Some(window) = app.get_webview_window("main") {
                // In `cargo tauri dev`, the webview loads devUrl (Vite) before
                // setup runs. Don't inject the loading page or navigate away —
                // Vite proxies to the daemon and handles its own reconnection.
                let has_dev_frontend = window
                    .url()
                    .map(|u| u.scheme() == "http" || u.scheme() == "https")
                    .unwrap_or(false);

                let _ = window.show();

                if !has_dev_frontend {
                    inject_loading_page(&window);
                    let handle = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        start_and_navigate(handle).await;
                    });
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            match event {
                // macOS: hide window on close instead of destroying
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    #[cfg(target_os = "macos")]
                    {
                        let _ = window.hide();
                        api.prevent_close();
                    }
                    // On non-macOS, let it close and stop daemon if we started it
                    #[cfg(not(target_os = "macos"))]
                    {
                        let _ = api;
                        let state = window.state::<AppState>();
                        if state.we_started.load(Ordering::Relaxed) {
                            if let Some(info) = state.daemon_info.lock().unwrap().as_ref() {
                                tracing::info!("Stopping daemon we started (pid={})", info.pid);
                                daemon::stop_daemon(info);
                            }
                        }
                    }
                }
                _ => {}
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // Handle true app exit (Cmd+Q, tray Quit, etc.)
            if let tauri::RunEvent::Exit = event {
                let state = app_handle.state::<AppState>();
                if state.we_started.load(Ordering::Relaxed) {
                    if let Some(info) = state.daemon_info.lock().unwrap().as_ref() {
                        tracing::info!(
                            "App exiting — stopping daemon we started (pid={})",
                            info.pid
                        );
                        daemon::stop_daemon(info);
                    }
                }
            }
        });
}

// =============================================================================
// Native Menu Bar
// =============================================================================

fn setup_menu(app: &tauri::App) -> tauri::Result<()> {
    let settings_item = MenuItemBuilder::new("Settings...")
        .id("settings")
        .accelerator("CmdOrCtrl+,")
        .build(app)?;

    let reload_item = MenuItemBuilder::new("Reload")
        .id("reload")
        .accelerator("CmdOrCtrl+R")
        .build(app)?;

    let devtools_item = MenuItemBuilder::new("Toggle Developer Tools")
        .id("devtools")
        .accelerator("CmdOrCtrl+Alt+I")
        .build(app)?;

    // App submenu (becomes macOS application menu)
    let app_submenu = SubmenuBuilder::new(app, "Crab City")
        .about(Some(AboutMetadata {
            name: Some("Crab City".into()),
            version: Some(env!("CARGO_PKG_VERSION").into()),
            ..Default::default()
        }))
        .separator()
        .item(&settings_item)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let edit_submenu = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    let view_submenu = SubmenuBuilder::new(app, "View")
        .item(&reload_item)
        .item(&devtools_item)
        .separator()
        .fullscreen()
        .build()?;

    let window_submenu = SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize()
        .separator()
        .close_window()
        .build()?;

    let menu = MenuBuilder::new(app)
        .items(&[&app_submenu, &edit_submenu, &view_submenu, &window_submenu])
        .build()?;

    app.set_menu(menu)?;

    app.on_menu_event(move |app_handle, event| match event.id().as_ref() {
        "settings" => {
            navigate_to_fragment(app_handle, "settings");
        }
        "reload" => {
            if let Some(window) = app_handle.get_webview_window("main") {
                // Re-navigate to the daemon URL to force reload
                let state = app_handle.state::<AppState>();
                if let Some(info) = state.daemon_info.lock().unwrap().as_ref() {
                    let url: tauri::Url = info.base_url().parse().expect("invalid daemon URL");
                    let _ = window.navigate(url);
                }
            }
        }
        "devtools" => {
            if let Some(window) = app_handle.get_webview_window("main") {
                if window.is_devtools_open() {
                    window.close_devtools();
                } else {
                    window.open_devtools();
                }
            }
        }
        _ => {}
    });

    Ok(())
}

/// Navigate to a fragment route by evaluating JS in the webview.
fn navigate_to_fragment(app_handle: &tauri::AppHandle, fragment: &str) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
        let js = format!(
            "if (window.location.hostname === 'localhost' && window.location.port !== '5173') {{ window.location.hash = '{}'; }}",
            fragment
        );
        let _ = window.eval(&js);
    }
}

// =============================================================================
// System Tray
// =============================================================================

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let show_item =
        tauri::menu::MenuItem::with_id(app, "tray_show", "Show Window", true, None::<&str>)?;
    let quit_item =
        tauri::menu::MenuItem::with_id(app, "tray_quit", "Quit Crab City", true, None::<&str>)?;
    let menu = tauri::menu::Menu::with_items(app, &[&show_item, &quit_item])?;

    let mut builder = TrayIconBuilder::new()
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("Crab City")
        .on_menu_event(|app_handle, event| match event.id.as_ref() {
            "tray_show" => {
                show_main_window(app_handle);
            }
            "tray_quit" => {
                app_handle.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });

    // Use app icon if available
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;

    Ok(())
}

fn show_main_window(app_handle: &tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

// =============================================================================
// Tauri IPC Commands
// =============================================================================

/// Called from loading.html when the user clicks "Retry".
/// Re-runs daemon startup and navigates on success.
#[tauri::command]
async fn retry_daemon_startup(handle: tauri::AppHandle) -> Result<(), String> {
    start_and_navigate(handle).await;
    Ok(())
}

// =============================================================================
// Daemon Startup & Health Monitor
// =============================================================================

/// Start (or discover) the daemon, then navigate the webview to it.
async fn start_and_navigate(handle: tauri::AppHandle) {
    let state = handle.state::<AppState>();
    let config = state.config.clone();

    // Update loading status
    eval_loading(&handle, "setStatus('Checking for daemon...')");

    match daemon::ensure_daemon(&config).await {
        Ok((info, we_started)) => {
            let url = info.base_url();
            tracing::info!("Daemon ready at {url}");

            state.we_started.store(we_started, Ordering::Relaxed);
            *state.daemon_info.lock().unwrap() = Some(info);

            eval_loading(&handle, "setStatus('Connected — loading UI...')");

            // Small delay for the status message to render, then fade and navigate
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            if let Some(window) = handle.get_webview_window("main") {
                // Fade out the loading screen, then navigate
                let navigate_js = format!("fadeOutAndNavigate('{}')", url.replace('\'', "\\'"));
                let _ = window.eval(&navigate_js);
            }

            spawn_health_monitor(handle);
        }
        Err(err) => {
            tracing::error!("Failed to start daemon: {err:#}");
            if let Some(window) = handle.get_webview_window("main") {
                let msg = err.to_string().replace('\\', "\\\\").replace('\'', "\\'");
                let js = format!("showError('{}')", msg);
                let _ = window.eval(&js);
            }
        }
    }
}

/// Inject the loading page into the webview via DOM manipulation.
///
/// Uses innerHTML + manual script execution instead of document.write(),
/// which would clobber Tauri's injected IPC scripts (__TAURI_INTERNALS__).
fn inject_loading_page(window: &tauri::WebviewWindow) {
    // Parse out <style> and <body> content from the loading HTML, then inject
    // via DOM APIs that preserve Tauri's existing script context.
    let mut js = String::from(
        "(function() {\
         var parser = new DOMParser();\
         var doc = parser.parseFromString(`",
    );
    js.push_str(LOADING_HTML);
    js.push_str(
        "`, 'text/html');\
         document.head.innerHTML = doc.head.innerHTML;\
         document.body.innerHTML = doc.body.innerHTML;\
         doc.querySelectorAll('script').forEach(function(s) {\
           var ns = document.createElement('script');\
           ns.textContent = s.textContent;\
           document.body.appendChild(ns);\
         });\
         })();",
    );
    let _ = window.eval(&js);
}

/// Evaluate JS on the loading page (helper to keep main logic clean).
fn eval_loading(handle: &tauri::AppHandle, js: &str) {
    if let Some(window) = handle.get_webview_window("main") {
        let _ = window.eval(js);
    }
}

/// Background task that polls daemon health every 5 seconds.
/// On failure, tries to rediscover the daemon (it may have restarted on a new port).
fn spawn_health_monitor(handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let interval = std::time::Duration::from_secs(5);
        loop {
            tokio::time::sleep(interval).await;

            let state = handle.state::<AppState>();
            let current_info = state.daemon_info.lock().unwrap().clone();

            let Some(info) = current_info else {
                continue;
            };

            if daemon::health_check(&info).await {
                continue;
            }

            // Daemon is unhealthy — try to rediscover
            tracing::warn!("Daemon health check failed, attempting rediscovery...");
            if let Some(new_info) = daemon::rediscover_daemon(&state.config).await {
                let new_url = new_info.base_url();
                tracing::info!("Rediscovered daemon at {new_url}");
                *state.daemon_info.lock().unwrap() = Some(new_info);

                if let Some(window) = handle.get_webview_window("main") {
                    let navigate_url: tauri::Url = new_url.parse().expect("invalid daemon URL");
                    let _ = window.navigate(navigate_url);
                }
            } else {
                // Daemon is truly gone. The SvelteKit reconnection UI handles
                // the user experience (exponential backoff → "Server Offline").
                tracing::warn!("Daemon not found — frontend reconnection UI will handle UX");
            }
        }
    });
}

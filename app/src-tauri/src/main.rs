//! Poddies.
//!
//! The same executable is both the app and the plugin sandbox: spawning it with
//! `--plugin-worker <dir>` runs a single plugin in isolation. That is how the
//! shipped product stays a single portable EXE while still isolating plugins in
//! their own processes.

// Release builds are GUI-only; debug keeps the console for diagnostics.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod services;
mod state;
mod views;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WindowEvent};

use poddies_core::library::Library;
use poddies_plugin_host::{LoadReport, PluginHost, WorkerLauncher};

use crate::services::{LibraryServices, SharedLibrary};
use crate::state::{plugin_search_paths, AppState};

fn main() {
    // Worker mode must be handled before Tauri starts, so no window is created.
    if let Some(directory) = plugin_worker_dir() {
        let code = match poddies_plugin_host::run_worker(std::path::Path::new(&directory)) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("[poddies] plugin worker failed: {error}");
                1
            }
        };
        std::process::exit(code);
    }

    tauri::Builder::default()
        .setup(|app| {
            setup(app)?;
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Closing hides to the tray rather than quitting; the tray menu owns
            // the real exit.
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            // A minimise from the taskbar should also land in the tray rather
            // than in a titlebar-less window the user cannot get back to.
            WindowEvent::Resized(_) => {
                if window.is_minimized().unwrap_or(false) {
                    let _ = window.hide();
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::snapshot,
            commands::episodes_for_show,
            commands::subscribe,
            commands::unsubscribe,
            commands::remove_show,
            commands::refresh_all,
            commands::discovery,
            commands::search,
            commands::preferences,
            commands::set_weights,
            commands::set_avoid_explicit,
            commands::plugin_panels,
            commands::plugin_panel_content,
            commands::plugin_status,
            commands::plugins_directory,
            commands::plugin_reload,
            commands::playback_started,
            commands::record_progress,
            commands::app_hide,
            commands::app_start_drag,
            commands::app_toggle_maximise,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Poddies");
}

fn plugin_worker_dir() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--plugin-worker" {
            return args.next();
        }
    }
    None
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&data_dir)?;

    let library = Library::load(&data_dir.join("library.json"));
    eprintln!(
        "[poddies] library: {} shows, {} episodes",
        library.shows.len(),
        library.episodes.len()
    );

    let shared = Arc::new(SharedLibrary(Mutex::new(library)));
    let services = Arc::new(LibraryServices {
        library: Arc::clone(&shared),
    });

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|parent| parent.to_path_buf()));

    let mut plugins = PluginHost::new(WorkerLauncher::current_exe()?, services);
    let mut reports: Vec<LoadReport> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for path in plugin_search_paths(exe_dir.as_deref(), &data_dir) {
        if !path.is_dir() {
            continue;
        }

        for (directory, manifest) in plugins.discover(&path) {
            let plugin_id = manifest.as_ref().ok().map(|manifest| manifest.id.clone());
            if let Some(id) = &plugin_id {
                if !seen.insert(id.clone()) {
                    continue;
                }
            }

            let result = plugins.load_dir(&directory).map_err(|error| error.message);
            match &result {
                Ok(()) => eprintln!("[poddies] plugin loaded: {plugin_id:?}"),
                Err(error) => eprintln!(
                    "[poddies] plugin failed ({}): {error}",
                    directory.display()
                ),
            }
            reports.push(LoadReport {
                directory,
                plugin_id,
                result,
            });
        }
    }

    let plugins_dir = data_dir.join("plugins");
    app.manage(AppState {
        library: shared,
        plugins: Mutex::new(plugins),
        plugin_reports: reports,
        plugins_dir,
        data_dir,
    });

    install_tray(app)?;
    Ok(())
}

fn install_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let show = MenuItemBuilder::with_id("show", "Open Poddies").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
    let menu = MenuBuilder::new(app).items(&[&show, &quit]).build()?;

    let mut builder = TrayIconBuilder::with_id("poddies")
        .tooltip("Poddies")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => reveal(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                reveal(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    Ok(())
}

/// Bring the window back from the tray and tell the UI to play its reveal
/// animation.
fn reveal(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.emit("poddies://reveal", ());
    }
}

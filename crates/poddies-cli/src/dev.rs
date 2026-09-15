//! `poddies-cli dev` — a reload-on-save loop for plugin development.
//!
//! Builds a Rust plugin, loads it through the *real* sandboxed host (same
//! worker, same Job Object, same protocol as production), prints what it
//! contributes, then rebuilds and reloads whenever a file changes.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use poddies_plugin_api::manifest::{Capability, PluginManifest, Runtime};
use poddies_plugin_api::ui::Widget;
use poddies_plugin_host::{HostServices, PluginHost, WorkerLauncher};

/// Stands in for the app while developing a plugin, so a panel renders with
/// believable numbers instead of zeroes.
struct DevServices;

impl HostServices for DevServices {
    fn library_shows(&self) -> serde_json::Value {
        serde_json::json!([
            {
                "id": "sh_1",
                "title": "Syntax",
                "author": "Wes Bos & Scott Tolinski",
                "categories": ["Technology"],
                "subscribed": true,
                "episode_count": 812
            },
            {
                "id": "sh_2",
                "title": "99% Invisible",
                "author": "Roman Mars",
                "categories": ["Design"],
                "subscribed": true,
                "episode_count": 610
            }
        ])
    }

    fn library_history(&self) -> serde_json::Value {
        serde_json::json!([
            {
                "episode_id": "ep_1", "show_id": "sh_1", "title": "Modern CSS",
                "show_title": "Syntax", "position_secs": 2400.0,
                "duration_secs": 2400, "completed": true,
                "last_played": "2026-09-01T10:00:00Z"
            },
            {
                "episode_id": "ep_2", "show_id": "sh_1", "title": "TypeScript tips",
                "show_title": "Syntax", "position_secs": 600.0,
                "duration_secs": 2700, "completed": false,
                "last_played": "2026-09-02T10:00:00Z"
            },
            {
                "episode_id": "ep_3", "show_id": "sh_2", "title": "The Vault",
                "show_title": "99% Invisible", "position_secs": 1900.0,
                "duration_secs": 2000, "completed": true,
                "last_played": "2026-09-03T10:00:00Z"
            }
        ])
    }

    fn log(&self, level: &str, message: &str) {
        println!("  [plugin log] {level}: {message}");
    }
}

pub fn run(args: &[String]) -> ExitCode {
    let once = args.iter().any(|arg| arg == "--once");
    let Some(dir) = args.iter().find(|arg| !arg.starts_with("--")) else {
        eprintln!("usage: poddies-cli dev <plugin-dir> [--once]");
        return ExitCode::from(2);
    };
    let dir = PathBuf::from(dir);
    if !dir.is_dir() {
        eprintln!("{} is not a directory", dir.display());
        return ExitCode::from(2);
    }

    let Some(worker) = worker_binary() else {
        eprintln!("could not locate the poddies-plugin-worker binary");
        return ExitCode::FAILURE;
    };

    println!("poddies dev — {}", dir.display());
    if !once {
        println!("watching for changes; press Ctrl+C to stop\n");
    } else {
        println!();
    }

    loop {
        if dir.join("Cargo.toml").is_file() {
            let built = build_rust(&dir);
            if !built {
                println!("build failed — waiting for a change");
                if once {
                    return ExitCode::FAILURE;
                }
            }
        }

        let ok = load_and_report(&dir, &worker);

        if once {
            return if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE };
        }

        let stamp = dir_stamp(&dir);
        loop {
            std::thread::sleep(Duration::from_millis(500));
            if dir_stamp(&dir) != stamp {
                println!("\nchange detected — reloading\n");
                break;
            }
        }
    }
}

fn load_and_report(dir: &Path, worker: &Path) -> bool {
    let mut launcher = WorkerLauncher::new(worker);
    if let Some(python) = crate::scaffold::python_sdk_path() {
        launcher = launcher.with_env("PYTHONPATH", python.to_string_lossy().into_owned());
    }

    let mut host = PluginHost::new(launcher, Arc::new(DevServices));

    if let Err(error) = host.load_dir(dir) {
        println!("  load failed: {} ({})", error.message, error.code);
        return false;
    }

    let Some(plugin) = host.plugins().first() else {
        println!("  loaded nothing");
        return false;
    };

    println!(
        "  loaded '{}' v{} (protocol {})",
        plugin.info.name, plugin.info.version, plugin.info.protocol
    );

    if !plugin.stderr_tail().is_empty() {
        for line in plugin.stderr_tail() {
            println!("  [stderr] {line}");
        }
    }

    for panel in &plugin.info.ui_panels {
        match host.panel_content(0, &panel.id) {
            Ok(content) => {
                println!(
                    "\n  panel '{}' ({} widgets)",
                    panel.title,
                    content.widgets.len()
                );
                for widget in &content.widgets {
                    println!("    {}", describe_widget(widget));
                }
            }
            Err(error) => println!("  panel '{}' failed: {}", panel.id, error.message),
        }
    }

    if plugin.has(Capability::DiscoverySource) {
        let candidates = host.discovery_candidates(
            5,
            &["Technology".to_string()],
            Duration::from_secs(12),
        );
        println!("\n  discovery: {} candidates", candidates.len());
        for candidate in candidates.iter().take(5) {
            println!(
                "    {}  [{}]",
                candidate.title,
                candidate.categories.join(", ")
            );
        }
        if candidates.is_empty() {
            println!("    (none — check the plugin's network access)");
        }
    }

    let ok = plugin.is_alive();
    host.shutdown_all();
    ok
}

fn describe_widget(widget: &Widget) -> String {
    match widget {
        Widget::Heading { text } => format!("# {text}"),
        Widget::Metric { label, value } => format!("{value}  {label}"),
        Widget::Text { text } => text.clone(),
        Widget::Divider => "\u{2500}".to_string(),
        Widget::Bar { label, value, max } => format!("[{label}] {value}/{max}"),
        Widget::List { items } => format!("list of {}", items.len()),
        Widget::Knob { label, value, style, readout, .. } => format!(
            "knob '{label}' = {value} ({:?}){}",
            style,
            readout.as_deref().map(|r| format!("  {r}")).unwrap_or_default()
        ),
        Widget::Slider { label, value, unit, .. } => format!("slider '{label}' = {value} {unit}"),
        Widget::Toggle { label, value, .. } => format!("toggle '{label}' = {value}"),
        Widget::Eq { bands, .. } => format!("eq curve, {} bands", bands.len()),
        Widget::Meter { label, source, .. } => format!("meter '{label}' from {source:?}"),
    }
}

fn worker_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for name in ["poddies-plugin-worker.exe", "poddies-plugin-worker"] {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    Some(PathBuf::from("poddies-plugin-worker"))
}

fn build_rust(dir: &Path) -> bool {
    print!("  building… ");
    let status = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status();

    let ok = matches!(&status, Ok(status) if status.success());
    if !ok {
        return false;
    }

    copy_native_library(dir);
    true
}

/// Move the freshly built cdylib next to `plugin.json` where the worker expects
/// to find it. The target directory is resolved via `cargo metadata` so this
/// works both for a standalone plugin and for one that lives in a workspace.
fn copy_native_library(dir: &Path) {
    let Ok(text) = std::fs::read_to_string(dir.join("plugin.json")) else {
        return;
    };
    let Ok(manifest) = PluginManifest::parse(&text) else {
        return;
    };
    let Runtime::Native { library } = manifest.runtime else {
        return;
    };

    let Some(target) = target_directory(dir) else {
        eprintln!("could not resolve the cargo target directory");
        return;
    };

    let source = target.join("release").join(&library);
    if !source.is_file() {
        eprintln!("{library} was not produced by the build");
        return;
    }

    match std::fs::copy(&source, dir.join(&library)) {
        Ok(_) => println!("built and copied {library}"),
        Err(err) => eprintln!("could not copy {library}: {err}"),
    }
}

fn target_directory(dir: &Path) -> Option<PathBuf> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(dir)
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    value
        .get("target_directory")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
}

/// A cheap fingerprint of the plugin directory, used to detect edits. `target`
/// and `.git` are skipped so a build does not trigger another reload.
fn dir_stamp(dir: &Path) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut stack = vec![dir.to_path_buf()];
    let mut entries: Vec<(PathBuf, u64, u64)> = Vec::new();

    while let Some(current) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let skip = path
                    .file_name()
                    .map(|name| name == "target" || name == ".git")
                    .unwrap_or(true);
                if !skip {
                    stack.push(path);
                }
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or(0);
            entries.push((path, metadata.len(), modified));
        }
    }

    entries.sort();
    for (path, len, modified) in entries {
        for byte in path.to_string_lossy().bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= len;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        hash ^= modified;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

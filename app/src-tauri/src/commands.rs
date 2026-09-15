//! Tauri commands: the entire surface the UI can call.
//!
//! Every command is synchronous and short. Feed refreshes are the only slow
//! operation, and the UI keeps them off the interactive path.

use std::sync::MutexGuard;
use std::time::Duration;

use poddies_core::discovery::{build_profile, rank, ListeningProfile, ScoredCandidate, Weights};
use poddies_core::fetch::{FeedClient, Fetched};
use poddies_core::library::Library;
use poddies_core::model::{Episode, Show};
use poddies_core::util::title_case;
use poddies_plugin_api::protocol::{methods, AudioUnit, PlaybackEvent, WidgetChange};
use poddies_plugin_api::ui::PanelContent;
use poddies_plugin_host::PluginHost;
use serde::Serialize;
use serde_json::Value;
use tauri::State;

use crate::services::{is_subscribed, lock_library};
use crate::state::{plugin_search_paths, AppState};
use crate::views::{
    rfc3339, DiscoveryItemView, EpisodeView, LibraryView, PanelView, PluginStatusView, ReasonView,
    RefreshSummary, SearchView, ShowView,
};

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(12);
const LATEST_LIMIT: usize = 300;
const IN_PROGRESS_LIMIT: usize = 24;
const SEARCH_EPISODES_LIMIT: usize = 150;
const SEARCH_SHOWS_LIMIT: usize = 24;
const WEB_RESULTS_LIMIT: usize = 12;

fn lock_plugins(state: &AppState) -> MutexGuard<'_, PluginHost> {
    state
        .plugins
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn show_view(library: &Library, show: &Show) -> ShowView {
    ShowView {
        id: show.id.clone(),
        title: show.title.clone(),
        author: show.author.clone(),
        description: show.description.clone(),
        image_url: show.image_url.clone(),
        categories: show.categories.clone(),
        explicit: show.explicit,
        episode_count: library
            .episodes
            .values()
            .filter(|episode| episode.show_id == show.id)
            .count(),
    }
}

fn episode_view(library: &Library, episode: &Episode) -> EpisodeView {
    let progress = library.playback.get(&episode.id);
    EpisodeView {
        id: episode.id.clone(),
        show_id: episode.show_id.clone(),
        show_title: library
            .show(&episode.show_id)
            .map(|show| show.title.clone())
            .unwrap_or_else(|| "Unknown show".to_string()),
        title: episode.title.clone(),
        description: episode.description.clone(),
        image_url: episode.image_url.clone(),
        enclosure_url: episode.enclosure_url.clone(),
        duration_secs: episode
            .duration_secs
            .or_else(|| progress.and_then(|state| state.duration_secs)),
        published: rfc3339(episode.published),
        position_secs: progress.map(|state| state.position_secs).unwrap_or(0.0),
        completed: progress.map(|state| state.is_finished()).unwrap_or(false),
        play_count: progress.map(|state| state.play_count).unwrap_or(0),
    }
}

fn describe_fetch_error(error: &poddies_core::CoreError) -> String {
    match error {
        poddies_core::CoreError::Network(_) => {
            "Couldn't reach that address. Check the URL and your connection.".to_string()
        }
        poddies_core::CoreError::Feed(_) => "That address isn't a podcast feed.".to_string(),
        other => other.to_string(),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PreferencesView {
    pub avoid_explicit: bool,
    pub discovery: Weights,
}

#[tauri::command]
pub fn search(query: String, state: State<'_, AppState>) -> SearchView {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return SearchView {
            shows: Vec::new(),
            episodes: Vec::new(),
            web: Vec::new(),
        };
    }

    // The library lock is taken in a scope and dropped before the network call:
    // holding it across Apple's endpoint would freeze every other command for
    // as long as the request takes.
    let (shows, episodes) = {
        let library = lock_library(&state.library);
        let hit =
            |text: Option<&String>| text.is_some_and(|value| value.to_lowercase().contains(&needle));

        let shows = library
            .subscriptions()
            .iter()
            .filter(|show| {
                hit(Some(&show.title))
                    || hit(show.author.as_ref())
                    || show.categories.iter().any(|category| category.to_lowercase().contains(&needle))
            })
            .take(SEARCH_SHOWS_LIMIT)
            .map(|show| show_view(&library, show))
            .collect();

        let mut matched: Vec<&Episode> = library
            .episodes
            .values()
            .filter(|episode| hit(Some(&episode.title)) || hit(episode.description.as_ref()))
            .take(SEARCH_EPISODES_LIMIT)
            .collect();
        matched.sort_by(|a, b| b.published.cmp(&a.published));

        let episodes = matched
            .iter()
            .map(|episode| episode_view(&library, episode))
            .collect();

        (shows, episodes)
    };

    // Apple Podcasts is public and unauthenticated, so a query can surface
    // shows that are not subscribed yet. Failure degrades to local results.
    let web = poddies_core::itunes::search(&query, WEB_RESULTS_LIMIT);
    let library = lock_library(&state.library);
    let web = match web {
        Ok(candidates) => candidates
            .into_iter()
            .map(|candidate| to_web_item_view(&library, candidate))
            .collect(),
        Err(error) => {
            eprintln!("[poddies] apple podcasts search: {error}");
            Vec::new()
        }
    };

    SearchView {
        shows,
        episodes,
        web,
    }
}

#[tauri::command]
pub fn snapshot(state: State<'_, AppState>) -> LibraryView {
    let library = lock_library(&state.library);

    let shows = library
        .subscriptions()
        .iter()
        .map(|show| show_view(&library, show))
        .collect();

    let latest = library
        .latest_episodes(LATEST_LIMIT)
        .iter()
        .map(|episode| episode_view(&library, episode))
        .collect();

    let in_progress = library
        .in_progress(IN_PROGRESS_LIMIT)
        .iter()
        .map(|(episode, _)| episode_view(&library, episode))
        .collect();

    LibraryView {
        shows,
        latest,
        in_progress,
    }
}

#[tauri::command]
pub fn episodes_for_show(show_id: String, state: State<'_, AppState>) -> Vec<EpisodeView> {
    let library = lock_library(&state.library);
    library
        .episodes_for_show(&show_id)
        .iter()
        .map(|episode| episode_view(&library, episode))
        .collect()
}

#[tauri::command]
pub fn subscribe(url: String, state: State<'_, AppState>) -> Result<ShowView, String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err("Enter a feed address.".to_string());
    }

    let client = FeedClient::new().map_err(|error| format!("Network unavailable: {error}"))?;
    let fetched = client
        .fetch(&url, None, None)
        .map_err(|error| describe_fetch_error(&error))?;

    let (parsed, etag, last_modified) = match fetched {
        Fetched::Updated {
            parsed,
            etag,
            last_modified,
        } => (parsed, etag, last_modified),
        Fetched::NotModified => return Err("That address returned no feed.".to_string()),
    };

    let show_id = parsed.show.id.clone();
    {
        let mut library = lock_library(&state.library);
        library.subscribe(*parsed);
        library.set_feed_meta(&show_id, etag, last_modified);
        let _ = library.save(&state.library_path());
    }

    let library = lock_library(&state.library);
    library
        .show(&show_id)
        .map(|show| show_view(&library, show))
        .ok_or_else(|| "The feed could not be saved.".to_string())
}

#[tauri::command]
pub fn unsubscribe(show_id: String, state: State<'_, AppState>) -> LibraryView {
    {
        let mut library = lock_library(&state.library);
        library.unsubscribe(&show_id);
        let _ = library.save(&state.library_path());
    }
    snapshot(state)
}

#[tauri::command]
pub fn remove_show(show_id: String, state: State<'_, AppState>) -> LibraryView {
    {
        let mut library = lock_library(&state.library);
        library.remove_show(&show_id);
        let _ = library.save(&state.library_path());
    }
    snapshot(state)
}

#[tauri::command]
pub fn refresh_all(state: State<'_, AppState>) -> RefreshSummary {
    let targets: Vec<(String, String, Option<String>, Option<String>)> = {
        let library = lock_library(&state.library);
        library
            .subscriptions()
            .iter()
            .map(|show| {
                (
                    show.id.clone(),
                    show.feed_url.clone(),
                    show.etag.clone(),
                    show.last_modified.clone(),
                )
            })
            .collect()
    };

    let Ok(client) = FeedClient::new() else {
        return RefreshSummary {
            failed: vec!["Network unavailable".to_string()],
            ..Default::default()
        };
    };

    let mut summary = RefreshSummary::default();
    let mut changed = false;

    for (show_id, feed_url, etag, last_modified) in targets {
        summary.checked += 1;

        match client.fetch(&feed_url, etag.as_deref(), last_modified.as_deref()) {
            Ok(Fetched::NotModified) => {}
            Ok(Fetched::Updated {
                parsed,
                etag,
                last_modified,
            }) => {
                let mut library = lock_library(&state.library);
                let stats = library.refresh(*parsed);
                library.set_feed_meta(&show_id, etag, last_modified);
                summary.updated += 1;
                summary.new_episodes += stats.new_episodes;
                changed = true;
            }
            Err(error) => summary
                .failed
                .push(format!("{feed_url}: {}", describe_fetch_error(&error))),
        }
    }

    if changed {
        let library = lock_library(&state.library);
        let _ = library.save(&state.library_path());
    }

    summary
}

#[tauri::command]
pub fn discovery(limit: usize, state: State<'_, AppState>) -> Vec<DiscoveryItemView> {
    let (profile, weights) = {
        let library = lock_library(&state.library);
        (build_profile(&library), library.settings.discovery.clone())
    };

    let topics = top_topics(&profile, 3);
    let candidates = {
        let plugins = lock_plugins(&state);
        plugins.discovery_candidates(limit.saturating_mul(3).max(24), &topics, DISCOVERY_TIMEOUT)
    };

    let scored = rank(&candidates, &profile, &weights, limit);

    let library = lock_library(&state.library);
    scored
        .into_iter()
        .map(|item| to_item_view(&library, item))
        .collect()
}

fn top_topics(profile: &ListeningProfile, count: usize) -> Vec<String> {
    let mut topics: Vec<(&String, f64)> = profile
        .category_weights
        .iter()
        .map(|(category, weight)| (category, *weight))
        .collect();
    topics.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    topics
        .into_iter()
        .take(count)
        .map(|(category, _)| title_case(category))
        .collect()
}

/// Map a scored candidate into what the interface shows. `reasons` stays empty
/// for an unscored candidate (an Apple Podcasts result was looked up, not
/// ranked), and the row then simply omits the explanation chips.
fn to_item_view(library: &Library, scored: ScoredCandidate) -> DiscoveryItemView {
    let reasons: Vec<ReasonView> = scored
        .top_reasons(3)
        .into_iter()
        .map(|factor| ReasonView {
            label: factor.label.clone(),
            weight: factor.weight,
            contribution: factor.contribution,
        })
        .collect();

    DiscoveryItemView {
        subscribed: is_subscribed(library, &scored.candidate.feed_url),
        title: scored.candidate.title,
        feed_url: scored.candidate.feed_url,
        image_url: scored.candidate.image_url,
        author: scored.candidate.author,
        categories: scored.candidate.categories,
        source: scored.candidate.source,
        explicit: scored.candidate.explicit,
        score: scored.score,
        reasons,
    }
}

/// Map an unranked candidate (search results) with a zero score.
fn to_web_item_view(
    library: &Library,
    candidate: poddies_plugin_api::DiscoveryCandidate,
) -> DiscoveryItemView {
    to_item_view(
        library,
        ScoredCandidate {
            candidate,
            score: 0.0,
            factors: Vec::new(),
        },
    )
}

#[tauri::command]
pub fn preferences(state: State<'_, AppState>) -> PreferencesView {
    let library = lock_library(&state.library);
    PreferencesView {
        avoid_explicit: library.settings.avoid_explicit,
        discovery: library.settings.discovery.clone(),
    }
}

#[tauri::command]
pub fn set_weights(weights: Weights, state: State<'_, AppState>) {
    let mut library = lock_library(&state.library);
    library.settings.discovery = weights;
    let _ = library.save(&state.library_path());
}

#[tauri::command]
pub fn set_avoid_explicit(value: bool, state: State<'_, AppState>) {
    let mut library = lock_library(&state.library);
    library.settings.avoid_explicit = value;
    let _ = library.save(&state.library_path());
}

#[tauri::command]
pub fn plugin_panels(state: State<'_, AppState>) -> Vec<PanelView> {
    let plugins = lock_plugins(&state);
    plugins
        .panels()
        .into_iter()
        .map(|entry| PanelView {
            plugin_index: entry.plugin_index,
            plugin_name: plugins
                .plugin(entry.plugin_index)
                .map(|plugin| plugin.info.name.clone())
                .unwrap_or_default(),
            plugin_id: entry.plugin_id,
            panel_id: entry.descriptor.id,
            title: entry.descriptor.title,
            placement: entry.descriptor.placement,
        })
        .collect()
}

#[tauri::command]
pub fn plugin_panel_content(
    plugin_index: usize,
    panel_id: String,
    state: State<'_, AppState>,
) -> Result<PanelContent, String> {
    let plugins = lock_plugins(&state);
    plugins
        .panel_content(plugin_index, &panel_id)
        .map_err(|error| error.message)
}

/// Relay a control movement to the plugin that owns it.
///
/// Fire and forget: the plugin updates its state and the caller re-reads the
/// panel. Kept as its own command so a drag never blocks on a reply.
#[tauri::command]
pub fn plugin_panel_change(
    plugin_index: usize,
    panel_id: String,
    widget_id: String,
    value: Value,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let change = WidgetChange {
        panel_id,
        widget_id,
        value,
    };
    lock_plugins(&state)
        .notify_change(plugin_index, &change)
        .map_err(|error| error.message)
}

/// The audio units every enabled plugin wants in the playback chain.
#[tauri::command]
pub fn audio_graph(state: State<'_, AppState>) -> Vec<AudioUnit> {
    lock_plugins(&state).audio_graph()
}


#[tauri::command]
pub fn plugin_status(state: State<'_, AppState>) -> Vec<PluginStatusView> {
    let disabled = {
        let library = lock_library(&state.library);
        library.settings.disabled_plugins.clone()
    };

    let mut statuses: Vec<PluginStatusView> = {
        let plugins = lock_plugins(&state);
        plugins
            .plugins()
            .iter()
            .enumerate()
            .map(|(index, plugin)| {
                let alive = plugin.is_alive();
                PluginStatusView {
                    id: plugin.manifest.id.clone(),
                    name: plugin.info.name.clone(),
                    version: plugin.info.version.clone(),
                    ok: alive,
                    enabled: true,
                    detail: if alive {
                        "Running".to_string()
                    } else {
                        "Stopped unexpectedly".to_string()
                    },
                    index: Some(index),
                }
            })
            .collect()
    };

    // A plugin the user switched off is not loaded, so it has to be found on
    // disk to appear in the list at all.
    let known: Vec<String> = statuses.iter().map(|status| status.id.clone()).collect();
    for path in plugin_search_paths(
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|parent| parent.to_path_buf()))
            .as_deref(),
        &state.data_dir,
    ) {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let directory = entry.path();
            if !directory.is_dir() {
                continue;
            }
            let Ok(manifest) = poddies_plugin_host::worker::load_manifest(&directory) else {
                continue;
            };
            if !disabled.contains(&manifest.id) || known.contains(&manifest.id) {
                continue;
            }
            statuses.push(PluginStatusView {
                id: manifest.id,
                name: manifest.name,
                version: manifest.version,
                ok: false,
                enabled: false,
                detail: "Disabled".to_string(),
                index: None,
            });
        }
    }

    for report in &state.plugin_reports {
        if let Err(error) = &report.result {
            let label = report.plugin_id.clone().unwrap_or_else(|| {
                report
                    .directory
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "unknown".to_string())
            });
            // A disabled plugin reports "failed to load" at startup precisely
            // because we skipped it; do not call that a failure.
            if disabled.contains(&label) {
                continue;
            }
            statuses.push(PluginStatusView {
                id: label.clone(),
                name: label,
                version: String::new(),
                ok: false,
                enabled: true,
                detail: error.clone(),
                index: None,
            });
        }
    }

    // Loaded, disabled and failed entries arrive from three different sources.
    // Sort them so a plugin keeps its place in the list when it is switched off,
    // instead of jumping to the bottom.
    statuses.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });

    statuses
}

/// Turn a plugin on or off. Enabling spawns a worker immediately; disabling
/// stops one, so both take effect without a restart.
#[tauri::command]
pub fn plugin_set_enabled(
    plugin_id: String,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let directory = find_plugin_directory(&state, &plugin_id)
        .ok_or_else(|| format!("{plugin_id} is not installed"))?;

    if enabled {
        let already_loaded = lock_plugins(&state)
            .plugins()
            .iter()
            .any(|plugin| plugin.manifest.id == plugin_id);
        if !already_loaded {
            lock_plugins(&state)
                .load_dir(&directory)
                .map_err(|error| error.message)?;
        }
    } else {
        let index = lock_plugins(&state)
            .plugins()
            .iter()
            .position(|plugin| plugin.manifest.id == plugin_id);
        if let Some(index) = index {
            lock_plugins(&state)
                .unload(index)
                .map_err(|error| error.message)?;
        }
    }

    let mut library = lock_library(&state.library);
    library.settings.disabled_plugins.retain(|id| id != &plugin_id);
    if !enabled {
        library.settings.disabled_plugins.push(plugin_id.clone());
    }
    let _ = library.save(&state.library_path());
    Ok(())
}

#[tauri::command]
pub fn open_plugins_folder(state: State<'_, AppState>) -> Result<(), String> {
    let directory = state.data_dir.join("plugins");
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    reveal_in_file_manager(&directory)
}

#[cfg(windows)]
fn reveal_in_file_manager(path: &std::path::Path) -> Result<(), String> {
    std::process::Command::new("explorer")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
fn reveal_in_file_manager(path: &std::path::Path) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn reveal_in_file_manager(path: &std::path::Path) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Locate a plugin on disk by its id, across every search path.
fn find_plugin_directory(state: &AppState, plugin_id: &str) -> Option<std::path::PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|parent| parent.to_path_buf()));

    for path in plugin_search_paths(exe_dir.as_deref(), &state.data_dir) {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let directory = entry.path();
            if !directory.is_dir() {
                continue;
            }
            if let Ok(manifest) = poddies_plugin_host::worker::load_manifest(&directory) {
                if manifest.id == plugin_id {
                    return Some(directory);
                }
            }
        }
    }
    None
}

fn playback_event(state: &AppState, episode_id: &str, position_secs: f64) -> Option<PlaybackEvent> {
    let library = lock_library(&state.library);
    let episode = library.episodes.get(episode_id)?;
    Some(PlaybackEvent {
        episode_id: episode.id.clone(),
        show_id: episode.show_id.clone(),
        title: episode.title.clone(),
        position_secs,
        duration_secs: episode.duration_secs,
    })
}

fn broadcast_playback(state: &AppState, method: &str, event: &PlaybackEvent) {
    let payload = serde_json::to_value(event).unwrap_or(Value::Null);
    lock_plugins(state).broadcast(method, payload);
}

#[tauri::command]
pub fn playback_started(episode_id: String, state: State<'_, AppState>) {
    if let Some(event) = playback_event(&state, &episode_id, 0.0) {
        broadcast_playback(&state, methods::EVENT_PLAYBACK_STARTED, &event);
    }
}

#[tauri::command]
pub fn record_progress(
    episode_id: String,
    position_secs: f64,
    duration_secs: Option<u64>,
    completed: bool,
    state: State<'_, AppState>,
) {
    let finished_before = {
        let mut library = lock_library(&state.library);
        let before = library
            .playback
            .get(&episode_id)
            .map(|progress| progress.is_finished())
            .unwrap_or(false);
        library.record_play(&episode_id, position_secs, duration_secs, completed);
        let _ = library.save(&state.library_path());
        before
    };

    let finished_now = lock_library(&state.library)
        .playback
        .get(&episode_id)
        .map(|progress| progress.is_finished())
        .unwrap_or(false);

    if let Some(event) = playback_event(&state, &episode_id, position_secs) {
        if finished_now && !finished_before {
            broadcast_playback(&state, methods::EVENT_PLAYBACK_COMPLETED, &event);
        } else {
            broadcast_playback(&state, methods::EVENT_PLAYBACK_PROGRESS, &event);
        }
    }
}

/// Restart a plugin in place. Used from the plugin manager and by hot reload.
#[tauri::command]
pub fn plugin_reload(plugin_index: usize, state: State<'_, AppState>) -> Result<(), String> {
    lock_plugins(&state)
        .reload(plugin_index)
        .map_err(|error| error.message)
}

/// Begin an interactive window drag. The frontend calls this from the titlebar
/// so dragging does not depend on the injected drag-region script.
#[tauri::command]
pub fn app_start_drag(window: tauri::Window) {
    let _ = window.start_dragging();
}

/// Double-click on the titlebar chrome behaves like a native one.
#[tauri::command]
pub fn app_toggle_maximise(window: tauri::Window) {
    if window.is_maximized().unwrap_or(false) {
        let _ = window.unmaximize();
    } else {
        let _ = window.maximize();
    }
}

/// Where a user drops a plugin directory to install it. Shown in the UI.
#[tauri::command]
pub fn plugins_directory(state: State<'_, AppState>) -> String {
    state.plugins_dir.to_string_lossy().into_owned()
}

/// Poddies has no taskbar-minimised state: minimising *is* going to the tray.
#[tauri::command]
pub fn app_hide(window: tauri::Window) {
    let _ = window.hide();
}

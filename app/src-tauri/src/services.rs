//! Library-backed services exposed to plugins.
//!
//! A plugin can read the library through `host/library/shows` and
//! `host/library/history`. Only the aggregate shapes in `poddies-plugin-api`
//! cross the boundary, so a plugin never sees internal storage.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::Value;

use poddies_core::library::Library;
use poddies_core::util::normalize_feed_url;
use poddies_plugin_api::{HistoryEntry, ShowSummary};
use poddies_plugin_host::HostServices;

/// The library, shared between the Tauri commands and every plugin worker.
pub struct SharedLibrary(pub Mutex<Library>);

pub fn lock_library(library: &SharedLibrary) -> MutexGuard<'_, Library> {
    library
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone)]
pub struct LibraryServices {
    pub library: Arc<SharedLibrary>,
}

impl HostServices for LibraryServices {
    fn library_shows(&self) -> Value {
        let library = lock_library(&self.library);

        let mut counts: HashMap<&str, usize> = HashMap::new();
        for episode in library.episodes.values() {
            *counts.entry(episode.show_id.as_str()).or_default() += 1;
        }

        let mut summaries: Vec<ShowSummary> = library
            .shows
            .values()
            .map(|show| ShowSummary {
                id: show.id.clone(),
                title: show.title.clone(),
                author: show.author.clone(),
                categories: show.categories.clone(),
                subscribed: show.subscribed,
                episode_count: counts.get(show.id.as_str()).copied().unwrap_or(0),
            })
            .collect();

        summaries.sort_by(|a, b| a.title.to_ascii_lowercase().cmp(&b.title.to_ascii_lowercase()));
        serde_json::to_value(summaries).unwrap_or_else(|_| Value::Array(Vec::new()))
    }

    fn library_history(&self) -> Value {
        let library = lock_library(&self.library);

        let entries: Vec<HistoryEntry> = library
            .history()
            .into_iter()
            .map(|(episode, state)| HistoryEntry {
                episode_id: episode.id.clone(),
                show_id: episode.show_id.clone(),
                title: episode.title.clone(),
                show_title: library
                    .show(&episode.show_id)
                    .map(|show| show.title.clone())
                    .unwrap_or_else(|| "Unknown show".to_string()),
                position_secs: state.position_secs,
                duration_secs: state.duration_secs.or(episode.duration_secs),
                completed: state.is_finished(),
                last_played: state.last_played,
            })
            .collect();

        serde_json::to_value(entries).unwrap_or_else(|_| Value::Array(Vec::new()))
    }

    fn log(&self, level: &str, message: &str) {
        eprintln!("[plugin {level}] {message}");
    }
}

/// True when this feed URL already belongs to a subscription.
pub fn is_subscribed(library: &Library, feed_url: &str) -> bool {
    let normalized = normalize_feed_url(feed_url);
    library
        .shows
        .values()
        .filter(|show| show.subscribed)
        .any(|show| normalize_feed_url(&show.feed_url) == normalized)
}

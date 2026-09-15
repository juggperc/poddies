//! The library: subscriptions, episodes, playback progress and preferences.
//!
//! Persisted as a single JSON document. For a personal podcast library (tens
//! of shows, thousands of episodes) that stays well under a megabyte and makes
//! the format inspectable — which fits the transparency rule the app follows
//! everywhere else.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::discovery::Weights;
use crate::error::Result;
use crate::feed::ParsedFeed;
use crate::model::{Episode, EpisodeId, PlaybackState, Show, ShowId};

/// User preferences. Discovery weights live here so the ranking stays fully
/// user-adjustable and is persisted with the library.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub discovery: Weights,
    pub avoid_explicit: bool,
    /// Plugin ids the user has switched off. Kept with the library so the
    /// choice survives a restart.
    #[serde(default)]
    pub disabled_plugins: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Library {
    pub shows: BTreeMap<ShowId, Show>,
    pub episodes: BTreeMap<EpisodeId, Episode>,
    pub playback: BTreeMap<EpisodeId, PlaybackState>,
    #[serde(default)]
    pub settings: Settings,
}

/// What changed during a merge, so the UI can report "3 new episodes".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MergeStats {
    pub new_episodes: usize,
    pub total_episodes: usize,
}

impl Library {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load from disk. A missing or corrupt file yields an empty library rather
    /// than an error: the app should always start.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Write atomically: temp file first, then rename over the target.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&temp, path)?;
        Ok(())
    }

    /// Add a show as a subscription and merge its episodes.
    pub fn subscribe(&mut self, parsed: ParsedFeed) -> MergeStats {
        self.merge(parsed, true)
    }

    /// Merge a fetched feed, preserving the existing subscription flag.
    pub fn refresh(&mut self, parsed: ParsedFeed) -> MergeStats {
        self.merge(parsed, false)
    }

    fn merge(&mut self, parsed: ParsedFeed, subscribe: bool) -> MergeStats {
        let mut stats = MergeStats::default();
        let show_id = parsed.show.id.clone();

        let already_subscribed = self
            .shows
            .get(&show_id)
            .map(|show| show.subscribed)
            .unwrap_or(false);

        let mut show = parsed.show;
        show.subscribed = subscribe || already_subscribed;
        show.last_refreshed = Some(Utc::now());
        self.shows.insert(show_id, show);

        for episode in parsed.episodes {
            if !self.episodes.contains_key(&episode.id) {
                stats.new_episodes += 1;
            }
            self.episodes.insert(episode.id.clone(), episode);
        }
        stats.total_episodes = self.episodes.len();
        stats
    }

    pub fn unsubscribe(&mut self, show_id: &str) {
        if let Some(show) = self.shows.get_mut(show_id) {
            show.subscribed = false;
        }
    }

    /// Forget a show entirely, including its episodes and their progress.
    pub fn remove_show(&mut self, show_id: &str) {
        self.shows.remove(show_id);
        let doomed: Vec<EpisodeId> = self
            .episodes
            .values()
            .filter(|episode| episode.show_id == show_id)
            .map(|episode| episode.id.clone())
            .collect();
        for id in doomed {
            self.episodes.remove(&id);
            self.playback.remove(&id);
        }
    }

    pub fn set_feed_meta(&mut self, show_id: &str, etag: Option<String>, last_modified: Option<String>) {
        if let Some(show) = self.shows.get_mut(show_id) {
            show.etag = etag;
            show.last_modified = last_modified;
        }
    }

    pub fn show(&self, show_id: &str) -> Option<&Show> {
        self.shows.get(show_id)
    }

    /// Subscribed shows, alphabetical.
    pub fn subscriptions(&self) -> Vec<&Show> {
        let mut shows: Vec<&Show> = self.shows.values().filter(|show| show.subscribed).collect();
        shows.sort_by_key(|show| show.title.to_ascii_lowercase());
        shows
    }

    /// Episodes of subscribed shows, newest first.
    pub fn latest_episodes(&self, limit: usize) -> Vec<&Episode> {
        let mut episodes: Vec<&Episode> = self
            .episodes
            .values()
            .filter(|episode| {
                self.shows
                    .get(&episode.show_id)
                    .map(|show| show.subscribed)
                    .unwrap_or(false)
            })
            .collect();
        episodes.sort_by(|a, b| b.published.cmp(&a.published));
        episodes.truncate(limit);
        episodes
    }

    /// Episodes for one show, newest first.
    pub fn episodes_for_show(&self, show_id: &str) -> Vec<&Episode> {
        let mut episodes: Vec<&Episode> = self
            .episodes
            .values()
            .filter(|episode| episode.show_id == show_id)
            .collect();
        episodes.sort_by(|a, b| b.published.cmp(&a.published));
        episodes
    }

    /// Started-but-unfinished episodes, most recently touched first.
    pub fn in_progress(&self, limit: usize) -> Vec<(&Episode, &PlaybackState)> {
        let mut items: Vec<(&Episode, &PlaybackState)> = self
            .playback
            .values()
            .filter(|state| state.position_secs > 0.0 && !state.is_finished())
            .filter_map(|state| {
                self.episodes
                    .get(&state.episode_id)
                    .map(|episode| (episode, state))
            })
            .collect();
        items.sort_by(|a, b| b.1.last_played.cmp(&a.1.last_played));
        items.truncate(limit);
        items
    }

    /// Everything the listener has touched, most recent first. This is the
    /// input to the discovery profile.
    pub fn history(&self) -> Vec<(&Episode, &PlaybackState)> {
        let mut items: Vec<(&Episode, &PlaybackState)> = self
            .playback
            .values()
            .filter(|state| state.position_secs > 0.0)
            .filter_map(|state| {
                self.episodes
                    .get(&state.episode_id)
                    .map(|episode| (episode, state))
            })
            .collect();
        items.sort_by(|a, b| b.1.last_played.cmp(&a.1.last_played));
        items
    }

    /// Record progress. Completion is sticky and counters only increment when
    /// an episode crosses the finish line for the first time.
    pub fn record_play(
        &mut self,
        episode_id: &str,
        position_secs: f64,
        duration_secs: Option<u64>,
        completed: bool,
    ) {
        let entry = self
            .playback
            .entry(episode_id.to_string())
            .or_insert_with(|| PlaybackState {
                episode_id: episode_id.to_string(),
                ..Default::default()
            });

        entry.position_secs = position_secs.max(0.0);
        if duration_secs.is_some() {
            entry.duration_secs = duration_secs;
        }
        let finished_before = entry.completed;
        entry.completed = entry.completed || completed;
        if entry.completed && !finished_before {
            entry.play_count += 1;
        }
        entry.last_played = Some(Utc::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Episode, PlaybackState, Show};

    fn show(id: &str, title: &str, subscribed: bool) -> Show {
        Show {
            id: id.to_string(),
            feed_url: format!("https://example.com/{id}.xml"),
            title: title.to_string(),
            author: None,
            description: None,
            image_url: None,
            link: None,
            language: None,
            categories: vec!["Technology".to_string()],
            explicit: false,
            published: None,
            updated: None,
            subscribed,
            last_refreshed: None,
            etag: None,
            last_modified: None,
        }
    }

    fn episode(id: &str, show_id: &str) -> Episode {
        Episode {
            id: id.to_string(),
            show_id: show_id.to_string(),
            title: format!("Episode {id}"),
            description: None,
            enclosure_url: format!("https://example.com/{id}.mp3"),
            duration_secs: Some(1000),
            published: None,
            image_url: None,
            guid: id.to_string(),
        }
    }

    #[test]
    fn record_play_counts_completions_once() {
        let mut library = Library::new();
        library.episodes.insert("ep_1".into(), episode("ep_1", "sh_1"));

        library.record_play("ep_1", 500.0, Some(1000), false);
        assert_eq!(library.playback["ep_1"].play_count, 0);

        library.record_play("ep_1", 1000.0, Some(1000), true);
        assert_eq!(library.playback["ep_1"].play_count, 1);

        library.record_play("ep_1", 1000.0, Some(1000), true);
        assert_eq!(library.playback["ep_1"].play_count, 1);
    }

    #[test]
    fn latest_episodes_only_includes_subscriptions() {
        let mut library = Library::new();
        library.shows.insert("sh_1".into(), show("sh_1", "Alpha", true));
        library.shows.insert("sh_2".into(), show("sh_2", "Beta", false));
        library.episodes.insert("ep_1".into(), episode("ep_1", "sh_1"));
        library.episodes.insert("ep_2".into(), episode("ep_2", "sh_2"));

        let latest = library.latest_episodes(10);
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].id, "ep_1");
    }

    #[test]
    fn remove_show_cascades() {
        let mut library = Library::new();
        library.shows.insert("sh_1".into(), show("sh_1", "Alpha", true));
        library.episodes.insert("ep_1".into(), episode("ep_1", "sh_1"));
        library.playback.insert(
            "ep_1".into(),
            PlaybackState {
                episode_id: "ep_1".into(),
                position_secs: 10.0,
                ..Default::default()
            },
        );

        library.remove_show("sh_1");
        assert!(library.shows.is_empty());
        assert!(library.episodes.is_empty());
        assert!(library.playback.is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join("poddies_library_test");
        let path = dir.join("library.json");
        let _ = std::fs::remove_file(&path);

        let mut library = Library::new();
        library.shows.insert("sh_1".into(), show("sh_1", "Alpha", true));
        library.save(&path).unwrap();

        let reloaded = Library::load(&path);
        assert_eq!(reloaded.shows.len(), 1);
        assert_eq!(reloaded.shows["sh_1"].title, "Alpha");

        let _ = std::fs::remove_file(&path);
    }
}

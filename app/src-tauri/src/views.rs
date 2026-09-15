//! Serializable shapes handed to the frontend.
//!
//! The UI never sees the storage model directly: it gets view types that are
//! already joined (episode + show + progress), so rendering stays dumb and
//! fast.

use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ShowView {
    pub id: String,
    pub title: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub categories: Vec<String>,
    pub explicit: bool,
    pub episode_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct EpisodeView {
    pub id: String,
    pub show_id: String,
    pub show_title: String,
    pub title: String,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub enclosure_url: String,
    pub duration_secs: Option<u64>,
    pub published: Option<String>,
    pub position_secs: f64,
    pub completed: bool,
    pub play_count: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct LibraryView {
    pub shows: Vec<ShowView>,
    pub latest: Vec<EpisodeView>,
    pub in_progress: Vec<EpisodeView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReasonView {
    pub label: String,
    pub weight: f64,
    pub contribution: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveryItemView {
    pub title: String,
    pub feed_url: String,
    pub image_url: Option<String>,
    pub author: Option<String>,
    pub categories: Vec<String>,
    pub source: String,
    pub explicit: bool,
    pub subscribed: bool,
    pub score: f64,
    pub reasons: Vec<ReasonView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PanelView {
    pub plugin_index: usize,
    pub plugin_id: String,
    pub plugin_name: String,
    pub panel_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchView {
    pub shows: Vec<ShowView>,
    pub episodes: Vec<EpisodeView>,
    /// Apple Podcasts results, so a query can find shows not yet in the library.
    #[serde(default)]
    pub web: Vec<DiscoveryItemView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginStatusView {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Whether the plugin is running. A failure entry is not running.
    pub ok: bool,
    /// Why the plugin is stopped, when it is.
    pub detail: String,
    /// Position in the host's plugin list, so the interface can reload it.
    /// Absent for a plugin that failed to load, since there is nothing to reload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RefreshSummary {
    pub checked: usize,
    pub updated: usize,
    pub new_episodes: usize,
    pub failed: Vec<String>,
}

pub fn rfc3339(value: Option<DateTime<Utc>>) -> Option<String> {
    value.map(|time| time.to_rfc3339())
}

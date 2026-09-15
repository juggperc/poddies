//! Typed shapes for the library data a plugin may read.
//!
//! These live in the API crate (rather than being ad-hoc JSON) so the host that
//! produces them and the plugins that consume them cannot drift apart.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A show as seen by a plugin: enough for stats and recommendations, without
/// exposing the whole library model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShowSummary {
    pub id: String,
    pub title: String,
    pub author: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    pub subscribed: bool,
    pub episode_count: usize,
}

/// One entry of listening history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub episode_id: String,
    pub show_id: String,
    pub title: String,
    pub show_title: String,
    pub position_secs: f64,
    pub duration_secs: Option<u64>,
    pub completed: bool,
    pub last_played: Option<DateTime<Utc>>,
}

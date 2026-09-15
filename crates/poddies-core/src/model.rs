//! The domain model. Everything the app persists or shows is expressed here.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::util::{normalize_feed_url, stable_id};

pub type ShowId = String;
pub type EpisodeId = String;

/// A podcast. `subscribed` is the only field the user controls directly; the
/// rest is refreshed from the feed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Show {
    pub id: ShowId,
    pub feed_url: String,
    pub title: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub link: Option<String>,
    pub language: Option<String>,
    pub categories: Vec<String>,
    pub explicit: bool,
    pub published: Option<DateTime<Utc>>,
    pub updated: Option<DateTime<Utc>>,
    pub subscribed: bool,
    pub last_refreshed: Option<DateTime<Utc>>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// A single playable item. Episodes without an audio enclosure are dropped at
/// parse time, so `enclosure_url` is always present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    pub id: EpisodeId,
    pub show_id: ShowId,
    pub title: String,
    pub description: Option<String>,
    pub enclosure_url: String,
    pub duration_secs: Option<u64>,
    pub published: Option<DateTime<Utc>>,
    pub image_url: Option<String>,
    pub guid: String,
}

/// Where the listener is in an episode, and how much of it they consumed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PlaybackState {
    pub episode_id: EpisodeId,
    pub position_secs: f64,
    pub duration_secs: Option<u64>,
    pub completed: bool,
    pub last_played: Option<DateTime<Utc>>,
    pub play_count: u32,
}

impl PlaybackState {
    /// Fraction of the episode consumed, in `0.0..=1.0`. Returns `None` when
    /// the duration is unknown.
    pub fn completion(&self) -> Option<f64> {
        let duration = self.duration_secs?;
        if duration == 0 {
            return None;
        }
        Some((self.position_secs / duration as f64).clamp(0.0, 1.0))
    }

    /// An episode counts as finished once the listener is within the last 30
    /// seconds, matching how podcast apps treat trailing credits.
    pub fn is_finished(&self) -> bool {
        if self.completed {
            return true;
        }
        match (self.duration_secs, self.position_secs) {
            (Some(duration), position) if duration > 0 => {
                position >= (duration as f64 - 30.0).max(0.0)
            }
            _ => false,
        }
    }
}

/// Stable show id derived from the canonical feed URL.
pub fn show_id_for_url(feed_url: &str) -> ShowId {
    stable_id("sh_", &[&normalize_feed_url(feed_url)])
}

/// Stable episode id derived from the owning show plus the feed's guid.
pub fn episode_id_for(show_id: &str, guid: &str) -> EpisodeId {
    stable_id("ep_", &[show_id, guid])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_handles_missing_duration() {
        let state = PlaybackState {
            position_secs: 30.0,
            duration_secs: None,
            ..Default::default()
        };
        assert_eq!(state.completion(), None);
        assert!(!state.is_finished());
    }

    #[test]
    fn completion_is_clamped() {
        let state = PlaybackState {
            position_secs: 500.0,
            duration_secs: Some(100),
            ..Default::default()
        };
        assert_eq!(state.completion(), Some(1.0));
        assert!(state.is_finished());
    }

    #[test]
    fn trailing_credits_count_as_finished() {
        let state = PlaybackState {
            position_secs: 290.0,
            duration_secs: Some(300),
            ..Default::default()
        };
        assert!(state.is_finished());
    }

    #[test]
    fn show_ids_follow_url_normalisation() {
        assert_eq!(
            show_id_for_url("HTTPS://Example.com/feed/"),
            show_id_for_url("https://example.com/feed")
        );
    }
}

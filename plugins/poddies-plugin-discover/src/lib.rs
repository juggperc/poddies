//! Reference plugin: a discovery source.
//!
//! Shows how a plugin feeds the Discovery queue. It takes the topic hints the
//! host derives from the listener's history, queries the public iTunes Search
//! API for matching podcasts, and returns plain candidates. It does **not**
//! score them: ranking belongs to the host, where the user can see and adjust
//! it. Results are cached, and a curated fallback keeps the queue non-empty
//! when the network is unavailable.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use poddies_plugin_sdk::api::DiscoveryCandidate;
use poddies_plugin_sdk::{export_plugin, json, Plugin, PluginError, PluginInfo, PROTOCOL_VERSION};
use serde::Deserialize;
use serde_json::Value;

const PLUGIN_ID: &str = "dev.poddies.discover";
const CACHE_TTL: Duration = Duration::from_secs(600);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_TOPICS: usize = 3;
const DEFAULT_TOPICS: [&str; 3] = ["Technology", "News", "Science"];

/// Real feeds used when the search API is unreachable, so Discovery still
/// explains itself offline.
const CURATED: [(&str, &str, &str); 10] = [
    ("This American Life", "https://www.thisamericanlife.org/podcast/rss", "Society & Culture"),
    ("Planet Money", "https://feeds.npr.org/510289/podcast.xml", "Business"),
    ("Radiolab", "https://feeds.wnyc.org/radiolab", "Science"),
    ("The Changelog", "https://changelog.com/podcast/feed", "Technology"),
    ("Syntax", "https://feed.syntax.fm/rss", "Technology"),
    ("Darknet Diaries", "https://feeds.megaphone.fm/darknetdiaries", "Technology"),
    ("99% Invisible", "https://feeds.simplecast.com/BqbsxVfO", "Design"),
    ("Software Engineering Daily", "https://softwareengineeringdaily.com/feed/podcast/", "Technology"),
    ("The Talk Show", "https://daringfireball.net/thetalkshow/rss", "Technology"),
    ("NPR News Now", "https://feeds.npr.org/500005/podcast.xml", "News"),
];

#[derive(Default)]
pub struct Discover {
    cache: Mutex<BTreeMap<String, CacheEntry>>,
}

struct CacheEntry {
    fetched_at: Instant,
    candidates: Vec<DiscoveryCandidate>,
}

impl Plugin for Discover {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: PLUGIN_ID.to_string(),
            name: "Discover".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: PROTOCOL_VERSION.to_string(),
            ui_panels: Vec::new(),
        }
    }

    fn on_request(&mut self, method: &str, params: Value) -> Result<Value, PluginError> {
        match method {
            "discovery/list" => {
                let limit = params
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(20) as usize;
                let topics = requested_topics(&params);
                Ok(json!({ "candidates": self.collect(&topics, limit) }))
            }
            other => Err(PluginError::unsupported(other)),
        }
    }
}

impl Discover {
    fn collect(&self, topics: &[String], limit: usize) -> Vec<DiscoveryCandidate> {
        let mut candidates = Vec::new();
        let mut seen = BTreeSet::new();

        for topic in topics.iter().take(MAX_TOPICS) {
            for candidate in self.search_cached(topic) {
                if seen.insert(candidate.feed_url.clone()) {
                    candidates.push(candidate);
                }
            }
        }

        if candidates.is_empty() {
            for candidate in curated() {
                if seen.insert(candidate.feed_url.clone()) {
                    candidates.push(candidate);
                }
            }
        }

        candidates.truncate(limit);
        candidates
    }

    /// Cached network search. A failed fetch is not cached, so the next call
    /// retries rather than pinning an outage.
    fn search_cached(&self, topic: &str) -> Vec<DiscoveryCandidate> {
        let key = topic.to_ascii_lowercase();

        if let Some(entry) = self.lock_cache().get(&key) {
            if entry.fetched_at.elapsed() < CACHE_TTL {
                return entry.candidates.clone();
            }
        }

        let fetched = search(topic).unwrap_or_default();
        if !fetched.is_empty() {
            self.lock_cache().insert(
                key,
                CacheEntry {
                    fetched_at: Instant::now(),
                    candidates: fetched.clone(),
                },
            );
        }
        fetched
    }

    fn lock_cache(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, CacheEntry>> {
        self.cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<SearchResult>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResult {
    collection_name: Option<String>,
    feed_url: Option<String>,
    artwork_url600: Option<String>,
    primary_genre_name: Option<String>,
    release_date: Option<String>,
    collection_explicitness: Option<String>,
}

fn search(topic: &str) -> Option<Vec<DiscoveryCandidate>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("Poddies-Discover/", env!("CARGO_PKG_VERSION")))
        .timeout(REQUEST_TIMEOUT)
        .build()
        .ok()?;

    let response = client
        .get("https://itunes.apple.com/search")
        .query(&[("media", "podcast"), ("limit", "12"), ("term", topic)])
        .send()
        .ok()?;

    if !response.status().is_success() {
        return None;
    }

    let body: SearchResponse = response.json().ok()?;
    Some(body.results.into_iter().filter_map(convert).collect())
}

fn convert(result: SearchResult) -> Option<DiscoveryCandidate> {
    Some(DiscoveryCandidate {
        title: result.collection_name?,
        feed_url: result.feed_url?,
        description: None,
        image_url: result.artwork_url600,
        author: None,
        categories: result.primary_genre_name.into_iter().collect(),
        latest_published: result.release_date.as_deref().and_then(parse_date),
        typical_duration_secs: None,
        explicit: result.collection_explicitness.as_deref() == Some("explicit"),
        popularity: None,
        source: PLUGIN_ID.to_string(),
    })
}

fn parse_date(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

fn requested_topics(params: &Value) -> Vec<String> {
    let topics: Vec<String> = params
        .get("topics")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    if topics.is_empty() {
        DEFAULT_TOPICS.iter().map(|topic| topic.to_string()).collect()
    } else {
        topics
    }
}

fn curated() -> Vec<DiscoveryCandidate> {
    CURATED
        .iter()
        .map(|(title, feed_url, category)| DiscoveryCandidate {
            title: (*title).to_string(),
            feed_url: (*feed_url).to_string(),
            description: None,
            image_url: None,
            author: None,
            categories: vec![(*category).to_string()],
            latest_published: None,
            typical_duration_secs: None,
            explicit: false,
            popularity: None,
            source: PLUGIN_ID.to_string(),
        })
        .collect()
}

export_plugin!(Discover);

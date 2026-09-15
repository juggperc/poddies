//! Apple Podcasts search over the public iTunes Search API.
//!
//! Unauthenticated: `GET https://itunes.apple.com/search?media=podcast&term=…`.
//! No key, no account. Two consumers share this lookup: in-app search, and the
//! Apple Podcasts discovery plugin. Neither scores anything here — Discovery
//! ranking stays in the host, plugin-fed and ranked by the user's own history.
//!
//! `parse` is split from the network call so the response shape can be tested
//! without touching the network.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::discovery::DiscoveryCandidate;
use crate::error::{CoreError, Result};

pub const ENDPOINT: &str = "https://itunes.apple.com/search";

/// The app shows this on results, next to plugin ids like `dev.poddies.apple-podcasts`.
pub const SOURCE: &str = "apple podcasts";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
const USER_AGENT: &str = concat!("Poddies/", env!("CARGO_PKG_VERSION"));
/// The API accepts 1..=200, so this is the widest a `popularity` can be.
const POPULARITY_CEILING: f64 = 200.0;

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<SearchResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResult {
    collection_name: Option<String>,
    feed_url: Option<String>,
    artist_name: Option<String>,
    artwork_url_600: Option<String>,
    primary_genre_name: Option<String>,
    release_date: Option<String>,
    collection_explicitness: Option<String>,
    track_count: Option<u64>,
}

/// Query Apple Podcasts for shows matching `term`.
pub fn search(term: &str, limit: usize) -> Result<Vec<DiscoveryCandidate>> {
    let term = term.trim();
    if term.is_empty() {
        return Ok(Vec::new());
    }

    let mut endpoint = url::Url::parse(ENDPOINT)?;
    {
        let mut query = endpoint.query_pairs_mut();
        query.append_pair("media", "podcast");
        query.append_pair("term", term);
        query.append_pair("limit", &limit.to_string());
    }

    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(Duration::from_secs(3))
        .build()?;

    let response = client.get(endpoint).send()?;
    let status = response.status();
    if !status.is_success() {
        return Err(CoreError::Search(format!(
            "Apple Podcasts answered {status}"
        )));
    }

    // `text` + serde_json rather than `response.json`, which would drag in
    // reqwest's gated `json` feature for one call.
    let body: SearchResponse =
        serde_json::from_str(&response.text()?).map_err(|error| CoreError::Search(error.to_string()))?;

    Ok(parse_candidates(&body))
}

fn parse_candidates(response: &SearchResponse) -> Vec<DiscoveryCandidate> {
    response
        .results
        .iter()
        .filter_map(result_into_candidate)
        .collect()
}

fn result_into_candidate(result: &SearchResult) -> Option<DiscoveryCandidate> {
    Some(DiscoveryCandidate {
        title: result.collection_name.clone()?,
        feed_url: result.feed_url.clone()?,
        description: None,
        image_url: result.artwork_url_600.clone(),
        author: result.artist_name.clone(),
        categories: result.primary_genre_name.clone().into_iter().collect(),
        latest_published: result.release_date.as_deref().and_then(parse_rfc3339),
        // The API does not report a show's typical episode length. Guessing
        // would poison the duration-fit factor, so it stays absent and that
        // weight simply falls back to neutral.
        typical_duration_secs: None,
        explicit: result.collection_explicitness.as_deref() == Some("explicit"),
        popularity: result.track_count.map(|count| count as f64 / POPULARITY_CEILING),
        source: SOURCE.to_string(),
    })
}

fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn candidates(json: &serde_json::Value) -> Vec<DiscoveryCandidate> {
        let response: std::result::Result<SearchResponse, _> =
            serde_json::from_value(json.clone());
        match response {
            Ok(parsed) => parse_candidates(&parsed),
            Err(_) => Vec::new(),
        }
    }

    #[test]
    fn parses_the_public_response() {
        let candidates = candidates(&json!({
            "resultCount": 3,
            "results": [
                {
                    "collectionName": "Syntax",
                    "feedUrl": "https://feed.syntax.fm/rss",
                    "artistName": "Wes Bos",
                    "artworkUrl600": "https://example.com/img.png",
                    "primaryGenreName": "Technology",
                    "releaseDate": "2026-09-01T00:00:00Z",
                    "collectionExplicitness": "notExplicit",
                    "trackCount": 1038
                },
                // No feed url: nothing to subscribe to, dropped.
                { "collectionName": "Ghost feed" },
                // The wrong media type: also no feed URL, dropped.
                { "collectionName": "A Song", "trackViewUrl": "https://music" }
            ]
        }));

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].title, "Syntax");
        assert_eq!(candidates[0].feed_url, "https://feed.syntax.fm/rss");
        assert_eq!(candidates[0].author.as_deref(), Some("Wes Bos"));
        assert_eq!(candidates[0].categories, vec!["Technology".to_string()]);
        assert_eq!(candidates[0].image_url.as_deref(), Some("https://example.com/img.png"));
        assert!(!candidates[0].explicit);
        assert_eq!(candidates[0].source, SOURCE);
    }

    #[test]
    fn popularity_is_a_fraction() {
        let candidates = candidates(&json!({
            "results": [{ "collectionName": "A", "feedUrl": "f", "trackCount": 100 }]
        }));
        assert_eq!(candidates[0].popularity, Some(0.5));
    }

    #[test]
    fn explicit_flag_is_honoured() {
        let candidates = candidates(&json!({
            "results": [
                { "collectionName": "A", "feedUrl": "a", "collectionExplicitness": "explicit" },
                { "collectionName": "B", "feedUrl": "b", "collectionExplicitness": "notExplicit" }
            ]
        }));
        assert!(candidates[0].explicit);
        assert!(!candidates[1].explicit);
    }

    #[test]
    fn malformed_response_yields_nothing() {
        assert!(candidates(&json!(42)).is_empty());
        assert!(candidates(&json!({ "results": 42 })).is_empty());
        assert!(candidates(&json!({})).is_empty());
    }

    #[test]
    fn blank_query_is_free() {
        assert!(search("   ", 12).unwrap().is_empty());
    }
}

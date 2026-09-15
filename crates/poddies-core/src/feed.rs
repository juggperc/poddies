//! Turns raw feed bytes into the domain model.
//!
//! Works for RSS 2.0, Atom, RSS 1.0 and JSON Feed because `feed-rs` normalises
//! them all. Podcast specifics (iTunes duration / artwork / explicit flag,
//! MediaRSS thumbnails) arrive via that normalisation, so we never parse XML
//! ourselves.

use std::collections::BTreeSet;

use feed_rs::model as fr;

use crate::error::{CoreError, Result};
use crate::model::{episode_id_for, Episode, Show};
use crate::util::{html_to_text, normalize_feed_url, stable_id, title_case, truncate_text};

/// A parsed feed: one show plus its episodes, ready to merge into the library.
#[derive(Debug, Clone)]
pub struct ParsedFeed {
    pub show: Show,
    pub episodes: Vec<Episode>,
}

const MAX_DESCRIPTION_CHARS: usize = 1_200;

/// Parse feed bytes. `feed_url` is the URL the bytes came from and is what the
/// show id is derived from, so the same feed always maps to the same show.
pub fn parse_feed(bytes: &[u8], feed_url: &str) -> Result<ParsedFeed> {
    let feed = feed_rs::parser::parse(bytes).map_err(|err| CoreError::Feed(err.to_string()))?;

    let canonical_url = normalize_feed_url(feed_url);
    let show_id = stable_id("sh_", &[&canonical_url]);

    let show = Show {
        id: show_id.clone(),
        feed_url: canonical_url,
        title: text_of(&feed.title).unwrap_or_else(|| "Untitled show".to_string()),
        author: show_author(&feed),
        description: feed
            .description
            .as_ref()
            .map(|text| truncate_text(&html_to_text(&text.content), MAX_DESCRIPTION_CHARS))
            .filter(|text| !text.is_empty()),
        image_url: feed
            .logo
            .as_ref()
            .or(feed.icon.as_ref())
            .map(|image| image.uri.clone()),
        link: primary_link(&feed),
        language: feed.language.clone(),
        categories: normalise_categories(&feed.categories),
        explicit: feed
            .rating
            .as_ref()
            .map(|rating| rating.value.eq_ignore_ascii_case("true"))
            .unwrap_or(false),
        published: feed.published,
        updated: feed.updated,
        subscribed: false,
        last_refreshed: None,
        etag: None,
        last_modified: None,
    };

    let episodes = feed
        .entries
        .iter()
        .filter_map(|entry| convert_episode(entry, &show))
        .collect();

    Ok(ParsedFeed { show, episodes })
}

fn convert_episode(entry: &fr::Entry, show: &Show) -> Option<Episode> {
    let enclosure_url = pick_enclosure(entry)?;

    let description = entry
        .summary
        .as_ref()
        .map(|text| text.content.clone())
        .or_else(|| entry.content.as_ref().and_then(|content| content.body.clone()))
        .map(|raw| truncate_text(&html_to_text(&raw), MAX_DESCRIPTION_CHARS))
        .filter(|text| !text.is_empty());

    Some(Episode {
        id: episode_id_for(&show.id, &entry.id),
        show_id: show.id.clone(),
        title: text_of(&entry.title).unwrap_or_else(|| "Untitled episode".to_string()),
        description,
        enclosure_url,
        duration_secs: entry
            .media
            .iter()
            .find_map(|media| media.duration)
            .map(|duration| duration.as_secs()),
        published: entry.published.or(entry.updated),
        image_url: entry
            .media
            .iter()
            .find_map(|media| media.thumbnails.first())
            .map(|thumb| thumb.image.uri.clone())
            .or_else(|| show.image_url.clone()),
        guid: entry.id.clone(),
    })
}

/// Prefer an audio-typed media content, then any media content, then an RSS
/// enclosure link, then anything that looks like an audio file.
fn pick_enclosure(entry: &fr::Entry) -> Option<String> {
    let mut fallback = None;
    for media in &entry.media {
        for content in &media.content {
            let Some(url) = &content.url else { continue };
            let url = url.to_string();
            let content_type = content
                .content_type
                .as_ref()
                .map(|mime| mime.to_string())
                .unwrap_or_default();
            if content_type.starts_with("audio") {
                return Some(url);
            }
            if fallback.is_none() {
                fallback = Some(url);
            }
        }
    }
    if fallback.is_some() {
        return fallback;
    }

    if let Some(link) = entry.links.iter().find(|link| link.rel.as_deref() == Some("enclosure")) {
        return Some(link.href.clone());
    }

    entry
        .links
        .iter()
        .map(|link| link.href.clone())
        .find(|href| {
            let href = href.to_ascii_lowercase();
            [".mp3", ".m4a", ".aac", ".ogg", ".opus", ".flac", ".wav"]
                .iter()
                .any(|ext| href.contains(ext))
        })
}

fn text_of(text: &Option<fr::Text>) -> Option<String> {
    text.as_ref()
        .map(|text| text.content.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn show_author(feed: &fr::Feed) -> Option<String> {
    feed.authors
        .first()
        .or_else(|| feed.contributors.first())
        .map(|person| person.name.trim().to_string())
        .filter(|name| !name.is_empty())
}

fn primary_link(feed: &fr::Feed) -> Option<String> {
    feed.links
        .iter()
        .find(|link| link.rel.as_deref() == Some("alternate"))
        .or_else(|| feed.links.first())
        .map(|link| link.href.clone())
}

fn normalise_categories(categories: &[fr::Category]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for category in categories {
        // iTunes nests sub-categories as "Parent/Child"; keep the leaf.
        let leaf = category
            .term
            .split('/')
            .next_back()
            .unwrap_or(&category.term)
            .trim();
        if leaf.is_empty() {
            continue;
        }
        let display = title_case(leaf);
        if seen.insert(display.to_ascii_lowercase()) {
            out.push(display);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">
  <channel>
    <title>Test Show</title>
    <link>https://example.com</link>
    <description>A &lt;b&gt;great&lt;/b&gt; show</description>
    <language>en-us</language>
    <itunes:author>Jane Doe</itunes:author>
    <itunes:image href="https://example.com/art.jpg"/>
    <itunes:category text="Technology"/>
    <itunes:explicit>false</itunes:explicit>
    <item>
      <title>Episode One</title>
      <guid>ep-1</guid>
      <pubDate>Tue, 01 Jan 2019 10:00:00 GMT</pubDate>
      <enclosure url="https://example.com/1.mp3" length="123" type="audio/mpeg"/>
      <itunes:duration>1:02:03</itunes:duration>
      <description>First &amp; best</description>
    </item>
    <item>
      <title>Not audio</title>
      <guid>ep-2</guid>
      <description>no enclosure here</description>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn parses_show_metadata() {
        let parsed = parse_feed(SAMPLE.as_bytes(), "https://example.com/feed.xml").unwrap();
        assert_eq!(parsed.show.title, "Test Show");
        assert_eq!(parsed.show.author.as_deref(), Some("Jane Doe"));
        assert_eq!(parsed.show.image_url.as_deref(), Some("https://example.com/art.jpg"));
        assert_eq!(parsed.show.language.as_deref(), Some("en-us"));
        assert_eq!(parsed.show.categories, vec!["Technology".to_string()]);
        assert!(!parsed.show.explicit);
        assert_eq!(parsed.show.description.as_deref(), Some("A great show"));
    }

    #[test]
    fn keeps_only_playable_episodes() {
        let parsed = parse_feed(SAMPLE.as_bytes(), "https://example.com/feed.xml").unwrap();
        assert_eq!(parsed.episodes.len(), 1);
        let episode = &parsed.episodes[0];
        assert_eq!(episode.title, "Episode One");
        assert_eq!(episode.enclosure_url, "https://example.com/1.mp3");
        assert_eq!(episode.duration_secs, Some(3723));
        assert_eq!(episode.description.as_deref(), Some("First & best"));
        assert_eq!(episode.show_id, parsed.show.id);
    }

    #[test]
    fn show_id_is_independent_of_feed_url_form() {
        let a = parse_feed(SAMPLE.as_bytes(), "HTTPS://Example.com/feed.xml").unwrap();
        let b = parse_feed(SAMPLE.as_bytes(), "https://example.com/feed.xml/").unwrap();
        assert_eq!(a.show.id, b.show.id);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_feed(b"not xml at all", "https://example.com/feed").is_err());
    }
}

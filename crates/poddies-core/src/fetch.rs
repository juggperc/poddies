//! Conditional feed fetching. Sends `If-None-Match` / `If-Modified-Since` so
//! refreshes of unchanged feeds cost nothing, and never blocks the UI thread
//! (callers own the threading).

use std::time::Duration;

use crate::error::Result;
use crate::feed::{parse_feed, ParsedFeed};

/// Outcome of a conditional fetch.
///
/// The parsed feed is boxed so the common `NotModified` path stays cheap to
/// move: a `ParsedFeed` is a few hundred bytes, and this value is returned and
/// matched on every refresh.
#[derive(Debug)]
pub enum Fetched {
    /// Server said 304: the cached copy is still current.
    NotModified,
    Updated {
        parsed: Box<ParsedFeed>,
        etag: Option<String>,
        last_modified: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct FeedClient {
    http: reqwest::blocking::Client,
}

impl FeedClient {
    pub fn new() -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .user_agent(concat!("Poddies/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self { http })
    }

    /// Fetch and parse a feed, honouring any validators the caller passes from
    /// the previous fetch.
    pub fn fetch(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<Fetched> {
        let mut request = self.http.get(url);
        if let Some(etag) = etag {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        if let Some(last_modified) = last_modified {
            request = request.header(reqwest::header::IF_MODIFIED_SINCE, last_modified);
        }

        let response = request.send()?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(Fetched::NotModified);
        }
        let response = response.error_for_status()?;

        let etag = header_string(&response, reqwest::header::ETAG);
        let last_modified = header_string(&response, reqwest::header::LAST_MODIFIED);
        let bytes = response.bytes()?;
        let parsed = parse_feed(&bytes, url)?;

        Ok(Fetched::Updated {
            parsed: Box::new(parsed),
            etag,
            last_modified,
        })
    }
}

fn header_string(response: &reqwest::blocking::Response, name: reqwest::header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

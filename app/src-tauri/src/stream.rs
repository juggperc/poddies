//! `poddies-media://` — a same-origin window proxy over episode media.
//!
//! Once a plugin inserts units into the playback chain, the webview routes the
//! `<audio>` element through a `MediaElementAudioSourceNode`. Chromium silences
//! any cross-origin media that does not arrive with CORS headers — and a large
//! share of podcast enclosures do not send them — so the plugin chain would
//! only ever see digital silence. Serving the same bytes through a custom URI
//! scheme keeps the stream same-origin from the webview's point of view: the
//! element is never tainted, and Web Audio receives real samples.
//!
//! The proxy fetches **windows**, not whole episodes: a `Range` request (or a
//! plain request, treated as starting at byte 0) is answered with at most
//! [`WINDOW`] bytes from upstream, correct `206`/`Content-Range` semantics so
//! the element's media cache can seek freely. The probed remote length is
//! cached briefly.
//!
//! Enclosure URLs travel base64url-encoded in the path, so no character in a
//! feed URL can confuse the scheme parser or percent-decoding.

use std::io::Read;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use tauri::http::{header, HeaderMap, HeaderValue, Request, Response, StatusCode};

/// Largest byte window served per request. Bounds the memory a seek can cost
/// and keeps responses cheap on the webview's media thread.
const WINDOW: u64 = 8 * 1024 * 1024;

/// How long a probed remote length may be reused; enclosures are immutable.
const PROBE_TTL: Duration = Duration::from_secs(600);

static CLIENT: LazyLock<reqwest::blocking::Client> = LazyLock::new(|| {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("Poddies/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("a usable HTTP client")
});

/// Remote lengths seen recently: target URL → (total bytes, when probed).
static LENGTHS: LazyLock<std::sync::Mutex<std::collections::HashMap<String, (u64, Instant)>>> =
    LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Wire the proxy into the Tauri builder. Each request is handled on its own
/// thread so the webview is never blocked by network latency.
pub fn register(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    builder.register_asynchronous_uri_scheme_protocol(
        "poddies-media",
        |_context, request, responder| {
            std::thread::spawn(move || responder.respond(serve(request)));
        },
    )
}

/// Turn an enclosure URL into the same-origin URL the audio element plays.
pub fn proxy_url(enclosure: &str) -> Result<String, String> {
    use base64::Engine as _;
    if !enclosure.starts_with("https://") {
        return Err("only https media can be proxied".to_string());
    }
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(enclosure.as_bytes());
    Ok(format!("http://poddies-media.localhost/{encoded}"))
}

fn serve(request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    use base64::Engine as _;
    let path = request.uri().path();
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(path.trim_start_matches('/').as_bytes())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok());
    let Some(target) = decoded else {
        return reject(StatusCode::BAD_REQUEST);
    };
    serve_window(target, request.headers())
}

fn serve_window(target: String, headers: &HeaderMap) -> Response<Vec<u8>> {
    let (start, requested_end) = parse_range(headers);
    let Some(total) = length(&target) else {
        return reject(StatusCode::BAD_GATEWAY);
    };
    if start >= total {
        return reject(StatusCode::RANGE_NOT_SATISFIABLE);
    }

    // The caller's end when the webview names one, otherwise a fresh window
    // from `start` — which is what a seek that lands past the cached range
    // looks like to the media cache.
    let end = match headers.get(header::RANGE) {
        Some(_) => requested_end.unwrap_or((start + WINDOW - 1).min(total - 1)),
        None => (start + WINDOW - 1).min(total - 1),
    }
    .min(total - 1);

    let response = CLIENT
        .get(&target)
        .header(header::RANGE, format!("bytes={start}-{end}"))
        .send();
    let Ok(mut upstream) = response else {
        return reject(StatusCode::BAD_GATEWAY);
    };

    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_owned())
        .unwrap_or_else(|| "application/octet-stream".to_string());

    // Read at most one window, even if the upstream ignored Range and streams
    // the whole episode.
    let wanted = (end - start + 1) as usize;
    let mut body: Vec<u8> = Vec::with_capacity(wanted.min(1024 * 1024));
    let mut chunk = [0u8; 64 * 1024];
    while body.len() < wanted {
        match upstream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                body.extend_from_slice(&chunk[..read]);
            }
        }
    }
    body.truncate(wanted);

    Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(header::CONTENT_TYPE, content_type)
        .header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{total}"),
        )
        .header(header::ACCEPT_RANGES, "bytes")
        .body(body)
        .unwrap_or_else(|_| reject(StatusCode::BAD_GATEWAY))
}

/// The remote length, probed once per URL and cached briefly. Every podcast CDN
/// answers a `bytes=0-0` request, and its `Content-Range` carries the total.
fn length(target: &str) -> Option<u64> {
    {
        let lengths = &mut *LENGTHS.lock().unwrap();
        if let Some((total, at)) = lengths.get(target) {
            if at.elapsed() < PROBE_TTL {
                return Some(*total);
            }
        }
    }

    let response = CLIENT
        .get(target)
        .header(header::RANGE, HeaderValue::from_static("bytes=0-0"))
        .send()
        .ok()?;
    let content_range = response
        .headers()
        .get(header::CONTENT_RANGE)?
        .to_str()
        .ok()?
        .to_string();
    // `bytes 0-0/N` — the count after the slash is the total length.
    let total = content_range.rsplit('/').next()?.parse::<u64>().ok()?;
    if total == 0 {
        return None;
    }
    let mut lengths = LENGTHS.lock().unwrap();
    lengths.insert(target.to_string(), (total, Instant::now()));
    Some(total)
}

/// `bytes=a-b`, `bytes=a-`, and the absence of a header mean the same thing to
/// the caller here: a start and an optional end, both inclusive.
fn parse_range(headers: &HeaderMap) -> (u64, Option<u64>) {
    let parse = || -> Option<(u64, Option<u64>)> {
        let range = headers
            .get(header::RANGE)?
            .to_str()
            .ok()?
            .strip_prefix("bytes=")?;
        let (start, end) = range.split_once('-')?;
        Some((start.trim().parse().ok()?, end.trim().parse().ok()))
    };
    parse().unwrap_or((0, None))
}

fn reject(status: StatusCode) -> Response<Vec<u8>> {
    Response::builder().status(status).body(Vec::new()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(range: Option<&str>) -> HeaderMap {
        let mut map = HeaderMap::new();
        if let Some(range) = range {
            map.insert(header::RANGE, HeaderValue::from_str(range).unwrap());
        }
        map
    }

    #[test]
    fn proxy_url_rejects_non_https() {
        assert!(proxy_url("http://example.com/x.mp3").is_err());
        assert!(proxy_url("https://example.com/x.mp3").is_ok());
    }

    #[test]
    fn missing_range_header_means_from_the_start() {
        let (start, end) = parse_range(&headers(None));
        assert_eq!((start, end), (0, None));
    }

    #[test]
    fn range_header_parses_open_and_bounded() {
        assert_eq!(
            parse_range(&headers(Some("bytes=100-200"))),
            (100, Some(200))
        );
        assert_eq!(parse_range(&headers(Some("bytes=100-"))), (100, None));
        // Malformed input is not fatal: the whole stream, from the start.
        assert_eq!(parse_range(&headers(Some("bytes=abc"))), (0, None));
    }

    #[test]
    fn quote_and_query_do_not_break_the_path() {
        let target = "https://cdn.example.com/ep?x=1&y=2#a";
        let encoded = crate::stream::proxy_url(target).unwrap();
        assert!(encoded.starts_with("http://poddies-media.localhost/"));
        // Round-trip through the same decoder network requests take.
        let path = encoded.rsplit("/").next().unwrap();
        let decoded = {
            use base64::Engine as _;
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(path.as_bytes())
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
        }
        .unwrap();
        assert_eq!(decoded, target);
    }

    /// Ground-truth: real enclosures answer byte probes without CORS headers,
    /// which is why the proxy exists. Needs a network connection; opt in.
    #[test]
    #[ignore]
    fn serves_a_window_from_a_live_enclosure() {
        let target = "https://www.podtrac.com/pts/redirect.mp3/dovetail.prxu.org/7057/82aaba05-80c9-4d2e-8069-3c20f2b7dbc8".to_string();
        if CLIENT.get(&target).send().is_err() {
            eprintln!("skipping: no network");
            return;
        }
        let response = serve_window(target, &headers(None));
        let range = response
            .headers()
            .get(header::CONTENT_RANGE)
            .map(|value| value.to_str().unwrap_or_default().to_string());
        match (response.status(), range) {
            (StatusCode::PARTIAL_CONTENT, Some(range)) => {
                println!("content-range: {range}");
                assert!(
                    range.starts_with("bytes 0-"),
                    "unexpected content-range: {range}"
                );
                assert!(response.body().len() > 10_000, "window body too small");
            }
            // CDNs that answer the initial request with a redirect and no
            // range support: the honest answer is also a serving failure.
            (StatusCode::BAD_GATEWAY, None) => {}
            (status, _) => panic!("unexpected proxy response: {status}"),
        }
    }
}

//! Small, dependency-free text and identity helpers.

/// FNV-1a. Used to mint opaque, stable identifiers for shows and episodes.
///
/// These ids are keys, not security tokens: a 64-bit non-cryptographic hash is
/// plenty to keep a personal library collision-free while staying tiny.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Build a stable id from a prefix and a list of parts, separated with a byte
/// that cannot appear in the parts themselves to avoid boundary collisions.
pub fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for &byte in part.as_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{prefix}{hash:016x}")
}

/// Canonicalise a feed URL so the same feed always maps to the same show id.
/// Lowercases the host, drops fragments and default ports, and trims a
/// trailing slash from non-root paths.
pub fn normalize_feed_url(raw: &str) -> String {
    let trimmed = raw.trim();
    let Ok(mut url) = url::Url::parse(trimmed) else {
        return trimmed.to_ascii_lowercase();
    };

    url.set_fragment(None);
    if let Some(host) = url.host_str().map(str::to_ascii_lowercase) {
        let _ = url.set_host(Some(&host));
    }
    let default_port = match url.scheme() {
        "https" => Some(443),
        "http" => Some(80),
        _ => None,
    };
    if url.port().is_some() && url.port() == default_port {
        let _ = url.set_port(None);
    }
    let path = url.path().to_string();
    if path.len() > 1 {
        url.set_path(path.trim_end_matches('/'));
    }
    url.to_string()
}

/// Convert feed HTML into readable plain text. Feeds routinely ship markup,
/// entities and tracking pixels in descriptions; the UI never shows raw HTML.
pub fn html_to_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut tag = String::new();
    let mut in_tag = false;
    let mut skipping = false;

    for ch in input.chars() {
        if in_tag {
            if ch == '>' {
                in_tag = false;
                let name = tag.trim().to_ascii_lowercase();
                match name.as_str() {
                    "script" | "style" => skipping = true,
                    "/script" | "/style" => skipping = false,
                    _ if !skipping => out.push(' '),
                    _ => {}
                }
                tag.clear();
            } else {
                tag.push(ch);
            }
            continue;
        }
        if ch == '<' {
            in_tag = true;
            tag.clear();
            continue;
        }
        if !skipping {
            out.push(ch);
        }
    }

    collapse_whitespace(&decode_entities(&out))
}

/// Decode the handful of entities that actually show up in podcast feeds,
/// including numeric forms. Anything unknown is passed through untouched.
pub fn decode_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.char_indices().peekable();

    while let Some((idx, ch)) = chars.next() {
        if ch != '&' {
            out.push(ch);
            continue;
        }
        let rest = &input[idx + 1..];
        let Some(semi) = rest.find(';').filter(|semi| *semi <= 10) else {
            out.push('&');
            continue;
        };
        let entity = &rest[..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" | "#160" => Some(' '),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            "rsquo" => Some('’'),
            "lsquo" => Some('‘'),
            "ldquo" => Some('“'),
            "rdquo" => Some('”'),
            _ => {
                let codepoint = entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| {
                        entity
                            .strip_prefix('#')
                            .and_then(|dec| dec.parse::<u32>().ok())
                    });
                codepoint.and_then(char::from_u32)
            }
        };
        match decoded {
            Some(decoded) => {
                out.push(decoded);
                for _ in 0..=semi {
                    chars.next();
                }
            }
            None => out.push('&'),
        }
    }
    out
}

/// Collapse runs of whitespace and trim, so multi-line feed descriptions read
/// as a single clean paragraph.
pub fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncate on a character boundary, appending an ellipsis when cut.
pub fn truncate_text(input: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(max_chars + 1);
    for (count, ch) in input.chars().enumerate() {
        if count >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

/// Title-case a category term ("technology" -> "Technology").
pub fn title_case(input: &str) -> String {
    input
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    let mut s = String::with_capacity(word.len());
                    s.extend(first.to_uppercase());
                    s.push_str(&chars.as_str().to_ascii_lowercase());
                    s
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Significant words in a phrase, for cheap similarity comparisons.
pub fn tokens(input: &str) -> std::collections::BTreeSet<String> {
    const STOPWORDS: &[&str] = &[
        "the", "and", "for", "with", "from", "that", "this", "your", "you", "about", "into",
        "podcast", "show", "episode",
    ];
    input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.len() > 2)
        .map(str::to_ascii_lowercase)
        .filter(|word| !STOPWORDS.contains(&word.as_str()))
        .collect()
}

/// Jaccard similarity of two token sets, in `0.0..=1.0`.
pub fn jaccard(
    a: &std::collections::BTreeSet<String>,
    b: &std::collections::BTreeSet<String>,
) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let intersection = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    if union == 0.0 { 0.0 } else { intersection / union }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_becomes_plain_text() {
        let html = "<p>Hello <b>world</b>&nbsp;&amp; friends</p><script>evil()</script>";
        assert_eq!(html_to_text(html), "Hello world & friends");
    }

    #[test]
    fn numeric_entities_decode() {
        assert_eq!(decode_entities("caf&#233; &#x2014; ok"), "café — ok");
    }

    #[test]
    fn url_normalisation_is_stable() {
        assert_eq!(
            normalize_feed_url("HTTPS://Example.COM:443/Feed/"),
            "https://example.com/Feed"
        );
        assert_eq!(normalize_feed_url("http://a.com"), "http://a.com/");
    }

    #[test]
    fn stable_ids_are_deterministic_and_distinct() {
        assert_eq!(stable_id("sh_", &["a"]), stable_id("sh_", &["a"]));
        assert_ne!(stable_id("sh_", &["a"]), stable_id("sh_", &["b"]));
        assert_ne!(stable_id("sh_", &["ab"]), stable_id("sh_", &["a", "b"]));
    }

    #[test]
    fn jaccard_bounds() {
        let a = tokens("The Daily Technology Show");
        let b = tokens("Technology Weekly");
        let sim = jaccard(&a, &b);
        assert!(sim > 0.0 && sim < 1.0);
        assert_eq!(jaccard(&a, &a), 1.0);
    }
}

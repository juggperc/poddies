//! The discovery ranking algorithm.
//!
//! This is deliberately *not* a black box. Every recommendation is a linear
//! combination of named factors whose weights the user can change, and the
//! result carries the full breakdown so the UI can show exactly why a show was
//! suggested. There is no model, no opaque embedding and no generated text:
//! each factor's label is derived directly from the data that produced it.
//!
//! ```text
//! score = Σ (weight_i * factor_i)  -  explicit_penalty  -  diversity_penalty
//! ```
//!
//! Diversity is applied greedily (maximal marginal relevance): candidates are
//! picked one at a time, each time discounting those too similar to what has
//! already been chosen. This keeps the queue varied instead of six near-copies
//! of the same topic.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::library::Library;
use crate::model::PlaybackState;
use crate::util::{jaccard, title_case, tokens};

/// User-adjustable weights, one per factor. All are non-negative; the two
/// penalties behave as `-weight * raw` automatically.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Weights {
    pub topic_affinity: f64,
    pub completion_signal: f64,
    pub recency: f64,
    pub novelty: f64,
    pub duration_fit: f64,
    pub explicit_penalty: f64,
    pub diversity: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            topic_affinity: 1.0,
            completion_signal: 0.5,
            recency: 0.35,
            novelty: 0.55,
            duration_fit: 0.25,
            explicit_penalty: 1.0,
            diversity: 0.4,
        }
    }
}

/// A show offered up by a discovery source (a plugin, or a built-in list)
/// before any of the user's history is taken into account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryCandidate {
    pub title: String,
    pub feed_url: String,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub author: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    pub latest_published: Option<DateTime<Utc>>,
    pub typical_duration_secs: Option<u64>,
    #[serde(default)]
    pub explicit: bool,
    /// Optional quality hint from the source. Used only as a deterministic
    /// tie-breaker — never as a weighted factor, so it cannot quietly dominate
    /// the user's own preferences.
    pub popularity: Option<f64>,
    pub source: String,
}

/// One term of the score, kept so the UI can explain a recommendation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FactorContribution {
    pub name: String,
    pub label: String,
    pub raw: f64,
    pub weight: f64,
    pub contribution: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoredCandidate {
    pub candidate: DiscoveryCandidate,
    pub score: f64,
    pub factors: Vec<FactorContribution>,
}

impl ScoredCandidate {
    /// The factors that actually moved the needle, strongest first.
    pub fn top_reasons(&self, count: usize) -> Vec<&FactorContribution> {
        let mut factors: Vec<&FactorContribution> = self
            .factors
            .iter()
            .filter(|factor| factor.contribution.abs() > 1e-6)
            .collect();
        factors.sort_by(|a, b| {
            b.contribution
                .abs()
                .partial_cmp(&a.contribution.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        factors.truncate(count);
        factors
    }
}

/// A summary of what the listener actually consumes, derived from history.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListeningProfile {
    /// Normalised interest per category (`0.0..=1.0`).
    pub category_weights: BTreeMap<String, f64>,
    pub preferred_duration_secs: Option<u64>,
    /// Mean completion ratio across listened episodes.
    pub avg_completion: f64,
    /// Tokenised titles of subscribed shows, used to judge novelty.
    pub subscribed_titles: Vec<BTreeSet<String>>,
    pub avoid_explicit: bool,
    pub listened_episodes: usize,
}

/// Build the listening profile from a library's history.
pub fn build_profile(library: &Library) -> ListeningProfile {
    let mut category_weights: BTreeMap<String, f64> = BTreeMap::new();
    let mut duration_total: u64 = 0;
    let mut duration_count: u64 = 0;
    let mut completion_total = 0.0;
    let mut listened_episodes = 0usize;

    for (episode, state) in library.history() {
        listened_episodes += 1;
        completion_total += completion_of(state);

        if let Some(show) = library.show(&episode.show_id) {
            // Heavily-finished shows push category interest up more.
            let weight = completion_of(state).max(0.05);
            for category in &show.categories {
                *category_weights
                    .entry(category.to_ascii_lowercase())
                    .or_insert(0.0) += weight;
            }
        }

        if let Some(duration) = episode.duration_secs.or(state.duration_secs) {
            duration_total += duration;
            duration_count += 1;
        }
    }

    let max = category_weights.values().cloned().fold(0.0_f64, f64::max);
    if max > 0.0 {
        for value in category_weights.values_mut() {
            *value /= max;
        }
    }

    ListeningProfile {
        category_weights,
        preferred_duration_secs: (duration_count > 0).then(|| duration_total / duration_count),
        avg_completion: if listened_episodes > 0 {
            completion_total / listened_episodes as f64
        } else {
            0.0
        },
        subscribed_titles: library
            .shows
            .values()
            .filter(|show| show.subscribed)
            .map(|show| tokens(&show.title))
            .collect(),
        avoid_explicit: library.settings.avoid_explicit,
        listened_episodes,
    }
}

/// Rank candidates for the user and return the best `limit`, best first.
pub fn rank(
    candidates: &[DiscoveryCandidate],
    profile: &ListeningProfile,
    weights: &Weights,
    limit: usize,
) -> Vec<ScoredCandidate> {
    if candidates.is_empty() || limit == 0 {
        return Vec::new();
    }

    let now = Utc::now();
    let category_sets: Vec<BTreeSet<String>> = candidates
        .iter()
        .map(|candidate| {
            candidate
                .categories
                .iter()
                .map(|category| category.to_ascii_lowercase())
                .collect()
        })
        .collect();

    let base_factors: Vec<Vec<FactorContribution>> = candidates
        .iter()
        .map(|candidate| score_candidate(candidate, profile, weights, now))
        .collect();

    let mut remaining: Vec<usize> = (0..candidates.len()).collect();
    let mut chosen: Vec<usize> = Vec::new();
    let mut results = Vec::with_capacity(limit.min(candidates.len()));

    while results.len() < limit && !remaining.is_empty() {
        let mut best_slot = 0usize;
        let mut best_total = f64::NEG_INFINITY;

        for (slot, &index) in remaining.iter().enumerate() {
            let base: f64 = base_factors[index].iter().map(|factor| factor.contribution).sum();
            let similarity = chosen
                .iter()
                .map(|&other| set_similarity(&category_sets[index], &category_sets[other]))
                .fold(0.0_f64, f64::max);
            // Popularity only breaks exact ties; it can never override a
            // preference-driven difference.
            let popularity = candidates[index].popularity.unwrap_or(0.0).clamp(0.0, 1.0);
            let total = base - weights.diversity * similarity + 1e-6 * popularity;

            if total > best_total {
                best_total = total;
                best_slot = slot;
            }
        }

        let index = remaining.remove(best_slot);
        let similarity = chosen
            .iter()
            .map(|&other| set_similarity(&category_sets[index], &category_sets[other]))
            .fold(0.0_f64, f64::max);

        let mut factors = base_factors[index].clone();
        factors.push(FactorContribution {
            name: "diversity".to_string(),
            label: if similarity > 0.0 {
                "Adds a topic the rest of your queue doesn't cover".to_string()
            } else {
                "Completely different from your other picks".to_string()
            },
            raw: similarity,
            weight: weights.diversity,
            contribution: -weights.diversity * similarity,
        });

        let score: f64 = factors.iter().map(|factor| factor.contribution).sum();
        chosen.push(index);
        results.push(ScoredCandidate {
            candidate: candidates[index].clone(),
            score,
            factors,
        });
    }

    results
}

fn score_candidate(
    candidate: &DiscoveryCandidate,
    profile: &ListeningProfile,
    weights: &Weights,
    now: DateTime<Utc>,
) -> Vec<FactorContribution> {
    let (topic_raw, topic_label) = topic_affinity(candidate, profile);
    let (completion_raw, completion_label) = completion_signal(profile);
    let (recency_raw, recency_label) = recency(candidate, now);
    let (novelty_raw, novelty_label) = novelty(candidate, profile);
    let (duration_raw, duration_label) = duration_fit(candidate, profile);

    let explicit_raw = if candidate.explicit && profile.avoid_explicit { 1.0 } else { 0.0 };

    vec![
        FactorContribution {
            name: "topic_affinity".to_string(),
            label: topic_label,
            raw: topic_raw,
            weight: weights.topic_affinity,
            contribution: weights.topic_affinity * topic_raw,
        },
        FactorContribution {
            name: "completion_signal".to_string(),
            label: completion_label,
            raw: completion_raw,
            weight: weights.completion_signal,
            contribution: weights.completion_signal * completion_raw,
        },
        FactorContribution {
            name: "recency".to_string(),
            label: recency_label,
            raw: recency_raw,
            weight: weights.recency,
            contribution: weights.recency * recency_raw,
        },
        FactorContribution {
            name: "novelty".to_string(),
            label: novelty_label,
            raw: novelty_raw,
            weight: weights.novelty,
            contribution: weights.novelty * novelty_raw,
        },
        FactorContribution {
            name: "duration_fit".to_string(),
            label: duration_label,
            raw: duration_raw,
            weight: weights.duration_fit,
            contribution: weights.duration_fit * duration_raw,
        },
        FactorContribution {
            name: "explicit_penalty".to_string(),
            label: if explicit_raw > 0.0 {
                "Explicit — filtered out by your preference".to_string()
            } else {
                "Passes your explicit-content filter".to_string()
            },
            raw: explicit_raw,
            weight: weights.explicit_penalty,
            contribution: -weights.explicit_penalty * explicit_raw,
        },
    ]
}

fn topic_affinity(candidate: &DiscoveryCandidate, profile: &ListeningProfile) -> (f64, String) {
    let mut total = 0.0;
    let mut best: Option<(String, f64)> = None;

    for category in &candidate.categories {
        let key = category.to_ascii_lowercase();
        if let Some(&weight) = profile.category_weights.get(&key) {
            total += weight;
            let better = match &best {
                Some((_, best_weight)) => weight > *best_weight,
                None => true,
            };
            if better {
                best = Some((key, weight));
            }
        }
    }

    // Saturating: many weak matches shouldn't beat one strong match by much.
    let raw = 1.0 - (-total).exp();
    let label = match best {
        Some((category, weight)) if weight > 0.4 => {
            format!("Matches your interest in {}", title_case(&category))
        }
        Some((category, _)) => format!("Touches on {}", title_case(&category)),
        None if profile.listened_episodes == 0 => "Starting point — no history yet".to_string(),
        None => "Outside your usual topics".to_string(),
    };
    (raw, label)
}

fn completion_signal(profile: &ListeningProfile) -> (f64, String) {
    if profile.listened_episodes == 0 {
        return (0.0, "No listening history yet".to_string());
    }
    let raw = profile.avg_completion.clamp(0.0, 1.0);
    let label = if raw >= 0.7 {
        format!("You finish about {:.0}% of what you start", raw * 100.0)
    } else {
        format!("You finish about {:.0}% of episodes you open", raw * 100.0)
    };
    (raw, label)
}

fn recency(candidate: &DiscoveryCandidate, now: DateTime<Utc>) -> (f64, String) {
    match candidate.latest_published {
        Some(published) => {
            let days = (now - published).num_days().max(0);
            let raw = (-(days as f64) / 30.0).exp();
            (raw, format!("Latest episode {}", humanize_days(days)))
        }
        None => (0.3, "Publish date unknown".to_string()),
    }
}

fn novelty(candidate: &DiscoveryCandidate, profile: &ListeningProfile) -> (f64, String) {
    let candidate_tokens = tokens(&candidate.title);
    let mut most_similar = 0.0_f64;

    for subscribed_tokens in &profile.subscribed_titles {
        let similarity = jaccard(&candidate_tokens, subscribed_tokens);
        if similarity > most_similar {
            most_similar = similarity;
        }
    }

    let raw = 1.0 - most_similar;
    let label = if most_similar > 0.5 {
        "Very similar to a show you already follow".to_string()
    } else if most_similar > 0.2 {
        "Related to shows in your library".to_string()
    } else {
        "New to your library".to_string()
    };
    (raw, label)
}

fn duration_fit(candidate: &DiscoveryCandidate, profile: &ListeningProfile) -> (f64, String) {
    match (candidate.typical_duration_secs, profile.preferred_duration_secs) {
        (Some(candidate_duration), Some(preferred)) if preferred > 0 => {
            let difference =
                (candidate_duration as f64 - preferred as f64).abs() / preferred as f64;
            let raw = (1.0 - difference).clamp(0.0, 1.0);
            let label = format!(
                "{} min episodes, near your {} min average",
                candidate_duration / 60,
                preferred / 60
            );
            (raw, label)
        }
        _ => (0.5, "Episode length varies".to_string()),
    }
}

fn completion_of(state: &PlaybackState) -> f64 {
    if state.completed {
        return 1.0;
    }
    state.completion().unwrap_or(0.0)
}

fn set_similarity(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    jaccard(a, b)
}

fn humanize_days(days: i64) -> String {
    match days {
        0 => "today".to_string(),
        1 => "yesterday".to_string(),
        d if d < 7 => format!("{d} days ago"),
        d if d < 30 => format!("{} weeks ago", d / 7),
        d if d < 365 => format!("{} months ago", d / 30),
        d => format!("{} years ago", d / 365),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Episode, PlaybackState, Show};

    fn candidate(title: &str, categories: &[&str], explicit: bool) -> DiscoveryCandidate {
        DiscoveryCandidate {
            title: title.to_string(),
            feed_url: format!("https://example.com/{}.xml", title.to_ascii_lowercase()),
            description: None,
            image_url: None,
            author: None,
            categories: categories.iter().map(|c| c.to_string()).collect(),
            latest_published: Some(Utc::now()),
            typical_duration_secs: Some(1800),
            explicit,
            popularity: None,
            source: "test".to_string(),
        }
    }

    /// A listener who has fully finished a Technology show.
    fn library_with_history() -> Library {
        let mut library = Library::new();
        library.shows.insert(
            "sh_1".to_string(),
            Show {
                id: "sh_1".to_string(),
                feed_url: "https://example.com/tech.xml".to_string(),
                title: "Tech Weekly".to_string(),
                author: None,
                description: None,
                image_url: None,
                link: None,
                language: None,
                categories: vec!["Technology".to_string()],
                explicit: false,
                published: None,
                updated: None,
                subscribed: true,
                last_refreshed: None,
                etag: None,
                last_modified: None,
            },
        );
        library.episodes.insert(
            "ep_1".to_string(),
            Episode {
                id: "ep_1".to_string(),
                show_id: "sh_1".to_string(),
                title: "An episode".to_string(),
                description: None,
                enclosure_url: "https://example.com/1.mp3".to_string(),
                duration_secs: Some(1800),
                published: None,
                image_url: None,
                guid: "ep_1".to_string(),
            },
        );
        library.playback.insert(
            "ep_1".to_string(),
            PlaybackState {
                episode_id: "ep_1".to_string(),
                position_secs: 1800.0,
                duration_secs: Some(1800),
                completed: true,
                last_played: Some(Utc::now()),
                play_count: 1,
            },
        );
        library
    }

    #[test]
    fn contributions_sum_to_score() {
        let profile = build_profile(&library_with_history());
        let results = rank(
            &[candidate("Tech Talk", &["Technology"], false)],
            &profile,
            &Weights::default(),
            5,
        );
        let scored = &results[0];
        let sum: f64 = scored.factors.iter().map(|factor| factor.contribution).sum();
        assert!((sum - scored.score).abs() < 1e-9);
    }

    #[test]
    fn topic_match_outranks_unrelated() {
        let profile = build_profile(&library_with_history());
        let results = rank(
            &[
                candidate("Gardening Hour", &["Gardening"], false),
                candidate("Tech Talk", &["Technology"], false),
            ],
            &profile,
            &Weights::default(),
            2,
        );
        assert_eq!(results[0].candidate.title, "Tech Talk");
    }

    #[test]
    fn explicit_is_penalised_when_filtering() {
        let mut library = library_with_history();
        library.settings.avoid_explicit = true;
        let profile = build_profile(&library);

        let results = rank(
            &[candidate("Risky Tech", &["Technology"], true)],
            &profile,
            &Weights::default(),
            1,
        );
        let factor = results[0]
            .factors
            .iter()
            .find(|factor| factor.name == "explicit_penalty")
            .unwrap();
        assert_eq!(factor.raw, 1.0);
        assert!(factor.contribution < 0.0);
    }

    #[test]
    fn diversity_discounts_repeated_topics() {
        let profile = build_profile(&library_with_history());
        let results = rank(
            &[
                candidate("Tech A", &["Technology"], false),
                candidate("Tech B", &["Technology"], false),
            ],
            &profile,
            &Weights::default(),
            2,
        );
        let first = results[0]
            .factors
            .iter()
            .find(|factor| factor.name == "diversity")
            .unwrap();
        let second = results[1]
            .factors
            .iter()
            .find(|factor| factor.name == "diversity")
            .unwrap();
        assert_eq!(first.raw, 0.0);
        assert_eq!(second.raw, 1.0);
        assert!(second.contribution < 0.0);
    }

    #[test]
    fn ranking_is_deterministic() {
        let profile = build_profile(&library_with_history());
        let pool = vec![
            candidate("Alpha", &["Technology"], false),
            candidate("Beta", &["Science"], false),
            candidate("Gamma", &["Technology", "Science"], false),
        ];
        let first = rank(&pool, &profile, &Weights::default(), 3);
        let second = rank(&pool, &profile, &Weights::default(), 3);
        let titles = |results: &[ScoredCandidate]| {
            results
                .iter()
                .map(|scored| scored.candidate.title.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(titles(&first), titles(&second));
    }

    #[test]
    fn empty_history_still_ranks() {
        let profile = build_profile(&Library::new());
        let results = rank(
            &[candidate("Something", &["Technology"], false)],
            &profile,
            &Weights::default(),
            1,
        );
        assert_eq!(results.len(), 1);
        assert!(results[0].score.is_finite());
    }

    #[test]
    fn top_reasons_are_ordered_by_impact() {
        let profile = build_profile(&library_with_history());
        let results = rank(
            &[candidate("Tech Talk", &["Technology"], false)],
            &profile,
            &Weights::default(),
            1,
        );
        let reasons = results[0].top_reasons(3);
        assert!(!reasons.is_empty());
        for pair in reasons.windows(2) {
            assert!(pair[0].contribution.abs() >= pair[1].contribution.abs());
        }
    }
}

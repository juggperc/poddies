//! Reference plugin: a listening-habits panel.
//!
//! Where the Stats plugin answers *how much*, this one answers *when*. It reads
//! the same library history and looks only at timestamps: streaks of
//! consecutive listening days, the hour-of-day rhythm split into four buckets,
//! and the balance across weekdays.
//!
//! Timestamps are read in UTC — the plugin sees no timezone, and a wrong local
//! guess would be worse than an honest UTC answer, so bucket labels say so.

use std::collections::BTreeSet;

use chrono::{DateTime, Datelike, NaiveDate, Timelike, Utc};
use poddies_plugin_sdk::api::library::HistoryEntry;
use poddies_plugin_sdk::{
    export_plugin, host_call, json, PanelContent, Plugin, PluginError, PluginInfo,
    UiPanelDescriptor, Widget, PROTOCOL_VERSION,
};
use serde_json::Value;

const PANEL_ID: &str = "habits";
const PLUGIN_ID: &str = "dev.poddies.listening-clock";

const BUCKETS: [(&str, u32, u32); 4] = [
    ("Night (00-05)", 0, 6),
    ("Morning (06-11)", 6, 12),
    ("Afternoon (12-17)", 12, 18),
    ("Evening / Late (18-23)", 18, 24),
];

/// Weekday names indexed by `Weekday::num_days_from_monday()`.
const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

#[derive(Default)]
pub struct ListeningClock;

impl Plugin for ListeningClock {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: PLUGIN_ID.to_string(),
            name: "Listening Clock".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: PROTOCOL_VERSION.to_string(),
            ui_panels: vec![UiPanelDescriptor::sidebar(PANEL_ID, "Habits")],
        }
    }

    fn on_request(&mut self, method: &str, _params: Value) -> Result<Value, PluginError> {
        match method {
            "ui/panel" => {
                let value = host_call("host/library/history", json!({}))?;
                let entries: Vec<HistoryEntry> = serde_json::from_value(value)
                    .map_err(|err| PluginError::new("bad_host_data", err.to_string()))?;
                let sessions: Vec<DateTime<Utc>> = entries
                    .into_iter()
                    .filter_map(|entry| entry.last_played)
                    .collect();

                let content = PanelContent {
                    panel_id: PANEL_ID.to_string(),
                    widgets: build_widgets(&sessions),
                };
                serde_json::to_value(content)
                    .map_err(|err| PluginError::new("serialize_failed", err.to_string()))
            }
            other => Err(PluginError::unsupported(other)),
        }
    }
}

fn build_widgets(sessions: &[DateTime<Utc>]) -> Vec<Widget> {
    if sessions.is_empty() {
        return vec![
            Widget::Heading {
                text: "No listening history yet".to_string(),
            },
            Widget::Text {
                text: "Play a few episodes and your habits will show up here.".to_string(),
            },
        ];
    }

    let distinct_days = days(sessions).len();

    let mut widgets = vec![Widget::Heading {
        text: format!(
            "{distinct_days} listening days, {} sessions",
            sessions.len()
        ),
    }];

    let today = Utc::now().date_naive();
    if let Some(streak) = current_streak(&days(sessions), today) {
        widgets.push(Widget::Metric {
            label: "Current streak".to_string(),
            value: format!("{streak} day{}", if streak == 1 { "" } else { "s" }),
        });
    }

    for (label, from, to) in BUCKETS {
        let count = sessions
            .iter()
            .filter(|at| (from..to).contains(&at.hour()))
            .count();
        if count > 0 {
            widgets.push(Widget::Bar {
                label: format!("{label} UTC"),
                value: count as f64,
                max: sessions.len() as f64,
            });
        }
    }

    widgets.push(Widget::Divider);

    for (day, count) in weekday_histogram(sessions) {
        if count > 0 {
            widgets.push(Widget::Bar {
                label: WEEKDAYS[day].to_string(),
                value: count as f64,
                max: sessions.len() as f64,
            });
        }
    }

    widgets
}

/// Calendar days (UTC) seen in the history, ascending.
fn days(sessions: &[DateTime<Utc>]) -> Vec<NaiveDate> {
    sessions
        .iter()
        .map(|at| at.date_naive())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Consecutive listening days ending today (or, when today's session has not
/// happened yet, yesterday). `days` must be ascending; the walk is O(days).
fn current_streak(days: &[NaiveDate], today: NaiveDate) -> Option<usize> {
    let latest = days.last()?;
    // A streak only counts while it is still alive: the last listening day is
    // today, or yesterday when today's session has not happened yet.
    if *latest != today && today.checked_sub_days(chrono::Days::new(1)) != Some(*latest) {
        return None;
    }

    let mut streak = 1usize;
    for pair in days.windows(2).rev() {
        // Ascending pair [a, b]: consecutive when b is exactly one day later.
        let next = pair[0].checked_add_days(chrono::Days::new(1));
        if next == Some(pair[1]) {
            streak += 1;
        } else {
            break;
        }
    }
    Some(streak)
}

/// Sessions per weekday, Monday ..= Sunday, zero counts skipped by callers.
fn weekday_histogram(sessions: &[DateTime<Utc>]) -> Vec<(usize, usize)> {
    let mut counts = [0usize; 7];
    for at in sessions {
        counts[at.weekday().num_days_from_monday() as usize] += 1;
    }
    counts
        .into_iter()
        .enumerate()
        .filter(|(_, count)| *count > 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, day, hour, 0, 0).unwrap()
    }

    fn days_of(dates: &[DateTime<Utc>]) -> Vec<NaiveDate> {
        days(dates)
    }

    #[test]
    fn empty_history_builds_the_explainer() {
        let widgets = build_widgets(&[]);
        assert_eq!(widgets.len(), 2);
    }

    #[test]
    fn bucket_bars_sum_to_the_session_count() {
        let sessions = vec![utc(1, 2), utc(1, 9), utc(1, 13), utc(1, 20)];
        let bars = build_widgets(&sessions)
            .into_iter()
            .filter_map(|widget| match widget {
                Widget::Bar { label, value, max } if label.contains("UTC") => Some((value, max)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(bars.len(), 4);
        assert_eq!(bars.iter().map(|(value, _)| *value).sum::<f64>(), 4.0);
        assert!(bars.iter().all(|(_, max)| *max == 4.0));
    }

    #[test]
    fn weekday_histogram_counts_per_day() {
        let sessions = vec![utc(1, 8), utc(1, 9), utc(7, 22)];
        let histogram = weekday_histogram(&sessions);
        assert_eq!(histogram.len(), 2);
        // 2026-09-01 is a Tuesday.
        assert!(histogram.contains(&(1, 2)));
    }

    #[test]
    fn streak_runs_back_through_consecutive_days() {
        let sessions = vec![utc(7, 8), utc(6, 8), utc(5, 8)];
        let today = utc(7, 0).date_naive();
        assert_eq!(current_streak(&days_of(&sessions), today), Some(3));
    }

    #[test]
    fn streak_survives_a_missing_today_session() {
        let sessions = vec![utc(6, 8), utc(5, 8)];
        let today = utc(7, 0).date_naive();
        assert_eq!(current_streak(&days_of(&sessions), today), Some(2));
    }

    #[test]
    fn stale_history_is_no_streak() {
        let sessions = vec![utc(1, 8), utc(2, 8)];
        let today = utc(7, 0).date_naive();
        assert_eq!(current_streak(&days_of(&sessions), today), None);
    }
}

export_plugin!(ListeningClock);

//! Reference plugin: a listening-statistics panel.
//!
//! Demonstrates the panel surface plus library reads. It asks the host for the
//! user's shows and history, aggregates them, and returns declarative widgets —
//! no HTML, no styling, so it renders in the app's own typography.

use poddies_plugin_sdk::api::library::{HistoryEntry, ShowSummary};
use poddies_plugin_sdk::{
    export_plugin, host_call, json, ListItem, PanelContent, Plugin, PluginError, PluginInfo,
    UiPanelDescriptor, Widget, PROTOCOL_VERSION,
};
use serde_json::Value;

const PANEL_ID: &str = "listening";
const PLUGIN_ID: &str = "dev.poddies.stats";

#[derive(Default)]
pub struct Stats;

impl Stats {
    fn load_shows(&self) -> Result<Vec<ShowSummary>, PluginError> {
        let value = host_call("host/library/shows", json!({}))?;
        serde_json::from_value(value)
            .map_err(|err| PluginError::new("bad_host_data", err.to_string()))
    }

    fn load_history(&self) -> Result<Vec<HistoryEntry>, PluginError> {
        let value = host_call("host/library/history", json!({}))?;
        serde_json::from_value(value)
            .map_err(|err| PluginError::new("bad_host_data", err.to_string()))
    }
}

impl Plugin for Stats {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: PLUGIN_ID.to_string(),
            name: "Listening Stats".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: PROTOCOL_VERSION.to_string(),
            ui_panels: vec![UiPanelDescriptor::sidebar(PANEL_ID, "Listening")],
        }
    }

    fn on_request(&mut self, method: &str, _params: Value) -> Result<Value, PluginError> {
        match method {
            "ui/panel" => {
                let shows = self.load_shows()?;
                let history = self.load_history()?;
                let content = PanelContent {
                    panel_id: PANEL_ID.to_string(),
                    widgets: build_widgets(&shows, &history),
                };
                serde_json::to_value(content)
                    .map_err(|err| PluginError::new("serialize_failed", err.to_string()))
            }
            other => Err(PluginError::unsupported(other)),
        }
    }
}

fn build_widgets(shows: &[ShowSummary], history: &[HistoryEntry]) -> Vec<Widget> {
    let subscribed = shows.iter().filter(|show| show.subscribed).count();
    let started = history.len();
    let finished = history.iter().filter(|entry| entry.completed).count();
    let listened_secs: f64 = history.iter().map(|entry| entry.position_secs.max(0.0)).sum();
    let completion = if started == 0 {
        0.0
    } else {
        finished as f64 / started as f64
    };

    let mut widgets = vec![
        Widget::Metric {
            label: "Subscriptions".to_string(),
            value: subscribed.to_string(),
        },
        Widget::Metric {
            label: "Episodes started".to_string(),
            value: started.to_string(),
        },
        Widget::Metric {
            label: "Episodes finished".to_string(),
            value: finished.to_string(),
        },
        Widget::Metric {
            label: "Time listened".to_string(),
            value: format_duration(listened_secs),
        },
        Widget::Bar {
            label: "Completion rate".to_string(),
            value: completion,
            max: 1.0,
        },
    ];

    let top = most_listened(history, 5);
    if !top.is_empty() {
        widgets.push(Widget::Divider);
        widgets.push(Widget::List {
            items: top
                .into_iter()
                .map(|(title, count)| ListItem {
                    primary: title,
                    secondary: Some(match count {
                        1 => "1 episode".to_string(),
                        other => format!("{other} episodes"),
                    }),
                })
                .collect(),
        });
    }

    widgets
}

fn most_listened(history: &[HistoryEntry], limit: usize) -> Vec<(String, usize)> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for entry in history {
        match counts
            .iter_mut()
            .find(|(title, _)| title == &entry.show_title)
        {
            Some((_, count)) => *count += 1,
            None => counts.push((entry.show_title.clone(), 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counts.truncate(limit);
    counts
}

fn format_duration(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

export_plugin!(Stats);

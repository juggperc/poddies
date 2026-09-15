//! The declarative UI surface.
//!
//! Plugins do not ship HTML, JavaScript or their own renderer. They describe
//! panels as a list of [`Widget`]s and the host renders them with the app's own
//! typography and spacing. That keeps every screen visually consistent and
//! means a plugin cannot inject markup into the webview.

use serde::{Deserialize, Serialize};

/// A panel a plugin wants the host to offer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiPanelDescriptor {
    pub id: String,
    pub title: String,
}

/// The contents of a panel, produced on demand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelContent {
    pub panel_id: String,
    #[serde(default)]
    pub widgets: Vec<Widget>,
}

/// A single rendered element. Deliberately a small closed set: enough for
/// stats, lists and status, not enough to build a second UI toolkit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Widget {
    /// Section title.
    Heading { text: String },
    /// A large number with a caption, e.g. "128 / Episodes".
    Metric { label: String, value: String },
    /// A paragraph of plain text.
    Text { text: String },
    /// A horizontal rule.
    Divider,
    /// A proportional bar.
    Bar { label: String, value: f64, max: f64 },
    /// A list of primary/secondary rows.
    List { items: Vec<ListItem> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListItem {
    pub primary: String,
    #[serde(default)]
    pub secondary: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widgets_use_a_tagged_representation() {
        let widget = Widget::Metric {
            label: "Episodes".to_string(),
            value: "128".to_string(),
        };
        let json = serde_json::to_value(&widget).unwrap();
        assert_eq!(json["type"], "metric");
        assert_eq!(json["value"], "128");

        assert_eq!(
            serde_json::to_value(Widget::Divider).unwrap()["type"],
            "divider"
        );
    }
}

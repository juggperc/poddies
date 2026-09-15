//! The declarative UI surface.
//!
//! Plugins do not ship HTML, JavaScript or their own renderer. They describe
//! panels as a list of [`Widget`]s and the host draws them with the app's own
//! typography and spacing. That keeps every screen visually consistent, keeps a
//! plugin from injecting markup into the webview, and means a panel looks right
//! in both the app window and a popout without the plugin doing anything.
//!
//! Several widgets are interactive: a plugin gives each one an `id`, the host
//! reports the new value with a `ui/change` notification, and the plugin answers
//! the next `ui/panel` with updated state. Widgets bound to the audio pipeline
//! work the same way — the plugin returns an [`AudioUnit`](crate::protocol::AudioUnit)
//! from `audio/graph` and the host does the DSP.

use serde::{Deserialize, Serialize};

/// Where a panel should appear.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// A section in the library, listed with the other navigation.
    #[default]
    Sidebar,
    /// Docked in the now-playing pane, under the playback speed control. For
    /// controls you want at hand while listening.
    NowPlaying,
    /// Its own small always-available window, opened from Settings. For
    /// something with real estate — an EQ curve, a spectrum.
    Popout,
}

/// A panel a plugin wants the host to offer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiPanelDescriptor {
    pub id: String,
    pub title: String,
    /// Defaults to [`Placement::Sidebar`] when absent, so a plugin written
    /// against an earlier minor version keeps working.
    #[serde(default)]
    pub placement: Placement,
}

impl UiPanelDescriptor {
    /// A panel in the library's navigation.
    pub fn sidebar(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            placement: Placement::Sidebar,
        }
    }

    /// A panel docked in the now-playing pane.
    pub fn now_playing(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            placement: Placement::NowPlaying,
        }
    }

    /// A panel that opens in its own window.
    pub fn popout(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            placement: Placement::Popout,
        }
    }
}

/// The contents of a panel, produced on demand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelContent {
    pub panel_id: String,
    #[serde(default)]
    pub widgets: Vec<Widget>,
}

/// A single rendered element.
///
/// The set is deliberately closed: enough for real controls and displays, not
/// enough to rebuild a browser. Interactive widgets carry an `id` that comes
/// back to the plugin in a `ui/change` notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Widget {
    /// Section title.
    Heading { text: String },
    /// A large number with a caption.
    Metric { label: String, value: String },
    /// A paragraph of plain text.
    Text { text: String },
    /// A horizontal rule.
    Divider,
    /// A proportional bar.
    Bar { label: String, value: f64, max: f64 },
    /// A list of primary/secondary rows.
    List { items: Vec<ListItem> },

    /// A rotary control. Drag to turn, shift-drag for fine adjustment.
    Knob {
        id: String,
        label: String,
        value: f64,
        #[serde(default)]
        min: f64,
        #[serde(default = "one")]
        max: f64,
        #[serde(default)]
        unit: String,
        #[serde(default)]
        style: KnobStyle,
        /// Text under the value, e.g. a derived ratio like "4.0:1".
        #[serde(default)]
        readout: Option<String>,
    },

    /// A horizontal control, for values that read better as a length.
    Slider {
        id: String,
        label: String,
        value: f64,
        #[serde(default)]
        min: f64,
        #[serde(default = "one")]
        max: f64,
        #[serde(default)]
        step: f64,
        #[serde(default)]
        unit: String,
    },

    /// A two-state switch.
    Toggle {
        id: String,
        label: String,
        value: bool,
    },

    /// A parametric EQ curve. Drag a node vertically to set its gain; the host
    /// draws the true summed response of the bands you declare here.
    Eq {
        id: String,
        bands: Vec<EqBand>,
        #[serde(default = "default_min_gain")]
        min_gain_db: f64,
        #[serde(default = "default_max_gain")]
        max_gain_db: f64,
        #[serde(default = "default_min_frequency")]
        min_frequency: f64,
        #[serde(default = "default_max_frequency")]
        max_frequency: f64,
    },

    /// A live level meter, fed by the audio pipeline rather than by the plugin.
    /// `source` selects what it shows.
    Meter {
        id: String,
        label: String,
        source: MeterSource,
        #[serde(default = "default_min_meter_db")]
        min_db: f64,
        #[serde(default)]
        max_db: f64,
    },
}

/// How a [`Widget::Knob`] is drawn. Both are monochrome; `Vintage` is heavier,
/// with a knurled edge and an ivory pointer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnobStyle {
    #[default]
    Modern,
    Vintage,
}

/// What a [`Widget::Meter`] displays. Values come from the host's audio graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeterSource {
    /// Peak level of the output, in dBFS.
    Peak,
    /// How much a compressor unit is pulling the signal down, in dB (negative).
    GainReduction,
}

/// One band of a [`Widget::Eq`], and of the matching audio unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqBand {
    pub id: String,
    pub label: String,
    /// Centre (or corner) frequency, Hz.
    pub frequency: f64,
    pub gain_db: f64,
    /// Width. Ignored by shelf filters.
    #[serde(default = "default_q")]
    pub q: f64,
    #[serde(default)]
    pub kind: EqBandKind,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqBandKind {
    #[default]
    Peaking,
    LowShelf,
    HighShelf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListItem {
    pub primary: String,
    #[serde(default)]
    pub secondary: Option<String>,
}

fn one() -> f64 {
    1.0
}

fn default_q() -> f64 {
    1.0
}

fn default_min_gain() -> f64 {
    -18.0
}

fn default_max_gain() -> f64 {
    18.0
}

fn default_min_frequency() -> f64 {
    30.0
}

fn default_max_frequency() -> f64 {
    16_000.0
}

fn default_min_meter_db() -> f64 {
    -24.0
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

        assert_eq!(serde_json::to_value(Widget::Divider).unwrap()["type"], "divider");
    }

    #[test]
    fn interactive_widgets_round_trip() {
        let widget = Widget::Knob {
            id: "amount".to_string(),
            label: "Amount".to_string(),
            value: 0.5,
            min: 0.0,
            max: 1.0,
            unit: String::new(),
            style: KnobStyle::Vintage,
            readout: Some("4.0:1".to_string()),
        };
        let encoded = serde_json::to_string(&widget).unwrap();
        assert!(encoded.contains("\"type\":\"knob\""));
        assert!(encoded.contains("\"style\":\"vintage\""));
        assert_eq!(serde_json::from_str::<Widget>(&encoded).unwrap(), widget);
    }

    #[test]
    fn an_eq_supplies_its_own_defaults() {
        let json = r#"{
            "type": "eq",
            "id": "curve",
            "bands": [{ "id": "low", "label": "Low", "frequency": 80.0, "gain_db": 2.0 }]
        }"#;
        let Widget::Eq { bands, min_gain_db, .. } = serde_json::from_str(json).unwrap() else {
            panic!("expected an eq widget");
        };
        assert_eq!(bands.len(), 1);
        assert_eq!(bands[0].q, 1.0);
        assert_eq!(bands[0].kind, EqBandKind::Peaking);
        assert_eq!(min_gain_db, -18.0);
    }

    #[test]
    fn a_panel_defaults_to_the_sidebar() {
        let descriptor: UiPanelDescriptor =
            serde_json::from_str(r#"{ "id": "main", "title": "Main" }"#).unwrap();
        assert_eq!(descriptor.placement, Placement::Sidebar);

        let docked = UiPanelDescriptor::now_playing("comp", "Compressor");
        assert_eq!(docked.placement, Placement::NowPlaying);
    }
}

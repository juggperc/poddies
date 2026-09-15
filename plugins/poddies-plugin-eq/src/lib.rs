//! Reference plugin: a five-band parametric EQ.
//!
//! Demonstrates the two halves of the newer plugin surface working together:
//!
//! * a **popout panel** with a genuinely custom control — the host draws the
//!   curve, but the shape, the bands and the interaction are this plugin's;
//! * an **audio unit** — the plugin never touches a sample, it just says which
//!   filters it wants, and the host builds them on the audio thread.
//!
//! That split is why the curve and the sound always agree: both the drawing and
//! the DSP come from the same band parameters, and the drag comes back here as
//! a `ui/change` notification.

use std::sync::Mutex;

use poddies_plugin_sdk::api::protocol::{AudioGraph, AudioUnit};
use poddies_plugin_sdk::api::ui::{EqBand, EqBandKind, Widget};
use poddies_plugin_sdk::{
    export_plugin, host_call, json, PanelContent, Plugin, PluginError, PluginInfo,
    UiPanelDescriptor, PROTOCOL_VERSION,
};
use serde::Deserialize;
use serde_json::Value;

const PLUGIN_ID: &str = "dev.poddies.eq";
const PANEL_ID: &str = "curve";
const EQ_UNIT_ID: &str = "eq";

/// Low shelf, three peaks, high shelf — the classic five.
const FACTORY_BANDS: [(&str, &str, f64, f64, EqBandKind); 5] = [
    ("low", "Low", 90.0, 0.7, EqBandKind::LowShelf),
    ("low_mid", "Low mid", 320.0, 1.0, EqBandKind::Peaking),
    ("mid", "Mid", 1_000.0, 1.0, EqBandKind::Peaking),
    ("high_mid", "High mid", 3_200.0, 1.0, EqBandKind::Peaking),
    ("high", "High", 8_000.0, 0.7, EqBandKind::HighShelf),
];

#[derive(Default)]
pub struct ParametricEq {
    state: Mutex<State>,
}

struct State {
    enabled: bool,
    bands: Vec<EqBand>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            enabled: true,
            bands: FACTORY_BANDS
                .iter()
                .map(|(id, label, frequency, q, kind)| EqBand {
                    id: (*id).to_string(),
                    label: (*label).to_string(),
                    frequency: *frequency,
                    gain_db: 0.0,
                    q: *q,
                    kind: *kind,
                })
                .collect(),
        }
    }
}

#[derive(Deserialize)]
struct Change {
    panel_id: String,
    widget_id: String,
    value: Value,
}

#[derive(Deserialize)]
struct BandMove {
    band_id: String,
    gain_db: f64,
    frequency: Option<f64>,
}

impl ParametricEq {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Apply a `ui/change`. Only a move on this panel's curve matters.
    fn on_change(&self, params: &Value) -> Result<(), PluginError> {
        let Ok(change) = serde_json::from_value::<Change>(params.clone()) else {
            return Ok(());
        };
        if change.panel_id != PANEL_ID || change.widget_id != "curve" {
            return Ok(());
        }

        match change.value.get("toggle").and_then(Value::as_bool) {
            Some(enabled) => self.lock().enabled = enabled,
            None => {
                let Ok(moved) = serde_json::from_value::<BandMove>(change.value.clone()) else {
                    return Ok(());
                };
                let mut state = self.lock();
                if let Some(band) = state.bands.iter_mut().find(|band| band.id == moved.band_id) {
                    band.gain_db = moved.gain_db.clamp(-18.0, 18.0);
                    if let Some(frequency) = moved.frequency {
                        band.frequency = frequency.clamp(30.0, 16_000.0);
                    }
                }
            }
        }
        Ok(())
    }

    fn panel(&self) -> Result<Value, PluginError> {
        let state = self.lock();

        let widgets = vec![
            Widget::Eq {
                id: "curve".to_string(),
                bands: state.bands.clone(),
                min_gain_db: -18.0,
                max_gain_db: 18.0,
                min_frequency: 30.0,
                max_frequency: 16_000.0,
            },
            Widget::Toggle {
                id: "curve".to_string(),
                label: "Enabled".to_string(),
                value: state.enabled,
            },
            Widget::Text {
                text: "Drag a node vertically for gain, horizontally for frequency. Double-click \
                       a node to flatten it."
                    .to_string(),
            },
        ];

        serde_json::to_value(PanelContent {
            panel_id: PANEL_ID.to_string(),
            widgets,
        })
        .map_err(|error| PluginError::new("serialize_failed", error.to_string()))
    }

    fn graph(&self) -> Result<Value, PluginError> {
        let state = self.lock();
        serde_json::to_value(AudioGraph {
            units: vec![AudioUnit::ParametricEq {
                id: EQ_UNIT_ID.to_string(),
                enabled: state.enabled,
                bands: state.bands.clone(),
            }],
        })
        .map_err(|error| PluginError::new("serialize_failed", error.to_string()))
    }
}

impl Plugin for ParametricEq {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: PLUGIN_ID.to_string(),
            name: "Parametric EQ".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: PROTOCOL_VERSION.to_string(),
            ui_panels: vec![UiPanelDescriptor::popout(PANEL_ID, "Parametric EQ")],
        }
    }

    fn on_request(&mut self, method: &str, _params: Value) -> Result<Value, PluginError> {
        match method {
            "ui/panel" => self.panel(),
            "audio/graph" => self.graph(),
            other => Err(PluginError::unsupported(other)),
        }
    }

    fn on_notification(&mut self, method: &str, params: Value) {
        if method == "ui/change" {
            if let Err(error) = self.on_change(&params) {
                let _ = host_call("host/log", json!({ "level": "warn", "message": error.message }));
            }
        }
    }
}

export_plugin!(ParametricEq);

//! Reference plugin: a one-knob compressor in the vintage style.
//!
//! Demonstrates the other half of the new surface:
//!
//! * a panel **docked into the now-playing pane**, under the speed control, so
//!   it is at hand while listening rather than buried in Settings;
//! * a single **vintage knob** — one control that moves four parameters
//!   together, the way outboard gear does it;
//! * a **live gain-reduction meter**, painted by the host straight from the
//!   audio graph rather than round-tripping through this plugin;
//! * an **audio unit** the host builds, plus playback hooks so the plugin knows
//!   whether anything is playing.
//!
//! The single `amount` parameter is mapped in `curve()`. Keeping that mapping
//! here — rather than in the host — is the point: the character of the
//! compression is the plugin's, the DSP is the host's.

use std::sync::Mutex;

use poddies_plugin_sdk::api::protocol::{AudioGraph, AudioUnit};
use poddies_plugin_sdk::api::ui::{KnobStyle, MeterSource, Widget};
use poddies_plugin_sdk::{
    export_plugin, PanelContent, Plugin, PluginError, PluginInfo, UiPanelDescriptor,
    PROTOCOL_VERSION,
};
use serde::Deserialize;
use serde_json::Value;

const PLUGIN_ID: &str = "dev.poddies.compressor";
const PANEL_ID: &str = "dial";
const UNIT_ID: &str = "compressor";

/// Where one knob lands in DSP terms, at each end of its travel.
///
/// The feel is deliberately gentle at the bottom and assertive at the top: a
/// show mixed for speech needs almost nothing, a noisy live recording needs
/// work, and one control should cover both without a manual.
struct Settings {
    threshold_db: f64,
    ratio: f64,
    attack_ms: f64,
    release_ms: f64,
    makeup_db: f64,
}

fn curve(amount: f64) -> Settings {
    let amount = amount.clamp(0.0, 1.0);
    let shaped = amount * amount; // more resolution at the quiet end

    Settings {
        // -6 dB (barely touching) down to -34 dB (a lot of levelling)
        threshold_db: -6.0 - shaped * 28.0,
        // 1.6:1 up to 8:1
        ratio: 1.6 + shaped * 6.4,
        // Slow enough to stay transparent, quick enough to catch a cough
        attack_ms: 30.0 - amount * 22.0,
        release_ms: 140.0 + amount * 260.0,
        // Put back what the compressing took away, roughly
        makeup_db: shaped * 7.0,
    }
}

fn describe(settings: &Settings) -> String {
    format!("{:.1}:1 · {:.0} dB", settings.ratio, settings.threshold_db)
}

#[derive(Default)]
pub struct Compressor {
    state: Mutex<State>,
}

struct State {
    amount: f64,
    enabled: bool,
    playing: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            // A sensible resting place: some levelling, no pumping.
            amount: 0.35,
            enabled: true,
            playing: false,
        }
    }
}

#[derive(Deserialize)]
struct Change {
    panel_id: String,
    widget_id: String,
    value: Value,
}

impl Compressor {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn on_change(&self, params: &Value) -> Result<(), PluginError> {
        let Ok(change) = serde_json::from_value::<Change>(params.clone()) else {
            return Ok(());
        };
        if change.panel_id != PANEL_ID {
            return Ok(());
        }

        match change.widget_id.as_str() {
            "amount" => {
                if let Some(value) = change.value.as_f64() {
                    self.lock().amount = value.clamp(0.0, 1.0);
                }
            }
            "bypass" => {
                if let Some(value) = change.value.as_bool() {
                    self.lock().enabled = !value;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn panel(&self) -> Result<Value, PluginError> {
        let state = self.lock();
        let settings = curve(state.amount);

        let widgets = vec![
            Widget::Knob {
                id: "amount".to_string(),
                label: "Amount".to_string(),
                value: state.amount,
                min: 0.0,
                max: 1.0,
                unit: String::new(),
                style: KnobStyle::Vintage,
                readout: Some(describe(&settings)),
            },
            Widget::Meter {
                id: "reduction".to_string(),
                label: "Gain reduction".to_string(),
                source: MeterSource::GainReduction,
                min_db: -18.0,
                max_db: 0.0,
            },
            Widget::Toggle {
                id: "bypass".to_string(),
                label: "Bypass".to_string(),
                value: !state.enabled,
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
        let settings = curve(state.amount);

        serde_json::to_value(AudioGraph {
            units: vec![AudioUnit::Compressor {
                id: UNIT_ID.to_string(),
                enabled: state.enabled,
                threshold_db: settings.threshold_db,
                ratio: settings.ratio,
                attack_ms: settings.attack_ms,
                release_ms: settings.release_ms,
                knee_db: 6.0,
                makeup_db: settings.makeup_db,
            }],
        })
        .map_err(|error| PluginError::new("serialize_failed", error.to_string()))
    }
}

impl Plugin for Compressor {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: PLUGIN_ID.to_string(),
            name: "Compressor".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: PROTOCOL_VERSION.to_string(),
            ui_panels: vec![UiPanelDescriptor::now_playing(PANEL_ID, "Compressor")],
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
        match method {
            "ui/change" => {
                let _ = self.on_change(&params);
            }
            "event/playback-started" | "event/playback-progress" => self.lock().playing = true,
            "event/playback-completed" => self.lock().playing = false,
            _ => {}
        }
    }
}

export_plugin!(Compressor);

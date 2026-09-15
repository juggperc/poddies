//! Reference plugin: late-night listening mode.
//!
//! Demonstrates one thing the other audio plugins do not: **composing both
//! audio units** the host can build into a single chain. The idea is the
//! classic "compress, then EQ for a quiet room" order — level the show first so
//! nothing startles anyone awake, then shape the frequency balance for speech
//! over speakers at low volume.
//!
//! The chain, in the order the graph returns it:
//!
//! 1. a `compressor` — the dynamic-range tamer;
//! 2. a `parametric_eq` — deep-bass reduction, a mud scoop, a small presence
//!    lift so quiet speech stays intelligible.
//!
//! The plugin never touches a sample. Both units' parameters come from one
//! `intensity` value mapped in `curve()`, exactly like the Compressor plugin's
//! one-knob mapping, and playback hooks flip the whole chain off when nothing
//! is playing.

use std::sync::Mutex;

use poddies_plugin_sdk::api::protocol::{AudioGraph, AudioUnit};
use poddies_plugin_sdk::api::ui::{EqBand, EqBandKind, Widget};
use poddies_plugin_sdk::{
    export_plugin, PanelContent, Plugin, PluginError, PluginInfo, UiPanelDescriptor,
    PROTOCOL_VERSION,
};
use serde::Deserialize;
use serde_json::Value;

const PLUGIN_ID: &str = "dev.poddies.night-listening";
const PANEL_ID: &str = "night";
const COMPRESSOR_ID: &str = "night-comp";
const EQ_ID: &str = "night-eq";

#[derive(Default)]
pub struct NightListening {
    state: Mutex<State>,
}

struct State {
    /// 0.0 off-hours, 1.0 firmest night setting.
    intensity: f64,
    enabled: bool,
    playing: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            // On by default at a gentle level — that is the point of the
            // plugin: it should be quietly doing its job, not waiting to be
            // discovered.
            intensity: 0.5,
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

/// Where one `intensity` lands in DSP terms. Shaped so the low end of the
/// slider is already useful (a mild evening setting) and the top end is a
/// proper "flatmate is asleep tomorrow" configuration.
fn chain(intensity: f64) -> Compressor {
    let intensity = intensity.clamp(0.0, 1.0);
    let shaped = intensity * intensity;

    Compressor {
        threshold_db: -14.0 - shaped * 16.0,
        ratio: 1.5 + shaped * 8.5,
        attack_ms: 10.0,
        release_ms: 200.0 + intensity * 200.0,
        knee_db: 9.0,
        makeup_db: shaped * 5.0,
        // Bass goes first — deep rumble is what wakes people up.
        low_shelf_db: -shaped * 10.0,
        // Mud out, presence in: speech gets the window.
        mud_db: -(3.0 * intensity),
        presence_db: shaped * 4.0,
    }
}

/// The parameter set both units are built from.
struct Compressor {
    threshold_db: f64,
    ratio: f64,
    attack_ms: f64,
    release_ms: f64,
    knee_db: f64,
    makeup_db: f64,
    low_shelf_db: f64,
    mud_db: f64,
    presence_db: f64,
}

fn describe(chain: &Compressor) -> String {
    format!(
        "{:.1}:1 · {} Bass · {} Presence",
        chain.ratio,
        fmt_db(chain.low_shelf_db),
        fmt_db(chain.presence_db)
    )
}

fn fmt_db(value: f64) -> String {
    format!("{}{:.0} dB", if value < 0.0 { "" } else { "+" }, value)
}

fn eq_bands(chain: &Compressor) -> Vec<EqBand> {
    vec![
        EqBand {
            id: "rumble".to_string(),
            label: "Rumble".to_string(),
            frequency: 70.0,
            gain_db: chain.low_shelf_db,
            q: 0.7,
            kind: EqBandKind::LowShelf,
        },
        EqBand {
            id: "mud".to_string(),
            label: "Mud".to_string(),
            frequency: 300.0,
            gain_db: chain.mud_db,
            q: 1.1,
            kind: EqBandKind::Peaking,
        },
        EqBand {
            id: "presence".to_string(),
            label: "Presence".to_string(),
            frequency: 2_800.0,
            gain_db: chain.presence_db,
            q: 0.9,
            kind: EqBandKind::Peaking,
        },
    ]
}

impl NightListening {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn on_change(&self, params: &Value) {
        let Ok(change) = serde_json::from_value::<Change>(params.clone()) else {
            return;
        };
        if change.panel_id != PANEL_ID {
            return;
        }
        match change.widget_id.as_str() {
            "intensity" => {
                if let Some(value) = change.value.as_f64() {
                    self.lock().intensity = value.clamp(0.0, 1.0);
                }
            }
            "on" => {
                if let Some(value) = change.value.as_bool() {
                    self.lock().enabled = value;
                }
            }
            _ => {}
        }
    }

    fn build(&self) -> (f64, bool, bool) {
        let state = self.lock();
        (state.intensity, state.enabled, state.playing)
    }

    fn panel(&self) -> Result<Value, PluginError> {
        let (intensity, enabled, playing) = self.build();

        let widgets = vec![
            Widget::Slider {
                id: "intensity".to_string(),
                label: "Intensity".to_string(),
                value: intensity,
                min: 0.0,
                max: 1.0,
                step: 0.05,
                unit: String::new(),
            },
            Widget::Text {
                text: describe(&chain(intensity)),
            },
            Widget::Toggle {
                id: "on".to_string(),
                label: if playing { "On (playing)" } else { "On" }.to_string(),
                value: enabled,
            },
        ];

        serde_json::to_value(PanelContent {
            panel_id: PANEL_ID.to_string(),
            widgets,
        })
        .map_err(|error| PluginError::new("serialize_failed", error.to_string()))
    }

    fn graph(&self) -> Result<Value, PluginError> {
        let (intensity, enabled, playing) = self.build();
        // Nothing to do when the user is not listening; the units drop out of
        // the chain entirely rather than idling in it.
        let enabled = enabled && playing;
        let settings = chain(if enabled { intensity } else { 0.0 });

        serde_json::to_value(AudioGraph {
            units: vec![
                AudioUnit::Compressor {
                    id: COMPRESSOR_ID.to_string(),
                    enabled,
                    threshold_db: settings.threshold_db,
                    ratio: settings.ratio,
                    attack_ms: settings.attack_ms,
                    release_ms: settings.release_ms,
                    knee_db: settings.knee_db,
                    makeup_db: settings.makeup_db,
                },
                AudioUnit::ParametricEq {
                    id: EQ_ID.to_string(),
                    enabled,
                    bands: eq_bands(&settings),
                },
            ],
        })
        .map_err(|error| PluginError::new("serialize_failed", error.to_string()))
    }
}

impl Plugin for NightListening {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: PLUGIN_ID.to_string(),
            name: "Night Listening".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: PROTOCOL_VERSION.to_string(),
            ui_panels: vec![UiPanelDescriptor::now_playing(PANEL_ID, "Night")],
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
            "ui/change" => self.on_change(&params),
            "event/playback-started" | "event/playback-progress" => self.lock().playing = true,
            "event/playback-completed" => self.lock().playing = false,
            _ => {}
        }
    }
}

export_plugin!(NightListening);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intensity_is_clamped() {
        assert_eq!(chain(2.5).ratio, chain(1.0).ratio);
        assert_eq!(chain(-1.0).ratio, chain(0.0).ratio);
    }

    #[test]
    fn more_intensity_is_stronger_everywhere() {
        let soft = chain(0.2);
        let hard = chain(0.9);
        assert!(hard.threshold_db < soft.threshold_db);
        assert!(hard.ratio > soft.ratio);
        assert!(hard.low_shelf_db < soft.low_shelf_db);
        assert!(hard.presence_db > soft.presence_db);
    }

    #[test]
    fn zero_intensity_is_transparent() {
        let idle = chain(0.0);
        assert!(idle.ratio < 2.0);
        assert!(idle.threshold_db > -15.0);
        assert!(idle.low_shelf_db > -2.0);
        assert!(idle.presence_db.abs() < 0.01);
    }

    #[test]
    fn eq_bands_cover_the_whole_speech_range() {
        let bands = eq_bands(&chain(0.5));
        assert_eq!(bands.len(), 3);
        assert_eq!(bands[0].kind, EqBandKind::LowShelf);
        assert!(bands[0].gain_db < 0.0, "rumble is cut");
        assert!(bands[2].gain_db > 0.0, "presence is lifted");
        assert!(bands
            .windows(2)
            .all(|pair| pair[0].frequency < pair[1].frequency));
    }

    #[test]
    fn readout_mentions_ratio_and_bass() {
        let text = describe(&chain(0.5));
        assert!(text.contains(':'), "ratio readout: {text}");
        assert!(text.contains("Bass"), "bass readout: {text}");
    }
}

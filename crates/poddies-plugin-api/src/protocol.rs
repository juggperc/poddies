//! The wire protocol: newline-delimited JSON request/reply envelopes.
//!
//! The channel is symmetric. The host sends requests (`describe`,
//! `ui/panel`, `discovery/list`) and notifications (playback and library
//! events); a plugin may also send requests back (`host/library/shows`,
//! `host/log`). Requests carry an `id` and expect exactly one reply with the
//! same `id`. Notifications omit the `id` and are never answered.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Contract version between host and plugin.
pub const PROTOCOL_VERSION: &str = "1.0";

/// Method names. Strings rather than an enum so a plugin built against a newer
/// minor version can still be understood by an older host when it ignores
/// unknown methods.
pub mod methods {
    // Host -> plugin, expects a reply.
    pub const DESCRIBE: &str = "describe";
    pub const UI_PANEL: &str = "ui/panel";
    pub const DISCOVERY_LIST: &str = "discovery/list";

    // Host -> plugin, notification.
    pub const SHUTDOWN: &str = "shutdown";
    pub const EVENT_PLAYBACK_STARTED: &str = "event/playback-started";
    pub const EVENT_PLAYBACK_PROGRESS: &str = "event/playback-progress";
    pub const EVENT_PLAYBACK_COMPLETED: &str = "event/playback-completed";
    pub const EVENT_LIBRARY_CHANGED: &str = "event/library-changed";

    // Plugin -> host.
    pub const HOST_LIBRARY_SHOWS: &str = "host/library/shows";
    pub const HOST_LIBRARY_HISTORY: &str = "host/library/history";
    pub const HOST_LOG: &str = "host/log";
}

/// A request or notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Envelope {
    pub fn request(id: u64, method: impl Into<String>, params: Value) -> Self {
        Self {
            id: Some(id),
            method: method.into(),
            params,
        }
    }

    pub fn notification(method: impl Into<String>, params: Value) -> Self {
        Self {
            id: None,
            method: method.into(),
            params,
        }
    }

    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// A reply to a request, or an unsolicited error notification when `id` is
/// `None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reply {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<PluginError>,
}

impl Reply {
    pub fn ok(id: Option<u64>, result: Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn failed(id: Option<u64>, code: &str, message: impl Into<String>) -> Self {
        Self {
            id,
            result: None,
            error: Some(PluginError::new(code, message)),
        }
    }

    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }
}

/// A structured failure. `code` is a stable, machine-readable token such as
/// `unsupported_method` or `invalid_params`; `message` is for humans and is
/// shown verbatim in the host's plugin log.
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct PluginError {
    pub code: String,
    pub message: String,
}

impl PluginError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }

    pub fn unsupported(method: &str) -> Self {
        Self::new("unsupported_method", format!("plugin does not handle '{method}'"))
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new("invalid_params", message)
    }
}

/// Parameters for `discovery/list`. The plugin only proposes candidates; the
/// host owns the ranking, so weights are not passed across the boundary.
///
/// `topics` is the host's summary of what the listener actually listens to
/// (their highest-weighted categories), so a source can fetch *relevant*
/// candidates without being trusted to rank them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryRequest {
    pub limit: usize,
    #[serde(default)]
    pub topics: Vec<String>,
}

/// Result of `discovery/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryResponse {
    pub candidates: Vec<crate::DiscoveryCandidate>,
}

/// Parameters for `ui/panel`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelRequest {
    pub panel_id: String,
}

/// A playback event delivered to plugins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackEvent {
    pub episode_id: String,
    pub show_id: String,
    pub title: String,
    pub position_secs: f64,
    pub duration_secs: Option<u64>,
}

/// Parameters for a plugin's `host/log` call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogRequest {
    pub level: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn notifications_omit_the_id() {
        let wire = serde_json::to_string(&Envelope::notification("shutdown", json!({}))).unwrap();
        assert!(!wire.contains("\"id\""));
        assert!(serde_json::from_str::<Envelope>(&wire).unwrap().is_notification());
    }

    #[test]
    fn replies_round_trip() {
        let reply = Reply::ok(Some(7), json!({"ok": true}));
        let wire = serde_json::to_string(&reply).unwrap();
        let parsed: Reply = serde_json::from_str(&wire).unwrap();
        assert_eq!(parsed.id, Some(7));
        assert!(!parsed.is_error());

        let failed = Reply::failed(Some(7), "boom", "it broke");
        assert!(serde_json::from_str::<Reply>(&serde_json::to_string(&failed).unwrap())
            .unwrap()
            .is_error());
    }
}

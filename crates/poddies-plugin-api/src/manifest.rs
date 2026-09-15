//! `plugin.json`: what a plugin declares about itself.

use serde::{Deserialize, Serialize};

use crate::protocol::PROTOCOL_VERSION;

/// A plugin's self-description, read from `plugin.json` in its directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Reverse-DNS-ish unique id, e.g. `dev.poddies.stats`.
    pub id: String,
    pub name: String,
    pub version: String,
    /// Protocol version this plugin was built against, e.g. `"1.0"`.
    pub protocol: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    /// What the plugin is allowed to do. The host enforces this list.
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    pub runtime: Runtime,
}

impl PluginManifest {
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    fn protocol_major(&self) -> Option<u32> {
        self.protocol.split('.').next()?.trim().parse().ok()
    }

    /// The host loads only plugins whose major protocol version matches.
    pub fn is_compatible(&self) -> bool {
        self.protocol_major() == Some(host_major())
    }

    pub fn has(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

/// Major version of the protocol this build of the API speaks.
pub fn host_major() -> u32 {
    PROTOCOL_VERSION
        .split('.')
        .next()
        .and_then(|major| major.parse().ok())
        .unwrap_or(0)
}

/// What a running plugin reports back for `describe`. This is the manifest the
/// plugin exposes at runtime, which may narrow (never widen) what it declared.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub protocol: String,
    #[serde(default)]
    pub ui_panels: Vec<crate::ui::UiPanelDescriptor>,
}

/// How the worker should start the plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Runtime {
    /// A native dynamic library exporting the [`crate::abi`] entry point.
    /// `library` is a path relative to the plugin directory.
    Native { library: String },
    /// A Python entry file, run with the bundled Python runtime.
    Python { entry: String },
}

/// Declared capabilities. Requesting a method outside these is refused by the
/// host before the plugin ever sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    /// Contributes panels to the UI.
    UiPanel,
    /// Supplies candidate shows to the discovery queue.
    DiscoverySource,
    /// Receives playback lifecycle events.
    PlaybackHook,
    /// May place units in the playback audio chain.
    AudioEffects,
    /// May query the library.
    LibraryRead,
    /// May write to the library. Not granted to reference plugins.
    LibraryWrite,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_native_manifest() {
        let manifest = PluginManifest::parse(
            r#"{
                "id": "dev.poddies.stats",
                "name": "Listening Stats",
                "version": "0.1.0",
                "protocol": "1.0",
                "capabilities": ["ui-panel", "library-read"],
                "runtime": { "kind": "native", "library": "poddies_plugin_stats.dll" }
            }"#,
        )
        .unwrap();

        assert!(manifest.is_compatible());
        assert!(manifest.has(Capability::UiPanel));
        assert!(!manifest.has(Capability::DiscoverySource));
        assert!(matches!(manifest.runtime, Runtime::Native { .. }));
    }

    #[test]
    fn rejects_a_foreign_major_version() {
        let manifest = PluginManifest::parse(
            r#"{
                "id": "x", "name": "x", "version": "1", "protocol": "2.0",
                "runtime": { "kind": "python", "entry": "main.py" }
            }"#,
        )
        .unwrap();
        assert!(!manifest.is_compatible());
    }
}

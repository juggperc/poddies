//! The Poddies plugin API.
//!
//! # How plugins run
//!
//! Plugins never execute inside the host process. The host spawns a short-lived
//! **worker** (`poddies.exe --plugin-worker <dir>`) and talks to it over
//! newline-delimited JSON on stdio. A native plugin is a `cdylib` the worker
//! loads with the C ABI in [`abi`]; a Python plugin is a script the worker
//! runs. Either way the plugin lives in its own process, so a panic, hang or
//! hard crash takes down the worker and nothing else — the host restarts it.
//!
//! # Stability
//!
//! [`PROTOCOL_VERSION`] is the contract version. The host refuses to load a
//! plugin whose major version differs, and warns on a minor mismatch. Additive
//! changes bump the minor version; anything that changes an existing message's
//! meaning bumps the major.
//!
//! [`abi::ABI_VERSION`] versions the in-process C ABI separately, because it
//! can only change when the DLL loading contract changes.

pub mod abi;
pub mod library;
pub mod manifest;
pub mod protocol;
pub mod ui;

pub use library::{HistoryEntry, ShowSummary};
pub use manifest::{Capability, PluginInfo, PluginManifest, Runtime};
pub use protocol::{AudioGraph, AudioUnit, Envelope, PluginError, Reply, WidgetChange, PROTOCOL_VERSION};
pub use ui::{
    EqBand, EqBandKind, KnobStyle, MeterSource, PanelContent, Placement, UiPanelDescriptor, Widget,
};

/// Re-exported so plugins depend on exactly the same domain types the host
/// uses, and never on a divergent copy.
pub use poddies_core::discovery::{DiscoveryCandidate, ListeningProfile, Weights};
pub use poddies_core::model::{Episode, PlaybackState, Show};

/// The crate version, useful in handshakes and logs.
pub const API_VERSION: &str = env!("CARGO_PKG_VERSION");

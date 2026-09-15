//! The plugin host: a sandboxed worker and the manager that drives it.
//!
//! See [`crate::manager`] for the host side and [`crate::worker`] for the child
//! side. The guarantee this crate provides: **a misbehaving plugin cannot crash
//! the host.** Plugins run in a separate process, calls are bounded by a
//! timeout, and on Windows the worker is placed in a Job Object that caps its
//! memory and guarantees it dies with the host.

pub mod manager;
pub mod sandbox;
pub mod worker;

pub use manager::{
    HostServices, LoadReport, LoadedPlugin, NoHostServices, PanelEntry, PluginHost, WorkerLauncher,
    DEFAULT_MEMORY_LIMIT, DEFAULT_REQUEST_TIMEOUT,
};
pub use worker::run_worker;

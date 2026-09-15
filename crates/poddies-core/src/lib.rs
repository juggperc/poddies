//! Poddies core: feed parsing, library storage, playback state and the
//! transparent discovery ranking algorithm.
//!
//! This crate is deliberately free of UI and platform dependencies so the same
//! logic backs the desktop host, the CLI and every plugin.

pub mod discovery;
pub mod error;
pub mod feed;
pub mod fetch;
pub mod itunes;
pub mod library;
pub mod model;
pub mod util;

pub use error::{CoreError, Result};

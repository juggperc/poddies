//! The native (DLL) ABI.
//!
//! A native plugin is a `cdylib` exporting [`ENTRY_SYMBOL`], which the worker
//! loads with `libloading` **inside its own process**. The DLL can be written in
//! any language that can export a C function and handle a JSON string.
//!
//! Two directions, one mechanism each:
//!
//! * **host → plugin**: [`PluginVTable::handle_line`]. The worker feeds it one
//!   request line and writes back the reply line.
//! * **plugin → host**: [`HostApi::call`], handed to the plugin at load time.
//!   Passing a request with an `id` gets a reply; passing one without is a
//!   fire-and-forget notification (this is how a plugin logs).
//!
//! Because everything crosses as JSON, the ABI itself rarely has to change —
//! the protocol evolves independently.

use std::ffi::c_char;

/// Version of this in-process ABI. Bump only when the loading contract itself
/// changes; ordinary feature work goes through the JSON protocol instead.
///
/// * v1 — `handle_line` / `free_string` / `shutdown`.
/// * v2 — adds [`HostApi`], so native plugins can call the host.
pub const ABI_VERSION: u32 = 2;

/// The symbol a native plugin must export.
pub const ENTRY_SYMBOL: &str = "poddies_plugin_init";

/// A synchronous call from a plugin to the host. Takes a NUL-terminated JSON
/// request line and returns a newly-allocated NUL-terminated JSON reply line
/// (or null if the host link is gone). Free the result with
/// [`HostApi::free_string`].
pub type HostCallFn = extern "C" fn(*const c_char) -> *mut c_char;

/// Services the worker provides to a native plugin.
#[repr(C)]
pub struct HostApi {
    pub call: HostCallFn,
    /// Free a string previously returned by `call`.
    pub free_string: extern "C" fn(*mut c_char),
}

/// The table a native plugin hands back. `#[repr(C)]` so it has a stable layout
/// across compilers and languages.
#[repr(C)]
pub struct PluginVTable {
    /// Must equal [`ABI_VERSION`] or the worker refuses to use the plugin.
    pub abi_version: u32,
    /// Handle one request line (NUL-terminated). Returns a newly-allocated
    /// NUL-terminated reply line, or null to signal a failure the worker turns
    /// into an error reply. Ownership passes to the caller, who frees it via
    /// `free_string`.
    pub handle_line: extern "C" fn(*const c_char) -> *mut c_char,
    /// Free a string previously returned by `handle_line`.
    pub free_string: extern "C" fn(*mut c_char),
    /// Called once before the worker exits, so the plugin can flush state.
    pub shutdown: extern "C" fn(),
}

/// Signature of a plugin's entry point.
pub type PluginInit = extern "C" fn(*const HostApi) -> *const PluginVTable;

//! Authoring SDK for Poddies plugins.
//!
//! Implement [`Plugin`], then either call [`run`] (for a plugin built as a
//! standalone worker target) or invoke the [`export_plugin!`] macro to ship a
//! native DLL the host can load.
//!
//! ```ignore
//! use poddies_plugin_sdk::{export_plugin, Plugin, PluginError, PluginInfo};
//! use serde_json::Value;
//!
//! #[derive(Default)]
//! struct Hello;
//!
//! impl Plugin for Hello {
//!     fn info(&self) -> PluginInfo {
//!         PluginInfo {
//!             id: "dev.example.hello".into(),
//!             name: "Hello".into(),
//!             version: env!("CARGO_PKG_VERSION").into(),
//!             protocol: poddies_plugin_api::PROTOCOL_VERSION.into(),
//!             ui_panels: Vec::new(),
//!         }
//!     }
//!
//!     fn on_request(&mut self, method: &str, _params: Value) -> Result<Value, PluginError> {
//!         Err(PluginError::unsupported(method))
//!     }
//! }
//!
//! export_plugin!(Hello);
//! ```

use std::ffi::{CStr, CString};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicPtr, Ordering};

use serde_json::Value;

use poddies_plugin_api::protocol::{Envelope, Reply, methods};

pub use poddies_plugin_api as api;
pub use poddies_plugin_api::abi::{ABI_VERSION, HostApi, PluginVTable};
pub use poddies_plugin_api::protocol::PROTOCOL_VERSION;
pub use poddies_plugin_api::ui::{ListItem, PanelContent, UiPanelDescriptor, Widget};
pub use poddies_plugin_api::{Capability, PluginError, PluginInfo};
pub use serde_json::json;

/// A plugin. Implement this and you are done — the SDK owns the transport.
pub trait Plugin: Default {
    /// Static description, returned for the `describe` request.
    fn info(&self) -> PluginInfo;

    /// Handle a request. Return the result payload, or a [`PluginError`].
    /// Notifications never reach here; use [`Plugin::on_notification`].
    fn on_request(&mut self, method: &str, params: Value) -> Result<Value, PluginError>;

    /// Handle a fire-and-forget event. Default: ignore.
    fn on_notification(&mut self, _method: &str, _params: Value) {}

    /// Flush any state before the worker exits. Default: nothing to do.
    fn on_shutdown(&mut self) {}
}

/// The host link, set once by the worker when it loads a native plugin.
static HOST_API: AtomicPtr<HostApi> = AtomicPtr::new(std::ptr::null_mut());

/// Called by the generated entry point. Not part of the authoring surface.
pub fn set_host_api(api: *const HostApi) {
    HOST_API.store(api as *mut HostApi, Ordering::SeqCst);
}

fn host_api() -> Option<&'static HostApi> {
    let pointer = HOST_API.load(Ordering::SeqCst);
    // SAFETY: the worker owns the HostApi for the lifetime of the plugin.
    (!pointer.is_null()).then(|| unsafe { &*pointer })
}

/// Ask the host something and wait for the answer, e.g.
/// `host_call("host/library/shows", json!({}))`.
///
/// Returns `host_unavailable` when the plugin is not running inside a worker
/// (for example in a unit test).
pub fn host_call(method: &str, params: Value) -> Result<Value, PluginError> {
    send_to_host(Envelope::request(0, method, params), true)
}

/// Tell the host something without waiting. Used for logging.
pub fn host_notify(method: &str, params: Value) -> Result<(), PluginError> {
    send_to_host(Envelope::notification(method, params), false).map(|_| ())
}

/// Write a line to the host's plugin log.
pub fn log(level: &str, message: &str) {
    let _ = host_notify("host/log", json!({ "level": level, "message": message }));
}

fn send_to_host(envelope: Envelope, expects_reply: bool) -> Result<Value, PluginError> {
    let api = host_api().ok_or_else(|| {
        PluginError::new(
            "host_unavailable",
            "plugin is not running inside a Poddies worker",
        )
    })?;

    let line = serde_json::to_string(&envelope)
        .map_err(|err| PluginError::new("serialize_failed", err.to_string()))?;
    let request =
        CString::new(line).map_err(|err| PluginError::new("bad_request", err.to_string()))?;

    let reply_ptr = (api.call)(request.as_ptr());
    if reply_ptr.is_null() {
        if expects_reply {
            return Err(PluginError::new("host_unavailable", "host link closed"));
        }
        return Ok(Value::Null);
    }

    if !expects_reply {
        (api.free_string)(reply_ptr);
        return Ok(Value::Null);
    }

    // SAFETY: the worker returned a valid NUL-terminated string we now own.
    let text = unsafe { CStr::from_ptr(reply_ptr) }
        .to_string_lossy()
        .into_owned();
    (api.free_string)(reply_ptr);

    let reply: Reply = serde_json::from_str(&text)
        .map_err(|err| PluginError::new("bad_response", err.to_string()))?;
    match reply.error {
        Some(error) => Err(error),
        None => Ok(reply.result.unwrap_or(Value::Null)),
    }
}

/// Extract the request id from a wire line, used when reporting a handler
/// panic so the host can match the error reply to its pending call.
///
/// `#[doc(hidden)]` because the `export_plugin!` macro (which expands in
/// downstream crates) needs it via `$crate::`; it is not authoring surface.
#[doc(hidden)]
pub fn plugin_call_id(line: &str) -> Option<u64> {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| value.get("id").and_then(Value::as_u64))
}

/// Handle one wire line, returning the line to write back (or `None` when the
/// input was a notification and no reply is expected).
///
/// Malformed input is turned into an error reply rather than a panic: a broken
/// request must never take the plugin down.
pub fn dispatch<P: Plugin>(plugin: &mut P, line: &str) -> Option<String> {
    let envelope: Envelope = match serde_json::from_str(line) {
        Ok(envelope) => envelope,
        Err(err) => {
            let reply = Reply::failed(None, "bad_request", err.to_string());
            return serde_json::to_string(&reply).ok();
        }
    };

    let Some(id) = envelope.id else {
        if envelope.method == methods::SHUTDOWN {
            plugin.on_shutdown();
        } else {
            plugin.on_notification(&envelope.method, envelope.params);
        }
        return None;
    };

    let reply = match envelope.method.as_str() {
        methods::DESCRIBE => match serde_json::to_value(plugin.info()) {
            Ok(info) => Reply::ok(Some(id), info),
            Err(err) => Reply::failed(Some(id), "serialize_failed", err.to_string()),
        },
        methods::SHUTDOWN => {
            plugin.on_shutdown();
            Reply::ok(Some(id), Value::Null)
        }
        method => match plugin.on_request(method, envelope.params) {
            Ok(result) => Reply::ok(Some(id), result),
            Err(error) => Reply::failed(Some(id), &error.code, error.message),
        },
    };

    serde_json::to_string(&reply).ok()
}

/// Run a plugin over stdio until the host closes the pipe. Used when a plugin
/// is built as its own worker binary rather than a loaded DLL.
pub fn run<P: Plugin>(mut plugin: P) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = dispatch(&mut plugin, &line) {
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Export a [`Plugin`] as a native DLL the host can load.
///
/// The plugin type must be `Default`. State is held in a mutex, so the plugin
/// is safe to call from the worker's single dispatch thread.
#[macro_export]
macro_rules! export_plugin {
    ($plugin:ty) => {
        static PODDIES_PLUGIN: ::std::sync::Mutex<Option<$plugin>> = ::std::sync::Mutex::new(None);

        extern "C" fn poddies_handle_line(
            request: *const ::std::os::raw::c_char,
        ) -> *mut ::std::os::raw::c_char {
            if request.is_null() {
                return ::std::ptr::null_mut();
            }
            // SAFETY: the worker passes a valid NUL-terminated string that
            // stays alive for the duration of this call.
            let line = match unsafe { ::std::ffi::CStr::from_ptr(request) }.to_str() {
                Ok(line) => line.to_owned(),
                Err(_) => return ::std::ptr::null_mut(),
            };

            // A panic escaping `extern "C"` would abort the worker process.
            // Catch it here and report it as a protocol error instead, so one
            // buggy handler costs one failed call, not the whole plugin.
            let caught = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                let mut guard = match PODDIES_PLUGIN.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                let plugin = guard.get_or_insert_with(<$plugin>::default);
                $crate::dispatch(plugin, &line)
            }));

            let response = match caught {
                ::std::result::Result::Ok(response) => response,
                ::std::result::Result::Err(_) => {
                    let reply = $crate::api::protocol::Reply::failed(
                        $crate::plugin_call_id(&line),
                        "plugin_error",
                        "plugin handler panicked",
                    );
                    ::serde_json::to_string(&reply).ok()
                }
            };

            let Some(response) = response else {
                return ::std::ptr::null_mut();
            };
            match ::std::ffi::CString::new(response) {
                Ok(response) => response.into_raw(),
                Err(_) => ::std::ptr::null_mut(),
            }
        }

        extern "C" fn poddies_free_string(pointer: *mut ::std::os::raw::c_char) {
            if pointer.is_null() {
                return;
            }
            // SAFETY: the pointer came from `CString::into_raw` above.
            unsafe { drop(::std::ffi::CString::from_raw(pointer)) };
        }

        extern "C" fn poddies_shutdown() {
            if let Ok(mut guard) = PODDIES_PLUGIN.lock() {
                if let Some(plugin) = guard.as_mut() {
                    plugin.on_shutdown();
                }
                *guard = None;
            }
        }

        /// Native plugin entry point. See [`poddies_plugin_api::abi`].
        #[unsafe(no_mangle)]
        pub extern "C" fn poddies_plugin_init(
            api: *const $crate::HostApi,
        ) -> *const $crate::PluginVTable {
            $crate::set_host_api(api);
            static VTABLE: $crate::PluginVTable = $crate::PluginVTable {
                abi_version: $crate::ABI_VERSION,
                handle_line: poddies_handle_line,
                free_string: poddies_free_string,
                shutdown: poddies_shutdown,
            };
            &VTABLE
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Default)]
    struct Echo;

    impl Plugin for Echo {
        fn info(&self) -> PluginInfo {
            PluginInfo {
                id: "dev.test.echo".to_string(),
                name: "Echo".to_string(),
                version: "0.0.1".to_string(),
                protocol: PROTOCOL_VERSION.to_string(),
                ui_panels: Vec::new(),
            }
        }

        fn on_request(&mut self, method: &str, params: Value) -> Result<Value, PluginError> {
            match method {
                "echo" => Ok(params),
                other => Err(PluginError::unsupported(other)),
            }
        }
    }

    #[test]
    fn describe_returns_plugin_info() {
        let response = dispatch(&mut Echo, r#"{"id":1,"method":"describe"}"#).unwrap();
        let reply: Reply = serde_json::from_str(&response).unwrap();
        assert_eq!(reply.id, Some(1));
        assert_eq!(reply.result.unwrap()["name"], "Echo");
    }

    #[test]
    fn unknown_method_becomes_an_error_reply() {
        let response = dispatch(&mut Echo, r#"{"id":2,"method":"nope"}"#).unwrap();
        let reply: Reply = serde_json::from_str(&response).unwrap();
        assert_eq!(reply.error.unwrap().code, "unsupported_method");
    }

    #[test]
    fn notifications_produce_no_reply() {
        assert!(dispatch(&mut Echo, r#"{"method":"event/playback-started"}"#).is_none());
    }

    #[test]
    fn malformed_input_does_not_panic() {
        let response = dispatch(&mut Echo, "this is not json").unwrap();
        let reply: Reply = serde_json::from_str(&response).unwrap();
        assert_eq!(reply.error.unwrap().code, "bad_request");
    }

    #[test]
    fn echo_round_trips_params() {
        let response = dispatch(&mut Echo, r#"{"id":3,"method":"echo","params":{"a":1}}"#).unwrap();
        let reply: Reply = serde_json::from_str(&response).unwrap();
        assert_eq!(reply.result.unwrap(), json!({"a": 1}));
    }
}

//! The worker: the child process that actually hosts a plugin.
//!
//! Invoked as `poddies.exe --plugin-worker <plugin-dir>` (or via the standalone
//! `poddies-plugin-worker` binary). It loads the plugin, then pumps JSON lines
//! between the host on stdin/stdout and the plugin. Stdout carries only
//! protocol traffic; everything else goes to stderr.

use std::ffi::{CStr, CString};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use libloading::{Library, Symbol};
use serde_json::Value;

use poddies_plugin_api::abi::{HostApi, PluginVTable, ABI_VERSION, ENTRY_SYMBOL};
use poddies_plugin_api::manifest::{PluginManifest, Runtime};
use poddies_plugin_api::protocol::{Envelope, Reply};

/// Run a plugin until the host closes stdin. Returns an error description if
/// the plugin could not be started at all.
pub fn run_worker(plugin_dir: &Path) -> Result<(), String> {
    let manifest = load_manifest(plugin_dir)?;

    if !manifest.is_compatible() {
        return Err(format!(
            "plugin '{}' speaks protocol {} but this host speaks {}",
            manifest.id,
            manifest.protocol,
            poddies_plugin_api::PROTOCOL_VERSION
        ));
    }

    match manifest.runtime.clone() {
        Runtime::Native { library } => run_native(&plugin_dir.join(library)),
        Runtime::Python { entry } => run_python(&plugin_dir.join(entry)),
    }
}

/// Read and parse `plugin.json` from a plugin directory.
pub fn load_manifest(plugin_dir: &Path) -> Result<PluginManifest, String> {
    let path = plugin_dir.join("plugin.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    PluginManifest::parse(&text).map_err(|err| format!("invalid manifest: {err}"))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The worker's stdio, shared between the main loop and a plugin's outbound
/// calls. Locked only for the duration of a single read or write, never held
/// across a plugin call, so a plugin can call the host from inside a handler.
struct HostLink {
    reader: Mutex<BufReader<std::io::Stdin>>,
    writer: Mutex<std::io::Stdout>,
    next_id: AtomicU64,
}

static HOST_LINK: OnceLock<HostLink> = OnceLock::new();

fn read_line(link: &HostLink, buffer: &mut String) -> std::io::Result<usize> {
    lock(&link.reader).read_line(buffer)
}

fn write_line(link: &HostLink, line: &str) -> std::io::Result<()> {
    let mut writer = lock(&link.writer);
    writeln!(writer, "{line}")?;
    writer.flush()
}

/// Host-call implementation handed to native plugins. A request with an `id`
/// waits for its reply; a request without one is fire-and-forget.
extern "C" fn host_call(request: *const std::ffi::c_char) -> *mut std::ffi::c_char {
    let Some(link) = HOST_LINK.get() else {
        return std::ptr::null_mut();
    };
    if request.is_null() {
        return std::ptr::null_mut();
    }

    // SAFETY: the plugin passes a NUL-terminated string valid for this call.
    let text = unsafe { CStr::from_ptr(request) }.to_string_lossy().into_owned();
    let Ok(mut envelope) = serde_json::from_str::<Envelope>(&text) else {
        return std::ptr::null_mut();
    };

    let reply_id = if envelope.id.is_some() {
        Some(link.next_id.fetch_add(1, Ordering::SeqCst))
    } else {
        None
    };
    envelope.id = reply_id;

    let Ok(line) = serde_json::to_string(&envelope) else {
        return std::ptr::null_mut();
    };
    if write_line(link, &line).is_err() {
        return std::ptr::null_mut();
    }

    let Some(reply_id) = reply_id else {
        return std::ptr::null_mut();
    };

    loop {
        let mut buffer = String::new();
        match read_line(link, &mut buffer) {
            Ok(0) | Err(_) => return std::ptr::null_mut(),
            Ok(_) => {}
        }
        let trimmed = buffer.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Skip anything that is not our reply, e.g. a notification racing us.
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        if value.get("method").is_some() {
            continue;
        }
        let Ok(reply) = serde_json::from_value::<Reply>(value) else {
            continue;
        };
        if reply.id != Some(reply_id) {
            continue;
        }
        return match CString::new(trimmed) {
            Ok(value) => value.into_raw(),
            Err(_) => std::ptr::null_mut(),
        };
    }
}

extern "C" fn host_free_string(pointer: *mut std::ffi::c_char) {
    if pointer.is_null() {
        return;
    }
    // SAFETY: the pointer came from `CString::into_raw` in `host_call`.
    unsafe { drop(CString::from_raw(pointer)) };
}

fn run_native(library_path: &Path) -> Result<(), String> {
    HOST_LINK
        .set(HostLink {
            reader: Mutex::new(BufReader::new(std::io::stdin())),
            writer: Mutex::new(std::io::stdout()),
            next_id: AtomicU64::new(10_000_000),
        })
        .map_err(|_| "worker stdio already initialised".to_string())?;
    let link = HOST_LINK.get().expect("host link was just set");

    let api = HostApi {
        call: host_call,
        free_string: host_free_string,
    };

    // SAFETY: loading a DLL runs its initialisers. This is exactly why the
    // worker is a disposable process: the host never loads untrusted code into
    // itself, so a bad plugin can only take down a process we can restart.
    unsafe {
        let library = Library::new(library_path)
            .map_err(|err| format!("cannot load {}: {err}", library_path.display()))?;

        let entry: Symbol<extern "C" fn(*const HostApi) -> *const PluginVTable> = library
            .get(ENTRY_SYMBOL.as_bytes())
            .map_err(|err| format!("missing '{ENTRY_SYMBOL}': {err}"))?;

        let vtable_ptr = entry(&api);
        if vtable_ptr.is_null() {
            return Err("plugin returned a null vtable".to_string());
        }
        let vtable = &*vtable_ptr;
        if vtable.abi_version != ABI_VERSION {
            return Err(format!(
                "ABI mismatch: plugin {:#x}, host {:#x}",
                vtable.abi_version, ABI_VERSION
            ));
        }

        loop {
            let mut buffer = String::new();
            match read_line(link, &mut buffer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(err) => return Err(err.to_string()),
            }

            let line = buffer.trim_end();
            if line.trim().is_empty() {
                continue;
            }
            let request = match CString::new(line) {
                Ok(request) => request,
                Err(_) => continue,
            };

            let response = (vtable.handle_line)(request.as_ptr());
            if response.is_null() {
                continue;
            }
            let text = CStr::from_ptr(response).to_string_lossy().into_owned();
            (vtable.free_string)(response);

            write_line(link, &text).map_err(|err| err.to_string())?;
        }

        (vtable.shutdown)();
        // `library` stays in scope until here so the vtable and any plugin
        // statics remain mapped for the whole session.
        drop(library);
    }
    Ok(())
}

/// Relay stdio to a Python plugin. The interpreter is a plain child; the worker
/// is a transparent pipe, so the protocol is identical to a native plugin and
/// the Python side can perform host calls directly.
fn run_python(entry: &Path) -> Result<(), String> {
    let (program, prefix_args) =
        find_python().ok_or_else(|| "no Python interpreter found on PATH".to_string())?;

    let mut command = Command::new(&program);
    command
        .args(&prefix_args)
        .arg("-u")
        .arg(entry)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    if let Some(sdk) = locate_python_sdk() {
        command.env("PYTHONPATH", sdk);
    }

    let mut child = command
        .spawn()
        .map_err(|err| format!("cannot start {program}: {err}"))?;

    let mut child_stdin = child.stdin.take().ok_or("python stdin unavailable")?;
    let child_stdout = child.stdout.take().ok_or("python stdout unavailable")?;

    let relay = std::thread::spawn(move || {
        let mut reader = BufReader::new(child_stdout);
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            let _ = out.write_all(line.as_bytes());
            let _ = out.flush();
            line.clear();
        }
    });

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if writeln!(child_stdin, "{line}")
            .and_then(|_| child_stdin.flush())
            .is_err()
        {
            break;
        }
    }

    drop(child_stdin);
    let _ = child.wait();
    let _ = relay.join();
    Ok(())
}

/// Probe for a usable interpreter. A real `python` install is preferred; the
/// Windows `py` launcher is the fallback and adds an extra process hop, which
/// is why the job object's process limit leaves room for it.
fn find_python() -> Option<(String, Vec<String>)> {
    let candidates: [(&str, &[&str]); 2] = [("python", &[]), ("py", &["-3"])];

    for (program, args) in candidates {
        let ok = Command::new(program)
            .args(args)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if ok {
            return Some((
                program.to_string(),
                args.iter().map(|arg| arg.to_string()).collect(),
            ));
        }
    }
    None
}

/// Find the bundled Python SDK (`poddies/` package) by walking up from the
/// worker binary. In a release layout it sits next to the executable.
fn locate_python_sdk() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?;
    for _ in 0..4 {
        let candidate = dir.join("python");
        if candidate.join("poddies").is_dir() {
            return Some(candidate);
        }
        dir = dir.parent()?;
    }
    None
}

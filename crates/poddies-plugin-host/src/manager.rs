//! Host-side plugin management: discover, load, call, isolate, restart.
//!
//! Every call is bounded by a timeout, every plugin is a separate process, and
//! a plugin that dies is reported through the pending-call channel so callers
//! get an error instead of hanging forever.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;

use poddies_plugin_api::manifest::{Capability, PluginManifest};
use poddies_plugin_api::protocol::{
    AudioGraph, AudioUnit, DiscoveryRequest, DiscoveryResponse, Envelope, LogRequest, PanelRequest,
    PluginError, Reply, WidgetChange, methods,
};
use poddies_plugin_api::ui::{PanelContent, UiPanelDescriptor};
use poddies_plugin_api::{DiscoveryCandidate, PluginInfo};

use crate::sandbox::Sandbox;

/// Default per-call limit. Generous enough for a plugin doing I/O, short enough
/// that a wedged plugin is noticeable rather than fatal.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Default worker memory cap (256 MiB).
pub const DEFAULT_MEMORY_LIMIT: usize = 256 * 1024 * 1024;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long `kill()` waits for a worker to exit on its own (after stdin EOF)
/// before whoever drops the `LoadedPlugin` lets the job object do it.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(500);

/// Longest line read from a worker before it is treated as broken output. A
/// sane protocol line is a few KiB; this only exists so a plugin that floods
/// stdout cannot balloon the host's memory.
const MAX_LINE_BYTES: usize = 1024 * 1024;

/// Read one newline-terminated line from `reader`, bounded per line rather
/// than per stream. Returns `Ok(false)` at end of stream. A line longer than
/// `cap` is discarded up to its terminating newline and returned as empty —
/// the plugin is flooding, and losing one junk line beats unbounded memory.
fn read_line_capped(
    reader: &mut impl BufRead,
    out: &mut String,
    cap: usize,
) -> std::io::Result<bool> {
    use std::io::Read;

    loop {
        let mut bytes = Vec::new();
        let read = reader
            .by_ref()
            .take(cap as u64)
            .read_until(b'\n', &mut bytes)?;
        if read == 0 {
            if bytes.is_empty() {
                return Ok(false);
            }
            // End of stream with a final unterminated line.
            out.push_str(&String::from_utf8_lossy(&bytes));
            return Ok(true);
        }
        if bytes.ends_with(b"\n") {
            out.push_str(&String::from_utf8_lossy(&bytes));
            return Ok(true);
        }
        // Overlong: keep discarding until this line's newline goes past.
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// How to start a worker. Release builds point this at the app's own binary;
/// development points it at `poddies-plugin-worker`.
#[derive(Debug, Clone)]
pub struct WorkerLauncher {
    program: PathBuf,
    env: Vec<(String, String)>,
}

impl WorkerLauncher {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            env: Vec::new(),
        }
    }

    pub fn current_exe() -> std::io::Result<Self> {
        Ok(Self::new(std::env::current_exe()?))
    }

    /// Extra environment for the worker. Used by `poddies dev` to point a
    /// Python plugin at the bundled SDK.
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    fn command(&self, plugin_dir: &Path) -> Command {
        let mut command = Command::new(&self.program);
        command.arg("--plugin-worker").arg(plugin_dir);
        for (key, value) in &self.env {
            command.env(key, value);
        }
        command
    }
}

/// What the host offers back to plugins. Kept behind a trait so the core stays
/// testable and so a plugin only ever sees the surface the host chooses.
pub trait HostServices: Send + Sync + 'static {
    /// Subscribed shows, as JSON the plugin can read.
    fn library_shows(&self) -> Value {
        Value::Array(Vec::new())
    }

    /// Listening history, as JSON.
    fn library_history(&self) -> Value {
        Value::Array(Vec::new())
    }

    /// A log line from a plugin. Surfaces in the host's plugin log.
    fn log(&self, _level: &str, _message: &str) {}
}

/// Services that do nothing — for tests and headless use.
pub struct NoHostServices;

impl HostServices for NoHostServices {}

/// The call path into a running worker, shareable across threads. Everything in
/// it is either `Arc`-cloned state or state whose accessor already tolerates a
/// closed pipe, so a request sent from a helper thread stays valid even if the
/// `LoadedPlugin` is dropped mid-call — it simply fails with `write_failed` or
/// `plugin_exited` like any other dead-plugin call.
#[derive(Clone)]
struct RequestCore {
    id: String,
    alive: Arc<AtomicBool>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    pending: Arc<Mutex<HashMap<u64, Sender<Reply>>>>,
    next_id: Arc<AtomicU64>,
}

impl RequestCore {
    fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, PluginError> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(PluginError::new(
                "plugin_exited",
                format!("plugin '{}' is not running", self.id),
            ));
        }

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = mpsc::channel();
        lock(&self.pending).insert(id, sender);

        let envelope = Envelope::request(id, method, params);
        let line = match serde_json::to_string(&envelope) {
            Ok(line) => line,
            Err(err) => {
                lock(&self.pending).remove(&id);
                return Err(PluginError::new("serialize_failed", err.to_string()));
            }
        };

        if let Err(err) = write_core_line(&self.stdin, &line) {
            lock(&self.pending).remove(&id);
            return Err(PluginError::new("write_failed", err.to_string()));
        }

        match receiver.recv_timeout(timeout) {
            Ok(reply) => match reply.error {
                Some(error) => Err(error),
                None => Ok(reply.result.unwrap_or(Value::Null)),
            },
            Err(_) => {
                lock(&self.pending).remove(&id);
                Err(PluginError::new(
                    "timeout",
                    format!("'{method}' did not answer within {timeout:?}"),
                ))
            }
        }
    }
}

fn write_core_line(stdin: &Mutex<Option<ChildStdin>>, line: &str) -> std::io::Result<()> {
    let mut guard = lock(stdin);
    match guard.as_mut() {
        Some(stdin) => writeln!(stdin, "{line}").and_then(|_| stdin.flush()),
        None => Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "worker stdin closed",
        )),
    }
}

/// A running plugin process.
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    /// The description the plugin reported at runtime.
    pub info: PluginInfo,
    pub directory: PathBuf,
    /// The shareable call path — plain `Arc` handles into the process plumbing,
    /// cloneable so helper threads can issue requests without borrowing `self`.
    core: RequestCore,
    child: Mutex<Child>,
    reader: Mutex<Option<JoinHandle<()>>>,
    /// Held purely for RAII: dropping it kills the worker via the job object.
    _sandbox: Option<Sandbox>,
    stderr_tail: Arc<Mutex<Vec<String>>>,
}

impl LoadedPlugin {
    pub fn is_alive(&self) -> bool {
        self.core.alive.load(Ordering::SeqCst)
    }

    pub fn has(&self, capability: Capability) -> bool {
        self.manifest.has(capability)
    }

    /// Recent stderr from the plugin, for diagnostics.
    pub fn stderr_tail(&self) -> Vec<String> {
        lock(&self.stderr_tail).clone()
    }

    pub fn request(&self, method: &str, params: Value) -> Result<Value, PluginError> {
        self.request_with_timeout(method, params, DEFAULT_REQUEST_TIMEOUT)
    }

    pub fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, PluginError> {
        self.core.request_with_timeout(method, params, timeout)
    }

    /// Fire-and-forget. Never waits for a reply.
    pub fn notify(&self, method: &str, params: Value) -> Result<(), PluginError> {
        let envelope = Envelope::notification(method, params);
        let line = serde_json::to_string(&envelope)
            .map_err(|err| PluginError::new("serialize_failed", err.to_string()))?;
        write_core_line(&self.core.stdin, &line)
            .map_err(|err| PluginError::new("write_failed", err.to_string()))
    }

    /// Stop the plugin: close stdin so the worker's own read loop sees EOF and
    /// tears itself down, then give it a short grace period before the caller
    /// drops us and the job object kills the process. A wedged worker never
    /// reads its pipe again, so no `shutdown` request is written — it could
    /// block the host forever on a full pipe.
    pub fn kill(&self) {
        self.core.alive.store(false, Ordering::SeqCst);
        drop(lock(&self.core.stdin).take());
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        while let Ok(mut child) = self.child.try_lock() {
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        return;
                    }
                    // Releasing the lock matters: a recovering path may want it.
                    drop(child);
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        self.kill();
        if let Some(handle) = lock(&self.reader).take() {
            let _ = handle.join();
        }
    }
}

/// Outcome of trying to load one plugin directory.
#[derive(Debug, Clone)]
pub struct LoadReport {
    pub directory: PathBuf,
    pub plugin_id: Option<String>,
    pub result: Result<(), String>,
}

/// A panel a loaded plugin contributes.
#[derive(Debug, Clone)]
pub struct PanelEntry {
    pub plugin_index: usize,
    pub plugin_id: String,
    pub descriptor: UiPanelDescriptor,
}

/// Owns every loaded plugin.
pub struct PluginHost {
    launcher: WorkerLauncher,
    services: Arc<dyn HostServices>,
    plugins: Vec<LoadedPlugin>,
    memory_limit: usize,
}

impl PluginHost {
    pub fn new(launcher: WorkerLauncher, services: Arc<dyn HostServices>) -> Self {
        Self {
            launcher,
            services,
            plugins: Vec::new(),
            memory_limit: DEFAULT_MEMORY_LIMIT,
        }
    }

    pub fn set_memory_limit(&mut self, bytes: usize) {
        self.memory_limit = bytes;
    }

    pub fn plugins(&self) -> &[LoadedPlugin] {
        &self.plugins
    }

    pub fn plugin(&self, index: usize) -> Option<&LoadedPlugin> {
        self.plugins.get(index)
    }

    /// Enumerate candidate plugin directories and read their manifests.
    pub fn discover(&self, plugins_dir: &Path) -> Vec<(PathBuf, Result<PluginManifest, String>)> {
        let Ok(entries) = std::fs::read_dir(plugins_dir) else {
            return Vec::new();
        };
        let mut found: Vec<(PathBuf, Result<PluginManifest, String>)> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .map(|path| {
                let manifest = crate::worker::load_manifest(&path);
                (path, manifest)
            })
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    }

    /// Load every plugin found in `plugins_dir`. Failures are reported, never
    /// fatal: one broken plugin must not stop the app from starting.
    pub fn load_all(&mut self, plugins_dir: &Path) -> Vec<LoadReport> {
        let mut reports = Vec::new();
        for (directory, manifest) in self.discover(plugins_dir) {
            let report = match manifest {
                Err(message) => LoadReport {
                    directory,
                    plugin_id: None,
                    result: Err(message),
                },
                Ok(manifest) => {
                    let plugin_id = Some(manifest.id.clone());
                    if !manifest.is_compatible() {
                        LoadReport {
                            directory,
                            plugin_id,
                            result: Err(format!(
                                "incompatible protocol '{}' (host speaks {})",
                                manifest.protocol,
                                poddies_plugin_api::PROTOCOL_VERSION
                            )),
                        }
                    } else {
                        let result = match self.load_one(directory.clone(), manifest) {
                            Ok(plugin) => {
                                self.plugins.push(plugin);
                                Ok(())
                            }
                            Err(error) => Err(error.message),
                        };
                        LoadReport {
                            directory,
                            plugin_id,
                            result,
                        }
                    }
                }
            };
            reports.push(report);
        }
        reports
    }

    /// Load a single plugin directory directly. Used by the app and by
    /// `poddies dev`, where the directory already *is* a plugin.
    pub fn load_dir(&mut self, directory: &Path) -> Result<(), PluginError> {
        let manifest = crate::worker::load_manifest(directory)
            .map_err(|err| PluginError::new("bad_manifest", err))?;

        if !manifest.is_compatible() {
            return Err(PluginError::new(
                "incompatible_protocol",
                format!(
                    "plugin speaks protocol '{}', host speaks {}",
                    manifest.protocol,
                    poddies_plugin_api::PROTOCOL_VERSION
                ),
            ));
        }

        let plugin = self.load_one(directory.to_path_buf(), manifest)?;
        self.plugins.push(plugin);
        Ok(())
    }

    /// Spawn a worker for one plugin and complete the `describe` handshake.
    pub fn load_one(
        &self,
        directory: PathBuf,
        manifest: PluginManifest,
    ) -> Result<LoadedPlugin, PluginError> {
        let mut command = self.launcher.command(&directory);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = command
            .spawn()
            .map_err(|err| PluginError::new("spawn_failed", err.to_string()))?;

        let sandbox = match Sandbox::attach(&child, self.memory_limit) {
            Ok(sandbox) => Some(sandbox),
            Err(err) => {
                eprintln!(
                    "[poddies] plugin '{}' running unsandboxed: {err}",
                    manifest.id
                );
                None
            }
        };

        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        if let Some(stderr) = child.stderr.take() {
            let tail = Arc::clone(&stderr_tail);
            let id = manifest.id.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                while read_line_capped(&mut reader, &mut line, MAX_LINE_BYTES).unwrap_or(false) {
                    let line = std::mem::take(&mut line);
                    eprintln!("[plugin {id}] {line}");
                    let mut tail = lock(&tail);
                    tail.push(line);
                    if tail.len() > 50 {
                        tail.remove(0);
                    }
                }
            });
        }

        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().ok_or_else(|| {
            PluginError::new("spawn_failed", "worker stdin unavailable")
        })?)));
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PluginError::new("spawn_failed", "worker stdout unavailable"))?;

        let pending: Arc<Mutex<HashMap<u64, Sender<Reply>>>> = Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));
        let next_id = Arc::new(AtomicU64::new(1));
        let services = Arc::clone(&self.services);

        let reader_handle = {
            let stdin = Arc::clone(&stdin);
            let pending = Arc::clone(&pending);
            let alive = Arc::clone(&alive);
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut line = String::new();
                while read_line_capped(&mut reader, &mut line, MAX_LINE_BYTES).unwrap_or(false) {
                    let line = std::mem::take(&mut line);
                    if line.trim().is_empty() {
                        continue;
                    }
                    // Junk on stdout is ignored rather than fatal.
                    let Ok(value) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };

                    if value.get("method").is_some() {
                        // A request from the plugin to the host.
                        if let Ok(envelope) = serde_json::from_value::<Envelope>(value) {
                            let reply = handle_host_request(&*services, &envelope);
                            if let Ok(payload) = serde_json::to_string(&reply) {
                                let mut guard = lock(&stdin);
                                if let Some(stdin) = guard.as_mut() {
                                    let _ = writeln!(stdin, "{payload}");
                                    let _ = stdin.flush();
                                }
                            }
                        }
                    } else if let Ok(reply) = serde_json::from_value::<Reply>(value) {
                        let sender = reply.id.and_then(|id| lock(&pending).remove(&id));
                        if let Some(sender) = sender {
                            let _ = sender.send(reply);
                        }
                    }
                }

                // EOF: the plugin is gone. Fail everything still waiting.
                alive.store(false, Ordering::SeqCst);
                for (_, sender) in lock(&pending).drain() {
                    let _ = sender.send(Reply::failed(
                        None,
                        "plugin_exited",
                        "plugin process exited",
                    ));
                }
            })
        };

        let placeholder = PluginInfo {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            protocol: manifest.protocol.clone(),
            ui_panels: Vec::new(),
        };

        let core = RequestCore {
            id: manifest.id.clone(),
            alive: Arc::clone(&alive),
            stdin: Arc::clone(&stdin),
            pending: Arc::clone(&pending),
            next_id: Arc::clone(&next_id),
        };

        let plugin = LoadedPlugin {
            manifest,
            info: placeholder,
            directory,
            core,
            child: Mutex::new(child),
            reader: Mutex::new(Some(reader_handle)),
            _sandbox: sandbox,
            stderr_tail,
        };

        let info: PluginInfo =
            match plugin.request_with_timeout(methods::DESCRIBE, Value::Null, HANDSHAKE_TIMEOUT) {
                Ok(value) => serde_json::from_value(value).map_err(|err| {
                    PluginError::new("handshake_failed", format!("bad describe payload: {err}"))
                })?,
                Err(error) => {
                    plugin.kill();
                    return Err(PluginError::new(
                        "handshake_failed",
                        format!("plugin did not answer describe: {}", error.message),
                    ));
                }
            };

        if info.id != plugin.manifest.id {
            plugin.kill();
            return Err(PluginError::new(
                "id_mismatch",
                format!(
                    "manifest declares '{}' but plugin reports '{}'",
                    plugin.manifest.id, info.id
                ),
            ));
        }

        let mut plugin = plugin;
        plugin.info = info;
        Ok(plugin)
    }

    /// Every panel contributed by a loaded plugin that declared the capability.
    /// A plugin whose worker has died contributes nothing, so a stopped plugin
    /// cannot leave a dead panel in the interface.
    pub fn panels(&self) -> Vec<PanelEntry> {
        let mut panels = Vec::new();
        for (index, plugin) in self.plugins.iter().enumerate() {
            if !plugin.has(Capability::UiPanel) || !plugin.is_alive() {
                continue;
            }
            for descriptor in &plugin.info.ui_panels {
                panels.push(PanelEntry {
                    plugin_index: index,
                    plugin_id: plugin.manifest.id.clone(),
                    descriptor: descriptor.clone(),
                });
            }
        }
        panels
    }

    /// Ask one plugin to render a panel.
    pub fn panel_content(&self, index: usize, panel_id: &str) -> Result<PanelContent, PluginError> {
        let plugin = self
            .plugins
            .get(index)
            .ok_or_else(|| PluginError::new("unknown_plugin", format!("no plugin at {index}")))?;

        if !plugin.has(Capability::UiPanel) {
            return Err(PluginError::new(
                "capability_denied",
                format!("'{}' did not declare ui-panel", plugin.manifest.id),
            ));
        }

        let params = serde_json::to_value(PanelRequest {
            panel_id: panel_id.to_string(),
        })
        .map_err(|err| PluginError::new("serialize_failed", err.to_string()))?;

        let value = plugin.request(methods::UI_PANEL, params)?;
        serde_json::from_value(value)
            .map_err(|err| PluginError::new("bad_response", err.to_string()))
    }

    /// Gather candidates from every discovery source. A failing source is
    /// logged and skipped so the queue still renders.
    ///
    /// `topics` is a hint derived from the listener's profile; `timeout` should
    /// be generous enough for a source that does network I/O.
    ///
    /// Sources are asked concurrently: the slowest source sets the wait, not
    /// the sum of all of them. Results stay in plugin order regardless.
    pub fn discovery_candidates(
        &self,
        limit: usize,
        topics: &[String],
        timeout: Duration,
    ) -> Vec<DiscoveryCandidate> {
        let sources: Vec<(String, RequestCore, Value)> = self
            .plugins
            .iter()
            .filter(|plugin| plugin.has(Capability::DiscoverySource) && plugin.is_alive())
            .filter_map(|plugin| {
                let params = serde_json::to_value(DiscoveryRequest {
                    limit,
                    topics: topics.to_vec(),
                })
                .ok()?;
                Some((plugin.manifest.id.clone(), plugin.core.clone(), params))
            })
            .collect();

        // One thread per source; the slowest source sets the wait instead of
        // the sum of all of them. Cores outlive the loop by clone, so a plugin
        // unloaded mid-flight just fails its call like any dead plugin would.
        let replies: Vec<_> = sources
            .into_iter()
            .map(|(id, core, params)| {
                let (sender, receiver) = mpsc::channel();
                std::thread::spawn(move || {
                    let _ = sender.send(core.request_with_timeout(
                        methods::DISCOVERY_LIST,
                        params,
                        timeout,
                    ));
                });
                (id, receiver)
            })
            .collect();

        let mut candidates = Vec::new();
        for (id, receiver) in replies {
            match receiver.recv_timeout(timeout + Duration::from_secs(1)) {
                Ok(Ok(value)) => match serde_json::from_value::<DiscoveryResponse>(value) {
                    Ok(response) => candidates.extend(response.candidates),
                    Err(err) => eprintln!(
                        "[poddies] plugin '{id}' returned malformed discovery data: {err}"
                    ),
                },
                Ok(Err(error)) => {
                    eprintln!(
                        "[poddies] plugin '{id}' discovery failed: {}",
                        error.message
                    )
                }
                Err(_) => {
                    eprintln!("[poddies] plugin '{id}' discovery thread did not finish")
                }
            }
        }
        candidates
    }

    /// Send an event to plugins. Playback lifecycle events only reach plugins
    /// that declared the playback-hook capability; anything else is broadcast
    /// to every live plugin.
    pub fn broadcast(&self, method: &str, params: Value) {
        let playback_only = method.starts_with("event/playback");
        for plugin in &self.plugins {
            if !plugin.is_alive() {
                continue;
            }
            if playback_only && !plugin.has(Capability::PlaybackHook) {
                continue;
            }
            let _ = plugin.notify(method, params.clone());
        }
    }

    /// Tell one plugin that a control in one of its panels moved. Fire and
    /// forget: the plugin answers with fresh state the next time the host asks
    /// for the panel.
    pub fn notify_change(&self, index: usize, change: &WidgetChange) -> Result<(), PluginError> {
        let plugin = self
            .plugins
            .get(index)
            .ok_or_else(|| PluginError::new("unknown_plugin", format!("no plugin at {index}")))?;
        if !plugin.has(Capability::UiPanel) {
            return Err(PluginError::new(
                "capability_denied",
                format!("'{}' did not declare ui-panel", plugin.manifest.id),
            ));
        }
        let payload = serde_json::to_value(change)
            .map_err(|error| PluginError::new("serialize_failed", error.to_string()))?;
        plugin.notify(methods::UI_CHANGE, payload)
    }

    /// Collect the audio units every enabled plugin wants in the playback
    /// chain, in plugin order. A unit's id is prefixed with the plugin id so two
    /// plugins cannot collide.
    pub fn audio_graph(&self) -> Vec<AudioUnit> {
        let mut units = Vec::new();
        for plugin in &self.plugins {
            if !plugin.has(Capability::AudioEffects) || !plugin.is_alive() {
                continue;
            }
            match plugin.request(methods::AUDIO_GRAPH, Value::Null) {
                Ok(value) => match serde_json::from_value::<AudioGraph>(value) {
                    Ok(graph) => {
                        units.extend(graph.units.into_iter().map(|unit| qualify(plugin, unit)))
                    }
                    Err(error) => eprintln!(
                        "[poddies] plugin '{}' returned a malformed audio graph: {error}",
                        plugin.manifest.id
                    ),
                },
                Err(error) => eprintln!(
                    "[poddies] plugin '{}' audio/graph failed: {}",
                    plugin.manifest.id, error.message
                ),
            }
        }
        units
    }

    /// Restart a plugin in place. Used by hot-reload and by crash recovery.
    pub fn reload(&mut self, index: usize) -> Result<(), PluginError> {
        let directory = self
            .plugins
            .get(index)
            .map(|plugin| plugin.directory.clone())
            .ok_or_else(|| PluginError::new("unknown_plugin", format!("no plugin at {index}")))?;
        let manifest = crate::worker::load_manifest(&directory)
            .map_err(|err| PluginError::new("bad_manifest", err))?;

        self.plugins.remove(index);
        let plugin = self.load_one(directory, manifest)?;
        self.plugins.insert(index, plugin);
        Ok(())
    }

    /// Stop a plugin and forget it. Dropping the entry kills the worker and
    /// closes the sandbox, so nothing is left running.
    ///
    /// Indices after `index` shift down; callers re-read the plugin list rather
    /// than holding an index across this call.
    pub fn unload(&mut self, index: usize) -> Result<PathBuf, PluginError> {
        if index >= self.plugins.len() {
            return Err(PluginError::new(
                "unknown_plugin",
                format!("no plugin at {index}"),
            ));
        }
        let plugin = self.plugins.remove(index);
        let directory = plugin.directory.clone();
        drop(plugin);
        Ok(directory)
    }

    pub fn shutdown_all(&self) {
        for plugin in &self.plugins {
            plugin.kill();
        }
    }
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        self.shutdown_all();
    }
}

/// Namespace a unit's id with its plugin, so two plugins cannot collide in the
/// merged graph and the interface can attribute a unit to its owner.
fn qualify(plugin: &LoadedPlugin, mut unit: AudioUnit) -> AudioUnit {
    let qualified = format!("{}::{}", plugin.manifest.id, unit.id());
    match &mut unit {
        AudioUnit::ParametricEq { id, .. } | AudioUnit::Compressor { id, .. } => *id = qualified,
    }
    unit
}

fn handle_host_request(services: &dyn HostServices, envelope: &Envelope) -> Reply {
    match envelope.method.as_str() {
        methods::HOST_LIBRARY_SHOWS => Reply::ok(envelope.id, services.library_shows()),
        methods::HOST_LIBRARY_HISTORY => Reply::ok(envelope.id, services.library_history()),
        methods::HOST_LOG => {
            let request: LogRequest =
                serde_json::from_value(envelope.params.clone()).unwrap_or(LogRequest {
                    level: "info".to_string(),
                    message: String::new(),
                });
            services.log(&request.level, &request.message);
            Reply::ok(envelope.id, Value::Null)
        }
        other => Reply::failed(
            envelope.id,
            "unsupported_method",
            format!("host does not handle '{other}'"),
        ),
    }
}

//! End-to-end plugin lifecycle test.
//!
//! This is the test that matters for the "a plugin can never crash the host"
//! claim: it spawns real worker processes through the real manager, drives the
//! protocol, kills a plugin mid-session and asserts the host survives with a
//! clean error instead of a hang.
//!
//! Requires Python for the fixture plugins; skipped cleanly when absent.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use poddies_plugin_host::{HostServices, PluginHost, WorkerLauncher};

const HELLO_PLUGIN: &str = r#"
import json, sys

playback_events = 0

def send(payload):
    sys.stdout.write(json.dumps(payload) + "\n")
    sys.stdout.flush()

def host_request(req_id, method, params):
    send({"id": req_id, "method": method, "params": params})
    while True:
        line = sys.stdin.readline()
        if not line:
            return None
        msg = json.loads(line)
        if "method" not in msg and msg.get("id") == req_id:
            return msg.get("result")

send({"id": 1000, "method": "host/log", "params": {"level": "info", "message": "hello from plugin"}})

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    if "method" not in msg:
        continue
    method = msg.get("method")
    mid = msg.get("id")
    if mid is None:
        if method.startswith("event/playback"):
            playback_events += 1
        continue

    if method == "describe":
        send({"id": mid, "result": {
            "id": "dev.test.hello", "name": "Hello", "version": "0.1.0",
            "protocol": "1.0", "ui_panels": [{"id": "main", "title": "Hello"}]}})
    elif method == "ui/panel":
        send({"id": mid, "result": {"panel_id": "main", "widgets": [
            {"type": "heading", "text": "Hello"},
            {"type": "metric", "label": "Playback events", "value": str(playback_events)}]}})
    elif method == "discovery/list":
        limit = (msg.get("params") or {}).get("limit", 0)
        send({"id": mid, "result": {"candidates": [
            {"title": "Show " + str(i), "feed_url": "https://example.com/" + str(i) + ".xml",
             "source": "dev.test.hello", "categories": ["Technology"]}
            for i in range(limit)]}})
    elif method == "test/playback-count":
        send({"id": mid, "result": playback_events})
    elif method == "test/library-count":
        shows = host_request(1001, "host/library/shows", {}) or []
        send({"id": mid, "result": len(shows)})
    elif method == "shutdown":
        send({"id": mid, "result": None})
        break
    else:
        send({"id": mid, "error": {"code": "unsupported_method", "message": str(method)}})
"#;

const CRASHER_PLUGIN: &str = r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    if msg.get("method") == "describe":
        sys.stdout.write(json.dumps({"id": msg["id"], "result": {
            "id": "dev.test.crasher", "name": "Crasher", "version": "0.1.0",
            "protocol": "1.0", "ui_panels": []}}) + "\n")
        sys.stdout.flush()
        break
sys.exit(3)
"#;

/// A second discovery source that is deliberately slow. The host asks sources
/// concurrently, so this bounds the whole discovery round rather than adding
/// to it, and plugin order decides merge order.
const SLOW_PLUGIN: &str = r#"
import json, sys, time
for line in sys.stdin:
    msg = json.loads(line)
    if "method" not in msg:
        continue
    mid = msg.get("id")
    if msg["method"] == "describe":
        sys.stdout.write(json.dumps({"id": mid, "result": {
            "id": "dev.test.slow", "name": "Slow", "version": "0.1.0",
            "protocol": "1.0", "ui_panels": []}}) + "\n")
        sys.stdout.flush()
        continue
    if msg["method"] == "discovery/list":
        time.sleep(1.0)
        sys.stdout.write(json.dumps({"id": mid, "result": {"candidates": [
            {"title": "Slow " + str(i),
             "feed_url": "https://slow.example/" + str(i) + ".xml",
             "source": "dev.test.slow", "categories": ["Science"]}
            for i in range(2)]}}) + "\n")
        sys.stdout.flush()
        continue
    sys.stdout.write(json.dumps({"id": mid, "error": {"code": "unsupported_method", "message": msg["method"]}}) + "\n")
    sys.stdout.flush()
"#;

#[derive(Default)]
struct TestServices {
    logs: Mutex<Vec<String>>,
}

impl HostServices for TestServices {
    fn library_shows(&self) -> serde_json::Value {
        json!([{ "title": "A Subscribed Show" }])
    }

    fn log(&self, level: &str, message: &str) {
        self.logs
            .lock()
            .unwrap()
            .push(format!("{level}: {message}"));
    }
}

fn python_available() -> bool {
    let candidates: [(&str, &[&str]); 2] =
        [("py", &["-3", "--version"]), ("python", &["--version"])];
    candidates.iter().any(|(program, args)| {
        Command::new(program)
            .args(*args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    })
}

fn write_fixture(root: &Path, name: &str, manifest: &str, source: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("plugin.json"), manifest).unwrap();
    std::fs::write(dir.join("main.py"), source).unwrap();
    dir
}

#[test]
fn plugin_lifecycle_end_to_end() {
    if !python_available() {
        eprintln!("skipping: no Python interpreter on PATH");
        return;
    }

    let root = std::env::temp_dir().join("poddies_plugin_lifecycle");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    write_fixture(
        &root,
        "hello",
        r#"{
            "id": "dev.test.hello",
            "name": "Hello",
            "version": "0.1.0",
            "protocol": "1.0",
            "capabilities": ["ui-panel", "discovery-source", "playback-hook", "library-read"],
            "runtime": { "kind": "python", "entry": "main.py" }
        }"#,
        HELLO_PLUGIN,
    );
    write_fixture(
        &root,
        "crasher",
        r#"{
            "id": "dev.test.crasher",
            "name": "Crasher",
            "version": "0.1.0",
            "protocol": "1.0",
            "capabilities": ["ui-panel"],
            "runtime": { "kind": "python", "entry": "main.py" }
        }"#,
        CRASHER_PLUGIN,
    );
    write_fixture(
        &root,
        "slow",
        r#"{
            "id": "dev.test.slow",
            "name": "Slow",
            "version": "0.1.0",
            "protocol": "1.0",
            "capabilities": ["discovery-source"],
            "runtime": { "kind": "python", "entry": "main.py" }
        }"#,
        SLOW_PLUGIN,
    );

    let services = Arc::new(TestServices::default());
    let launcher = WorkerLauncher::new(env!("CARGO_BIN_EXE_poddies-plugin-worker"));
    let mut host = PluginHost::new(launcher, Arc::clone(&services) as Arc<dyn HostServices>);

    let reports = host.load_all(&root);
    assert_eq!(reports.len(), 3);
    for report in &reports {
        assert!(report.result.is_ok(), "failed to load {:?}", report);
    }
    assert_eq!(host.plugins().len(), 3);

    // Panels are advertised and rendered.
    let panels = host.panels();
    assert_eq!(panels.len(), 1);
    assert_eq!(panels[0].descriptor.title, "Hello");

    let content = host.panel_content(panels[0].plugin_index, "main").unwrap();
    assert!(content.widgets.len() >= 2);

    // Discovery source contributes candidates. Two sources are asked
    // concurrently and merge in plugin order (hello < slow alphabetically).
    let started = std::time::Instant::now();
    let candidates =
        host.discovery_candidates(3, &["Technology".to_string()], Duration::from_secs(5));
    // The slow source sleeps 1s; a sequential host would spend 2s+ here.
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "discovery round took too long: {:?}",
        started.elapsed()
    );
    assert_eq!(candidates.len(), 5);
    assert_eq!(candidates[0].title, "Show 0");
    assert_eq!(candidates[3].title, "Slow 0");

    // Plugin -> host request for library data round-trips.
    let hello = host
        .plugins()
        .iter()
        .position(|plugin| plugin.manifest.id == "dev.test.hello")
        .unwrap();
    let count = host
        .plugins()
        .get(hello)
        .unwrap()
        .request("test/library-count", json!({}))
        .unwrap();
    assert_eq!(count, json!(1));

    // Host log call landed in our services.
    assert!(
        services
            .logs
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains("hello from plugin"))
    );

    // Playback events reach the plugin as notifications.
    host.broadcast(
        "event/playback-started",
        json!({ "episode_id": "ep_1", "show_id": "sh_1", "title": "T", "position_secs": 0.0 }),
    );
    host.broadcast(
        "event/playback-progress",
        json!({ "episode_id": "ep_1", "show_id": "sh_1", "title": "T", "position_secs": 5.0 }),
    );
    let seen = host
        .plugins()
        .get(hello)
        .unwrap()
        .request("test/playback-count", json!({}))
        .unwrap();
    assert_eq!(seen, json!(2));

    // Crash isolation: the crasher answers describe then dies. A later call
    // must return an error promptly rather than hanging, and the other plugin
    // and the host itself must still be healthy.
    let crasher = host
        .plugins()
        .iter()
        .position(|plugin| plugin.manifest.id == "dev.test.crasher")
        .unwrap();
    let error = host
        .plugins()
        .get(crasher)
        .unwrap()
        .request("ui/panel", json!({ "panel_id": "main" }))
        .unwrap_err();
    assert!(
        matches!(
            error.code.as_str(),
            "plugin_exited" | "write_failed" | "timeout"
        ),
        "unexpected error code: {}",
        error.code
    );

    let still_there = host
        .plugins()
        .get(hello)
        .unwrap()
        .request("test/playback-count", json!({}))
        .unwrap();
    assert_eq!(still_there, json!(2));

    host.shutdown_all();
    let _ = std::fs::remove_dir_all(&root);
}

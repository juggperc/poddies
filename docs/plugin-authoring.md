# Writing a Poddies plugin

A plugin adds a panel to the interface, supplies candidate shows to Discovery,
observes playback, or any combination of those. It runs in its own process and
talks to the host over a small JSON protocol, so you can write one in Rust or
Python and a misbehaving plugin can never take the app down.

- [Quickstart](#quickstart)
- [The manifest](#the-manifest)
- [A Rust plugin](#a-rust-plugin)
- [A Python plugin](#a-python-plugin)
- [Reading the library](#reading-the-library)
- [Panels and widgets](#panels-and-widgets)
- [Feeding Discovery](#feeding-discovery)
- [Playback hooks](#playback-hooks)
- [Hot reload](#hot-reload)
- [Debugging](#debugging)
- [Shipping](#shipping)

---

## Quickstart

```powershell
poddies-cli new plugin my-stats --lang rust
poddies-cli dev .\plugins\my-stats
```

`dev` builds the plugin, loads it through the real sandboxed host and reloads on
every save. Edit `src/lib.rs`, watch the panel output change.

To install it, copy the directory into the app's plugin folder:

```
%APPDATA%\Poddies\plugins\my-stats\
```

or next to the executable in `plugins\my-stats\`.

---

## The manifest

Every plugin directory contains a `plugin.json`:

```json
{
  "id": "dev.example.my-stats",
  "name": "My Stats",
  "version": "0.1.0",
  "protocol": "1.0",
  "description": "What this plugin does.",
  "author": "You",
  "capabilities": ["ui-panel", "library-read"],
  "runtime": { "kind": "native", "library": "my_stats.dll" }
}
```

| Field | Meaning |
|---|---|
| `id` | Unique, stable identifier. Must match what the plugin reports at runtime. |
| `protocol` | The protocol major.minor you were built against. The host refuses a different **major** and warns on a different minor. |
| `capabilities` | What the plugin may do. Enforced by the host. |
| `runtime` | `{"kind":"native","library":"<file>.dll"}` or `{"kind":"python","entry":"plugin.py"}`. Paths are relative to the plugin directory. |

### Capabilities

| Capability | Grants |
|---|---|
| `ui-panel` | Contribute panels to the interface. |
| `discovery-source` | Supply candidates to the Discovery queue. |
| `playback-hook` | Receive playback lifecycle events. |
| `library-read` | Call `host/library/shows` and `host/library/history`. |
| `library-write` | Reserved; not granted to the reference plugins. |

Calling a method you did not declare returns `capability_denied` before your
code is ever reached.

---

## A Rust plugin

A plugin is a `cdylib` exporting one entry point. You implement `Plugin` and call
`export_plugin!`; the SDK owns the transport, ID assignment, errors and
shutdown.

```toml
[package]
name = "my-stats"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
poddies-plugin-sdk = { path = "…/crates/poddies-plugin-sdk" }
serde_json = "1"
```

```rust
use poddies_plugin_sdk::{
    export_plugin, json, Plugin, PluginError, PluginInfo, UiPanelDescriptor, Widget,
    PROTOCOL_VERSION,
};
use serde_json::Value;

#[derive(Default)]
pub struct MyStats;

impl Plugin for MyStats {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: "dev.example.my-stats".to_string(),
            name: "My Stats".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: PROTOCOL_VERSION.to_string(),
            ui_panels: vec![UiPanelDescriptor {
                id: "main".to_string(),
                title: "My Stats".to_string(),
            }],
        }
    }

    fn on_request(&mut self, method: &str, params: Value) -> Result<Value, PluginError> {
        match method {
            "ui/panel" => Ok(json!({
                "panel_id": "main",
                "widgets": [{ "type": "heading", "text": "Hello" }]
            })),
            other => Err(PluginError::unsupported(other)),
        }
    }
}

export_plugin!(MyStats);
```

The plugin type must be `Default`; the SDK keeps one instance behind a mutex.

Errors are values, not panics. Return `PluginError::unsupported(method)` for a
method you do not implement, and the host sees a clean `unsupported_method`
error instead of a broken pipe.

---

## A Python plugin

```python
from poddies import Plugin, UnsupportedMethod, run


class MyStats(Plugin):
    id = "dev.example.my-stats"
    name = "My Stats"
    panels = [{"id": "main", "title": "My Stats"}]

    def on_request(self, method, params):
        if method == "ui/panel":
            return {
                "panel_id": "main",
                "widgets": [{"type": "heading", "text": "Hello"}],
            }
        raise UnsupportedMethod(method)


run(MyStats())
```

`python/poddies` is the SDK. The worker puts it on `PYTHONPATH` automatically
during `poddies dev`; when shipping, either bundle it beside the plugin or install
it. A plugin that raises still returns a structured error — the loop catches it
so a bad handler can never kill the pipe.

---

## Reading the library

Declare `library-read`, then call the host. Rust:

```rust
use poddies_plugin_sdk::api::library::{HistoryEntry, ShowSummary};
use poddies_plugin_sdk::{host_call, json, PluginError};

fn history(&self) -> Result<Vec<HistoryEntry>, PluginError> {
    let value = host_call("host/library/history", json!({}))?;
    serde_json::from_value(value)
        .map_err(|err| PluginError::new("bad_host_data", err.to_string()))
}
```

Python:

```python
from poddies import call_host

history = call_host("host/library/history") or []
```

The shapes are defined once, in `poddies-plugin-api::library`, and shared by the
host that produces them and the plugins that consume them:

```jsonc
// host/library/shows  ->  [ShowSummary]
{ "id": "sh_…", "title": "…", "author": "…", "categories": ["Technology"],
  "subscribed": true, "episode_count": 812 }

// host/library/history -> [HistoryEntry]
{ "episode_id": "ep_…", "show_id": "sh_…", "title": "…", "show_title": "…",
  "position_secs": 1234.0, "duration_secs": 2400, "completed": true,
  "last_played": "2026-09-01T10:00:00Z" }
```

Logging goes to the host's plugin log:

```rust
poddies_plugin_sdk::log("info", "did a thing");
```

```python
from poddies import log
log("info", "did a thing")
```

---

## Panels and widgets

Plugins never ship HTML, CSS or JavaScript. They return a list of widgets and
the host renders them with the app's own typography, so every panel matches the
rest of the interface and no plugin can inject markup.

| Widget | JSON |
|---|---|
| Heading | `{"type":"heading","text":"…"}` |
| Metric | `{"type":"metric","label":"Episodes","value":"128"}` |
| Text | `{"type":"text","text":"…"}` |
| Divider | `{"type":"divider"}` |
| Bar | `{"type":"bar","label":"Completion","value":0.8,"max":1.0}` |
| List | `{"type":"list","items":[{"primary":"…","secondary":"…"}]}` |

`ui/panel` receives `{"panel_id": "…"}` and returns:

```json
{ "panel_id": "main", "widgets": [ … ] }
```

A widget type the host does not recognise is skipped rather than rendered as
garbage, so a plugin built against a newer minor protocol still loads.

---

## Feeding Discovery

Declare `discovery-source` and answer `discovery/list`:

```json
// request
{ "limit": 24, "topics": ["Technology", "News", "Science"] }
```

```json
// response
{ "candidates": [
  { "title": "Example Show",
    "feed_url": "https://example.com/feed.xml",
    "source": "dev.example.my-source",
    "categories": ["Technology"],
    "latest_published": "2026-08-01T00:00:00Z",
    "typical_duration_secs": 2400,
    "explicit": false,
    "image_url": null,
    "author": null,
    "description": null,
    "popularity": null } ] }
```

`topics` is the host's summary of what the listener actually listens to, so a
source can fetch relevant candidates without being trusted to rank them.

**Ranking is the host's job, not yours.** Do not invent a score: the queue is a
transparent, user-weighted sum of topic affinity, follow-through, recency,
novelty, length fit, an explicit filter and variety — all of which the user can
see and adjust. A plugin that tries to smuggle in an opaque ordering achieves
nothing. `popularity` exists only as a deterministic tie-breaker.

The reference `poddies-plugin-apple-podcasts` plugin is a complete example: it queries
the public iTunes Search API per topic, caches results, and falls back to a
curated list when the network is unavailable.

---

## Playback hooks

Declare `playback-hook` and handle notifications. They need no reply.

```
event/playback-started    { episode_id, show_id, title, position_secs, duration_secs }
event/playback-progress   { … , position_secs }
event/playback-completed  { … }
event/library-changed     fired after a refresh or subscription change
```

In Rust these arrive on `Plugin::on_notification`; in Python, on
`Plugin.on_event`.

---

## Hot reload

```powershell
poddies-cli dev .\plugins\my-plugin
```

For a Rust plugin this runs `cargo build --release`, copies the fresh `cdylib`
next to `plugin.json`, loads it through the real sandbox, prints what it
contributes, then watches the directory and repeats on change. Add `--once` for
a single pass, which is useful in CI.

---

## Debugging

- Anything a plugin writes to **stderr** is prefixed with `[plugin <id>]` and
  surfaces in the host's console and in `poddies dev` output.
- Nothing may be written to **stdout** except protocol JSON. Stray output is
  ignored rather than fatal, but it is still a bug in your plugin.
- Use `log(...)` for diagnostics you want attributed to the plugin.
- `poddies-cli check .\plugins\my-plugin` validates the manifest, the protocol
  version and that the declared runtime file exists.
- A plugin that hangs is killed by the host's per-call timeout
  (5 s normally, 12 s for discovery) and reported as a `timeout` error; the app
  keeps working.

---

## Shipping

A plugin is just a directory:

```
my-plugin/
  plugin.json
  my_plugin.dll     (or plugin.py)
```

Copy it into `%APPDATA%\Poddies\plugins\` or `plugins\` next to `poddies.exe`.
Poddies loads plugins on start; a plugin that fails to load is reported in the
interface and never prevents the app from running.

For Python plugins, ship the `poddies` SDK package alongside or install it, and
note that the target machine needs Python 3 on `PATH`.

//! Scaffolding templates and the generated API reference.
//!
//! Substitution is done with explicit `@@TOKEN@@` markers rather than
//! `format!`, so templates can contain braces freely — which matters because
//! the Rust and Python templates are full of them.

pub const RUST_CARGO: &str = r#"[package]
name = "@@CRATE@@"
version = "0.1.0"
edition = "2021"
description = "@@DISPLAY@@ — a Poddies plugin."
license = "MIT"

[lib]
# A plugin is a dynamic library the worker loads.
crate-type = ["cdylib"]

[dependencies]
poddies-plugin-sdk = { path = "@@SDK@@" }
serde_json = "1"

[profile.release]
opt-level = "s"
strip = true
"#;

pub const RUST_LIB: &str = r#"//! @@DISPLAY@@ — a Poddies plugin.
//!
//! Build with `cargo build --release`, then run `poddies-cli dev .` for a
//! reload-on-save development loop.

use poddies_plugin_sdk::{export_plugin, json, Plugin, PluginError, PluginInfo, PROTOCOL_VERSION};
use serde_json::Value;

#[derive(Default)]
pub struct @@STRUCT@@;

impl Plugin for @@STRUCT@@ {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: "@@ID@@".to_string(),
            name: "@@DISPLAY@@".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: PROTOCOL_VERSION.to_string(),
            ui_panels: vec![poddies_plugin_sdk::UiPanelDescriptor {
                id: "main".to_string(),
                title: "@@DISPLAY@@".to_string(),
            }],
        }
    }

    fn on_request(&mut self, method: &str, _params: Value) -> Result<Value, PluginError> {
        match method {
            // The host asks for a panel's contents; return widgets, not HTML.
            "ui/panel" => Ok(json!({
                "panel_id": "main",
                "widgets": [
                    { "type": "heading", "text": "@@DISPLAY@@" },
                    { "type": "text", "text": "Edit src/lib.rs and reload." },
                ]
            })),
            other => Err(PluginError::unsupported(other)),
        }
    }
}

export_plugin!(@@STRUCT@@);
"#;

pub const PY_PLUGIN: &str = r#""""@@DISPLAY@@ — a Poddies plugin."""

from poddies import Plugin, UnsupportedMethod, run


class @@CLASS@@(Plugin):
    id = "@@ID@@"
    name = "@@DISPLAY@@"
    version = "0.1.0"
    panels = [{"id": "main", "title": "@@DISPLAY@@"}]

    def on_request(self, method, params):
        if method == "ui/panel":
            return {
                "panel_id": "main",
                "widgets": [
                    {"type": "heading", "text": "@@DISPLAY@@"},
                    {"type": "text", "text": "Edit plugin.py and reload."},
                ],
            }
        raise UnsupportedMethod(method)


if __name__ == "__main__":
    run(@@CLASS@@())
"#;

pub const RUST_MANIFEST: &str = r#"{
  "id": "@@ID@@",
  "name": "@@DISPLAY@@",
  "version": "0.1.0",
  "protocol": "1.0",
  "description": "A Poddies plugin.",
  "author": "",
  "capabilities": ["ui-panel"],
  "runtime": { "kind": "native", "library": "@@LIB@@" }
}
"#;

pub const PY_MANIFEST: &str = r#"{
  "id": "@@ID@@",
  "name": "@@DISPLAY@@",
  "version": "0.1.0",
  "protocol": "1.0",
  "description": "A Poddies plugin.",
  "author": "",
  "capabilities": ["ui-panel"],
  "runtime": { "kind": "python", "entry": "plugin.py" }
}
"#;

pub const README: &str = r#"# @@DISPLAY@@

A Poddies plugin scaffolded with `poddies-cli`.

## Develop

```sh
poddies-cli dev .
```

That builds (for Rust), loads the plugin through the real sandboxed host and
reloads automatically on every save.

## Validate

```sh
poddies-cli check .
```

## Install

Drop this directory into the Poddies `plugins` folder:

```
%APPDATA%\Poddies\plugins\@@CRATE@@\
```

## Next steps

`@@SOURCE@@` is where your logic goes. The SDK exposes:

- `ui/panel` — return widgets for a panel (see `poddies-cli docs` for the set)
- `discovery/list` — contribute shows to the Discovery queue
  (add the `discovery-source` capability first)
- playback events arrive on `on_event` / `on_notification` when the
  `playback-hook` capability is declared
- `host/library/shows` and `host/library/history` read the user's library
"#;

pub const DOCS: &str = r#"PODDIES PLUGIN API — reference
==============================

A plugin is a directory:

    plugins/<name>/
      plugin.json     declares identity, capabilities and runtime
      <library>       a .dll (kind: "native") or a .py entry file (kind: "python")

The host never loads a plugin into its own process. It spawns a worker
(`poddies.exe --plugin-worker <dir>`), so a plugin that panics, hangs or
hard-crashes takes down only the worker, which the host restarts.


MANIFEST (plugin.json)
----------------------

  id            unique string, e.g. "dev.poddies.stats"
  name          human-readable name
  version       semver string
  protocol      protocol major.minor you were built against ("1.0")
  capabilities  ["ui-panel" | "discovery-source" | "playback-hook"
                 | "library-read" | "library-write"]
  runtime       { "kind": "native", "library": "my_plugin.dll" }
                { "kind": "python", "entry": "plugin.py" }

A capability is enforced by the host: calling a method you did not declare
returns `capability_denied`.


TRANSPORT
---------

Newline-delimited JSON on stdin/stdout. Each request carries an "id" and gets
exactly one reply with the same "id". Notifications omit "id" and are never
answered. Malformed input is ignored, never fatal.

  -> {"id":1,"method":"describe","params":null}
  <- {"id":1,"result":{ ... }}


METHODS THE HOST CALLS ON YOU
-----------------------------

  describe          -> { id, name, version, protocol, ui_panels[] }
  ui/panel          params { panel_id } -> { panel_id, widgets[] }
  discovery/list    params { limit }    -> { candidates[] }
  shutdown          no reply required; flush state
  event/playback-started   notification { episode_id, show_id, title,
  event/playback-progress                     position_secs, duration_secs }
  event/playback-completed notification
  event/library-changed    notification


METHODS YOU MAY CALL ON THE HOST
--------------------------------

  host/library/shows     -> [{ ...show }]
  host/library/history   -> [{ ...episode, position_secs, completed }]
  host/log               params { level, message } — fire and forget


WIDGETS
-------

Panels are declarative; you never ship HTML. Each widget is one of:

  { "type": "heading", "text": "..." }
  { "type": "metric",  "label": "...", "value": "..." }
  { "type": "text",    "text": "..." }
  { "type": "divider" }
  { "type": "bar",     "label": "...", "value": 0.6, "max": 1.0 }
  { "type": "list",    "items": [ { "primary": "...", "secondary": "..." } ] }


DISCOVERY CANDIDATES
--------------------

The host ranks candidates; you only supply them. Return objects you build in
your own code — the ranking is a transparent, user-weighted sum of topic
affinity, completion, recency, novelty, duration fit and explicit filtering,
so injecting an opaque score achieves nothing.

  { "title": "...", "feed_url": "https://...", "source": "<your plugin id>",
    "categories": ["Technology"], "latest_published": "2026-01-01T00:00:00Z",
    "typical_duration_secs": 2400, "explicit": false }


RUST
----

  use poddies_plugin_sdk::{export_plugin, Plugin, PluginError, PluginInfo};
  impl Plugin for MyPlugin { fn info(...) ... fn on_request(...) ... }
  export_plugin!(MyPlugin);

See crates/poddies-plugin-sdk for the full surface.


PYTHON
------

  from poddies import Plugin, run
  class MyPlugin(Plugin): ...
  run(MyPlugin())

`poddies.call_host("host/library/shows")` performs a synchronous host call.

Full guide: docs/plugin-authoring.md
"#;

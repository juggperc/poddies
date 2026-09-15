# Plugin API reference

The contract between the Poddies host and a plugin. Two independent versions:

| Version | Constant | Meaning |
|---|---|---|
| Protocol | `PROTOCOL_VERSION = "1.0"` | The JSON conversation. Additive changes bump the minor; changing an existing message's meaning bumps the major. |
| ABI | `ABI_VERSION = 2` | The in-process C ABI for native DLLs. Bumps only when the loading contract changes. |

The host loads a plugin whose protocol **major** matches; a minor mismatch is
allowed and logged. A plugin built for `1.x` therefore keeps working as long as
the major does, and new methods can be added without breaking old plugins.

---

## Recommended route: the SDK

Hand-writing the protocol is possible — it is only JSON lines — but the SDKs
implement the loop, ID assignment, error mapping and shutdown for you.

- Rust: `poddies-plugin-sdk`
- Python: `python/poddies`

Everything below is what those SDKs do, and what you need if you are writing a
plugin in another language.

---

## Transport

Newline-delimited JSON over the worker's stdin/stdout.

- A **request** carries `id` and receives **exactly one** reply with the same
  `id`.
- A **notification** omits `id` and is never answered.
- Malformed input is ignored, never fatal.
- Anything on stdout that is not JSON is ignored.

```jsonc
// host -> plugin
{"id":1,"method":"ui/panel","params":{"panel_id":"main"}}

// plugin -> host
{"id":1,"result":{"panel_id":"main","widgets":[]}}
{"id":2,"error":{"code":"unsupported_method","message":"plugin does not handle 'x'"}}
```

The channel is symmetric. A plugin may send requests to the host, including from
inside a handler, which is how `host_call` is implemented.

---

## Methods the host calls on you

| Method | Direction | Params | Result |
|---|---|---|---|
| `describe` | request | — | `PluginInfo` |
| `ui/panel` | request | `{ panel_id }` | `{ panel_id, widgets[] }` |
| `discovery/list` | request | `{ limit, topics[] }` | `{ candidates[] }` |
| `audio/graph` | request | — | `{ units[] }` |
| `shutdown` | both | — | none |
| `ui/change` | notification | `WidgetChange` | — |
| `event/playback-started` | notification | `PlaybackEvent` | — |
| `event/playback-progress` | notification | `PlaybackEvent` | — |
| `event/playback-completed` | notification | `PlaybackEvent` | — |
| `event/library-changed` | notification | — | — |

`PluginInfo`:

```json
{ "id": "dev.example.x", "name": "X", "version": "0.1.0",
  "protocol": "1.0", "ui_panels": [{ "id": "main", "title": "Main" }] }
```

`PlaybackEvent`:

```json
{ "episode_id": "ep_…", "show_id": "sh_…", "title": "…",
  "position_secs": 1234.0, "duration_secs": 2400 }
```

`WidgetChange` — a control the user moved. `value` is passed through
uninterpreted: a number for a knob or slider, a boolean for a toggle, an object
for a compound control such as an EQ node.

```json
{ "panel_id": "dial", "widget_id": "amount", "value": 0.42 }
{ "panel_id": "curve", "widget_id": "curve",
  "value": { "band_id": "mid", "gain_db": 4.5, "frequency": 1000 } }
```

---

## Methods you may call on the host

| Method | Params | Result | Requires |
|---|---|---|---|
| `host/library/shows` | — | `[ShowSummary]` | `library-read` |
| `host/library/history` | — | `[HistoryEntry]` | `library-read` |
| `host/log` | `{ level, message }` | none (fire-and-forget) | — |

Data shapes are defined once in `poddies-plugin-api::library`. See
[plugin-authoring.md](plugin-authoring.md#reading-the-library).

---

## Widgets

```jsonc
// display
{ "type": "heading", "text": "…" }
{ "type": "metric",  "label": "Episodes", "value": "128" }
{ "type": "text",    "text": "…" }
{ "type": "divider" }
{ "type": "bar",     "label": "Completion", "value": 0.8, "max": 1.0 }
{ "type": "list",    "items": [ { "primary": "…", "secondary": "…" } ] }

// interactive — `id` comes back in a ui/change notification
{ "type": "knob",    "id": "amount", "label": "Amount", "value": 0.35,
  "min": 0.0, "max": 1.0, "unit": "", "style": "vintage",
  "readout": "2.4:1 · -9 dB" }
{ "type": "slider",  "id": "trim", "label": "Trim", "value": -2.0,
  "min": -12.0, "max": 12.0, "step": 0.5, "unit": "dB" }
{ "type": "toggle",  "id": "bypass", "label": "Bypass", "value": false }
{ "type": "eq",      "id": "curve", "bands": [
    { "id": "low", "label": "Low", "frequency": 90.0, "gain_db": 0.0,
      "q": 0.7, "kind": "low_shelf" } ] }
{ "type": "meter",   "id": "gr", "label": "Gain reduction",
  "source": "gain_reduction", "min_db": -18.0, "max_db": 0.0 }
```

`knob.style` is `modern` or `vintage`; `eq` bands are `peaking`, `low_shelf` or
`high_shelf`; `meter.source` is `peak` or `gain_reduction`. Meter values come
from the live audio graph, not from the plugin — the host paints them.

Unknown types are skipped, so the widget set can grow additively.

---

## Audio units

Returned from `audio/graph`. The plugin supplies parameters; the host builds the
nodes on the audio thread. Units apply in the order returned.

```jsonc
{ "type": "parametric_eq", "id": "eq", "enabled": true,
  "bands": [ { "id": "low", "label": "Low", "frequency": 90.0,
               "gain_db": 2.0, "q": 0.7, "kind": "low_shelf" } ] }

{ "type": "compressor", "id": "comp", "enabled": true,
  "threshold_db": -18.0, "ratio": 4.0, "attack_ms": 8.0,
  "release_ms": 400.0, "knee_db": 6.0, "makeup_db": 2.0 }
```

Requires the `audio-effects` capability. A `parametric_eq` band is one biquad;
`enabled: false` bypasses a unit without removing it from the graph.

---

## DiscoveryCandidate

```jsonc
{ "title": "…",                 // required
  "feed_url": "https://…",      // required
  "source": "dev.example.x",    // required — your plugin id
  "description": null,
  "image_url": null,
  "author": null,
  "categories": ["Technology"],
  "latest_published": "2026-08-01T00:00:00Z",
  "typical_duration_secs": 2400,
  "explicit": false,
  "popularity": null }          // tie-breaker only, never a weighted factor
```

Ranking is performed by the host. See
[architecture.md](architecture.md#the-discovery-algorithm).

---

## Error codes

| Code | Raised by | Meaning |
|---|---|---|
| `unsupported_method` | plugin or host | The receiver does not implement that method. |
| `invalid_params` | either | Arguments did not validate. |
| `serialize_failed` | either | A payload could not be encoded. |
| `bad_request` | SDK | Input was not valid JSON. |
| `bad_response` | SDK | A reply was not a valid `Reply`. |
| `host_unavailable` | SDK | The host link is closed, or the plugin is not inside a worker. |
| `plugin_error` | SDK | An unhandled handler error or panic. |
| `bad_host_data` | either | Host data did not match the documented shape. |
| `capability_denied` | host | The method needs a capability the manifest did not declare. |
| `timeout` | host | The plugin did not answer within the per-call limit. |
| `plugin_exited` | host | The worker died; pending calls are failed, not hung. |
| `handshake_failed` | host | `describe` failed during load. |
| `id_mismatch` | host | Runtime `id` differed from the manifest `id`. |
| `incompatible_protocol` | host | Protocol major mismatch. |

---

## Native (DLL) ABI

`ABI_VERSION = 2`. A native plugin exports `poddies_plugin_init`:

```c
const PluginVTable *poddies_plugin_init(const HostApi *api);

typedef struct {
    uint32_t   abi_version;                       // must equal ABI_VERSION
    char      *(*handle_line)(const char *req);   // NUL-terminated JSON line -> reply line
    void       (*free_string)(char *s);
    void       (*shutdown)(void);
} PluginVTable;

typedef struct {
    char *(*call)(const char *req);               // request with "id" waits for a reply;
    void  (*free_string)(char *s);                // request without "id" is fire-and-forget
} HostApi;
```

The worker loads the library with `libloading`, checks `abi_version`, then pumps
lines through `handle_line`. `handle_line` may call `HostApi::call` from inside
a handler; the worker serialises access to the pipes and matches the reply by
`id`, skipping any notification that races it.

In Rust, `export_plugin!` generates all of this.

---

## Limits

| Limit | Value |
|---|---|
| Per-call timeout | 5 s (`DEFAULT_REQUEST_TIMEOUT`) |
| Discovery timeout | 12 s (set by the host for `discovery/list`) |
| Handshake timeout | 10 s |
| Worker memory cap | 256 MiB |
| Worker process count | 4 (worker + interpreter + launcher + one slot) |
| Retained stderr lines | 50 |

A worker that exceeds the memory cap is killed by the OS. A worker that dies is
restarted on the next load; calls in flight fail with `plugin_exited` rather
than hanging.

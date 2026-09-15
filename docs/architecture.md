# Architecture

Poddies is a Rust host with a web UI. The domain logic, the plugin system and
the storage layer are library crates that know nothing about Tauri, which keeps
them testable and portable; the Tauri app is a thin shell over them.

```
┌────────────────────────────── poddies.exe ───────────────────────────────┐
│                                                                          │
│  Tauri host (app/src-tauri)                                              │
│    commands.rs   the entire surface the UI may call                      │
│    state.rs      AppState + plugin search paths                          │
│    services.rs   library data exposed to plugins (HostServices)          │
│                                                                          │
│  WebView2                                                                │
│    app/src       vanilla TypeScript, three panes, no framework           │
│                                                                          │
└───────────────┬──────────────────────────────────────────────────────────┘
                │ spawns, one process per plugin
                │ newline-delimited JSON over stdio
                ▼
┌────────────────── poddies.exe --plugin-worker <dir> ─────────────────────┐
│  poddies-plugin-host::worker                                             │
│    native  -> libloading -> plugin cdylib (C ABI v2)                     │
│    python  -> child interpreter, stdio relay                             │
│  Windows Job Object: kill-on-close, 256 MiB, bounded process count        │
└──────────────────────────────────────────────────────────────────────────┘
```

## Crates

| Crate | Responsibility | Depends on |
|---|---|---|
| `poddies-core` | Feed parsing, the library store, the discovery ranking algorithm, playback state. No UI, no platform code. | — |
| `poddies-plugin-api` | The versioned contract: protocol messages, widget schema, library data shapes, the C ABI. | core |
| `poddies-plugin-sdk` | Rust authoring surface: the `Plugin` trait, `host_call`, `export_plugin!`. | api |
| `poddies-plugin-host` | The sandboxed worker and the host-side manager (spawn, timeout, capability checks, crash detection, restart). | api, core |
| `poddies-cli` | Scaffolding, validation, the hot-reload dev loop. | host, api |
| `poddies` (app) | Tauri host, window and tray, commands, Mica, plugin wiring. | core, api, host |

The `poddies-plugin-worker` binary in `poddies-plugin-host` exists for
development and for `poddies dev`; the shipped app re-enters **its own binary**
in worker mode, which is how the product stays a single portable executable.

## Data flow

```
feed URL ──► poddies-core::fetch ──► parse_feed ──► Library::subscribe
                                                         │
                                        library.json  ◄──┘  (atomic write)
                                                         │
Discovery plugins ──► candidates ──► rank(candidates, profile, weights) ──► UI
                                                         ▲
                          listening history ─────────────┘
```

`Library` is a map of `Show`/`Episode`/`PlaybackState` plus `Settings`,
persisted as one JSON document. For a personal library (tens of shows, thousands
of episodes) that stays well under a megabyte, and it makes the store
inspectable — the same reasoning the app applies everywhere else. Writes are
atomic: write a temp file, then rename over the target.

Playback state is recorded with `record_play`, which keeps completion sticky and
only increments `play_count` when an episode crosses the finish line (within 30
seconds of the end, matching how podcast apps treat trailing credits).

## The sandbox

Plugins are not trusted to be correct, and are not assumed to be hostile either.
The design targets the realistic failure modes — a panic, an infinite loop, a
memory leak, a crash — and is explicit about what it does not cover.

**What is guaranteed**

- **Crash isolation.** A plugin runs in its own process. If it segfaults, the
  host survives; pending calls fail with `plugin_exited` rather than hanging.
- **Hang containment.** Every call has a timeout. A wedged plugin is reported as
  `timeout` and the app keeps working.
- **Resource bounds.** The worker is assigned to a Windows Job Object with
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` (no orphans when the host exits),
  `JOB_OBJECT_LIMIT_PROCESS_MEMORY` (256 MiB) and
  `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` (4 — the worker, a Python interpreter, the
  `py` launcher hop, and one slot of slack). The limit is deliberately not 1:
  a Python plugin legitimately needs to spawn its interpreter.
- **Capability enforcement.** A plugin cannot call a method outside its declared
  capabilities; the host refuses before the plugin sees it.
- **No UI surface to attack.** Plugins return declarative widgets, never HTML or
  JavaScript, so they cannot inject markup into the webview.

**What is not guaranteed**

- This is **not a security boundary**. A native plugin runs with the user's
  privileges and can read what the user can read. Treat a native plugin as code
  you have chosen to run, exactly like a browser extension with full disk
  access. A real security sandbox would need an AppContainer or a low-integrity
  token, which is future work.
- Job Objects are Windows-only. On other platforms the worker still runs as a
  separate process (so crash isolation holds) but the memory and process caps do
  not apply; cgroups or `rlimit`s would go in `sandbox.rs`.

If a Job Object cannot be created — for example when the host is itself already
inside a job that forbids nesting — the plugin runs unsandboxed and the host
logs it rather than failing to load.

## The discovery algorithm

Deliberately not a black box. The score is a linear combination of named
factors, and the result carries the full breakdown so the UI can show it.

```
score = Σ (weightᵢ × factorᵢ) − explicit_penalty − diversity_penalty
```

| Factor | Raw value |
|---|---|
| `topic_affinity` | `1 − exp(−Σ category_weight)`, saturating so several weak matches cannot beat one strong one. |
| `completion_signal` | The listener's mean completion ratio across history. |
| `recency` | `exp(−days / 30)` since the candidate's latest episode. |
| `novelty` | `1 − max Jaccard(title tokens, subscribed show titles)`. |
| `duration_fit` | `1 − |candidate − preferred| / preferred`, clamped. |
| `explicit_penalty` | `−weight × 1` when the show is explicit and the user filters it. |
| `diversity` | `−weight × max Jaccard(categories, already-chosen categories)`. |

Weights live in `Settings::discovery`, default to a sane spread, and are
persisted with the library. The profile is rebuilt from listening history on
every request, so the queue follows what you actually finish.

Diversity is applied greedily, maximal-marginal-relevance style: candidates are
chosen one at a time, each time discounting those too similar to what has already
been picked. `popularity` from a source is used only as a deterministic
tie-breaker (`+1e-6 × popularity`), so it can never override a preference-driven
difference.

Every factor produces a human-readable label derived directly from the data that
produced it ("Matches your interest in Technology", "Latest episode 3 days ago",
"You finish about 89% of what you start"). No generated text, no model.

The queue is asserted to be explainable: a test checks that the contributions sum
to the score, so the explanation cannot drift from the ranking.

## Frontend

Vanilla TypeScript and a single stylesheet — no framework, no runtime
dependencies beyond the Tauri API. The whole bundle is about 26 KB of JavaScript,
21 KB of CSS and 160 KB of font files (latin subsets).

- `api.ts` — typed wrappers over `invoke`, mirroring the Rust view types.
- `main.ts` — state, routing over the middle pane's views (discovery, search,
  latest, a show, a plugin panel, settings), the playback transport, and
  rendering. Rendering is direct DOM construction with a small `el()` helper;
  panes are rebuilt on change and the scrubber is updated in place so playback
  never triggers a re-render loop.
- `styles.css` — Geist (bundled, latin subset), then tokens, then sections for
  shell, panes, search, rows, discovery, now playing, widgets, settings, motion,
  and responsive collapse.

**Search** queries the library in Rust (titles, authors, categories, episode
titles and descriptions) and, on the same call, the public Apple Podcasts
endpoint, so a query surfaces shows that are not subscribed yet as one-click
Add rows. Results order: the user's shows, then addable Apple results, then
episodes — the addable items stay above a potentially long episode list.

## Window, tray and motion

The window is transparent with `decorations: false`, so Mica shows through the
app's translucent scrim, and the shell carries its own rounded corners and 1px
hairline. `shadow` is disabled: with a transparent undecorated window, Windows
otherwise adds an invisible resize frame that shows the desktop around the edges.

Closing and minimising both hide the window to the tray; the tray menu owns the
real exit. A `Resized` handler converts a taskbar-initiated minimise into a tray
hide, so there is no titlebar-less state the user cannot escape.

**Dragging belongs to the app, not to Tauri's injected script.** The titlebar
owns a `mousedown` handler that calls a Rust command wrapping
`Window::start_dragging()`. This matters for two reasons: the injected
`data-tauri-drag-region` script and an app handler can race — the second call's
`ReleaseCapture` cancels the native move loop the first one just entered, and the
window then refuses to move — and keeping one code path means buttons inside the
bar stay clickable because a drag only starts on chrome that is not a button or
field. Double-clicking the chrome toggles maximise, as on a native titlebar.

The wordmark ships as the logo's dark ink, so on the default dark material it is
CSS-inverted to the near-white the UI text sits at; in light mode it is drawn as
authored. The logo's transparent surround stays untouched, so it never gains a
box on any wallpaper.

Restoring emits `poddies://reveal`, which plays the signature bloom: a soft pill
of light expands across the top with a decaying blur while the content settles
out of a blur and a slight scale — the "island opening" read, built purely from
CSS filters and transforms on a fixed overlay.

## Performance

- No UI framework; Geist is bundled so there is no font request. Cold start is
  dominated by WebView2 initialisation.
- List rendering is bounded: the middle pane renders up to 300 latest episodes or
  one show's full list, and panes are only rebuilt when data or route changes.
- `opt-level = "s"`, LTO and `strip = true` in release.
- Apple Podcasts lookups and feed refreshes are the slow work, and both stay off
  the interactive path: search holds no library lock across its network call, and
  refreshes are conditional (`If-None-Match` / `If-Modified-Since`), so an
  unchanged feed costs one round trip and no parse.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/logo-light.png">
    <img src="docs/logo.png" alt="poddies" width="300">
  </picture>
</p>

<h3 align="center">An ultra-minimal podcast client for Windows</h3>

<p align="center">
  Super lean Podcasts app built in Rust with deep customisation.
</p>

<p align="center">
  <a href="#quick-start"><img alt="Windows 11 · Mica" src="https://img.shields.io/badge/Windows%2011-Mica-24292f?style=flat-square"></a>
  <a href="https://www.rust-lang.org"><img alt="Rust" src="https://img.shields.io/badge/Rust-stable-24292f?style=flat-square&logo=rust"></a>
  <a href="https://v2.tauri.app"><img alt="Built with Tauri" src="https://img.shields.io/badge/Tauri-2.0-24C6D8?style=flat-square&logo=tauri&labelColor=24292f"></a>
  <img alt="Portable · one exe" src="https://img.shields.io/badge/Portable-one_EXE-2ea043?style=flat-square">
  <img alt="License MIT" src="https://img.shields.io/badge/License-MIT-8B949E?style=flat-square">
</p>

<p align="center">
  <img alt="Poddies, showing the Discovery queue, adjustable ranking weights, and the now-playing pane" src="docs/screenshot.png" width="820">
</p>

---

Easy to set up and use on all platforms.

- **Subscriptions** — RSS 2.0, Atom, RSS 1.0 and JSON Feed all parse, with
  conditional refreshes (`If-None-Match` / `If-Modified-Since`) so checking for
  new episodes costs nothing when nothing changed. Add a feed from the dialog in
  the sidebar, and drop one from the same row on hover.
- **Search** — your whole library, plus [Apple Podcasts](https://itunes.apple.com/search)
  over its public endpoint, no key and no account, so you can find and add shows
  you haven't subscribed to yet.
  

  <p align="left">
    <img alt="Searching for design: local matches on top, then addable Apple Podcasts results with Subscribe buttons" src="docs/search.png" width="820">
  </p>

- **Discovery that shows its work.** Adjust your own algorithm to your liking in detail!
- **A plugin system that is genuinely easy** — You can basically make this your own app with a little time (or Claude). Switch plugins on and off from Settings and it takes effect straight away, no restart.
- **Featherweight** — 9.7 MB exe, ~27 KB of js, light and easy on ur cpu :)
  

## Quickstart Guide
Just install from Releases. If you want to build it yourself:

```powershell
# builds the portable exe and the Windows installer into .\dist
pwsh scripts\build-release.ps1
```

Or just the portable binary — the UI must be compiled first, because Tauri
embeds it into the exe:

```powershell
cd app ; pnpm install ; pnpm build ; cd ..
cargo build --release -p poddies
```

```
dist\poddies.exe                    # portable, no install
dist\Poddies_0.1.0_x64-setup.exe    # per-user installer, no admin prompt
```

Development uses the Vite dev server instead of the embedded bundle — disable the
production feature for a live-reload loop:

```powershell
cd app ; pnpm dev                               # terminal 1
cargo run -p poddies --no-default-features       # terminal 2
```

**Requirements:** Windows 11 (Mica; Windows 10 works without it), WebView2
(bundled with Windows 11), Rust stable with the MSVC toolchain, Node 20+ with
pnpm. Python 3 on `PATH` only if you want to run Python plugins.

## Plugins

Plugins never run inside the app. The host spawns a worker — *its own binary,
re-invoked* as `poddies.exe --plugin-worker <dir>` — and talks to it over
newline-delimited JSON on stdio. A native plugin is a `cdylib` the worker loads
through a stable C ABI; a Python plugin is a script it runs. Either way the
plugin lives in its own process, so a panic, hang or hard crash takes down the
worker and nothing else, and the host restarts it.

```powershell
poddies-cli new plugin my-panel --lang rust       # scaffold
poddies-cli dev .\plugins\my-panel                # build · load · reload on save
poddies-cli check .\plugins\my-panel              # validate
poddies-cli docs                                  # API reference in the terminal
```

A plugin's whole surface is one trait — `describe`, `on_request`, optional hooks —
and the host renders everything it returns with the app's own typography:

```rust
impl Plugin for MyStats {
    fn info(&self) -> PluginInfo { /* id, name, ui_panels */ }

    fn on_request(&mut self, method: &str, _params: Value) -> Result<Value, PluginError> {
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

### Custom UI, three ways

A panel says where it wants to live, and the plugin gets a real interface —
not a settings list:

| Placement | Where it appears | Good for |
|---|---|---|
| `sidebar` | A section in the library, listed with the navigation | stats, reading lists |
| `now_playing` | Docked in the now-playing pane, under the speed control | controls you want at hand while listening |
| `popout` | A floating sheet opened from the plugin manager | anything that needs room — a curve, a spectrum |

Widgets are declarative and **interactive**. A plugin gives a control an `id`,
the host reports the movement with a `ui/change` notification, and the plugin
answers the next `ui/panel` with fresh state:

```rust
Widget::Knob   { id: "amount", label: "Amount", value: 0.35, style: KnobStyle::Vintage, .. }
Widget::Slider { id: "trim",   label: "Trim",   value: -2.0, unit: "dB", .. }
Widget::Toggle { id: "bypass", label: "Bypass", value: false }
Widget::Eq     { id: "curve",  bands, .. }
Widget::Meter  { id: "gr",     label: "Gain reduction", source: MeterSource::GainReduction, .. }
```

Plugins do not ship HTML, CSS or JavaScript. The host draws every control, which
is why a panel looks like the rest of the app in both the window and the popout,
and why a plugin cannot inject markup into the webview.

### Plugins and the audio pipeline

A plugin can put its own processing in the playback chain, without touching a
single sample. It declares the units it wants from `audio/graph` and the host
builds real Web Audio nodes:

```rust
AudioUnit::ParametricEq { id: "eq", bands: vec![/* low shelf, peaks, high shelf */] }
AudioUnit::Compressor   { id: "comp", threshold_db: -18.0, ratio: 4.0,
                          attack_ms: 8.0, release_ms: 400.0, makeup_db: 2.0, .. }
```

The plugin supplies **parameters**, the host owns the **DSP**. That split is what
keeps it fast and keeps it honest: nothing crosses the process boundary per
sample, so latency is unchanged, and a plugin written in Python is exactly as
viable as one in Rust. The `meter` widget is filled by the host straight from
the live graph, so a gain-reduction meter is real rather than reported.

Four reference plugins ship in [`plugins/`](plugins) and cover the whole
lifecycle, each built as a real `.dll`:

| Plugin | Capability | Demonstrates |
|---|---|---|
| **Listening Stats** | `ui-panel`, `library-read` | host calls, aggregating history, declarative widgets |
| **Apple Podcasts** | `discovery-source` | feeding the Discovery queue from the public Apple Podcasts API (no key), caching, offline fallback |
| **Parametric EQ** | `ui-panel`, `audio-effects` | a five-band curve in a popout, where the drawn response comes from the same coefficients as the audio |
| **Compressor** | `ui-panel`, `audio-effects` | one vintage knob that moves four compressor parameters, docked under the speed control, with a live gain-reduction meter |
| **Listening Clock** | `ui-panel`, `library-read` | habits rather than counts: day streaks, hour-of-day rhythm and weekday balance from timestamps alone |
| **Night Listening** | `ui-panel`, `audio-effects` | composing both audio units — compressor plus EQ — into one late-night chain driven by a single intensity slider |

Settings lists every plugin with its state, a Reload button and an on/off
switch: disabling stops it at once, takes its panel out of the interface and
stops it contributing to Discovery, and the choice survives a restart. "Open
folder" shows you where to drop a plugin.

| | |
|---|---|
| API reference | [`docs/plugin-api.md`](docs/plugin-api.md) |
| Authoring guide | [docs/plugin-authoring.md](docs/plugin-authoring.md) |
| Architecture | [docs/architecture.md](docs/architecture.md) |

## Discovery, without a black box

```
score = Σ (weight × factor) − explicit_penalty − diversity_penalty
```

Candidates come from plugins. Ranking belongs to the host, as a linear
combination of **topic match**, **your follow-through**, **newness**, **novelty**,
**length fit**, an **explicit filter** and **variety** — the sliders in Settings.
Every recommendation carries the contributions that produced it as reason chips,
and `popularity` from a source is only ever a tie-breaker, so nothing can quietly
outweigh your own listening. The full maths is in
[architecture.md](docs/architecture.md#the-discovery-algorithm).

## Repository

```
crates/poddies-core/         feeds, library, discovery ranking — no UI, no platform
crates/poddies-plugin-api/   the versioned contract: protocol, widgets, C ABI
crates/poddies-plugin-sdk/   Rust authoring surface
crates/poddies-plugin-host/  the sandboxed worker and its manager
crates/poddies-cli/          scaffolding, dev loop, validation
app/                         the Tauri host and the interface
python/poddies/              the Python plugin SDK
scripts/build-release.ps1    portable bundle + Windows installer
docs/                        authoring guide, API reference, architecture
```

```powershell
cargo test --workspace      # 46 tests across five crates
cargo clippy --workspace --all-targets
```

## Licence

[MIT](LICENSE).

import { listen } from "@tauri-apps/api/event";

import * as engine from "./audio";
import {
  paintMeters,
  renderWidget as renderPluginWidget,
  patchWidgets,
  type WidgetHost,
  isInteracting,
} from "./widgets";
import {
  api,
  type DiscoveryItemView,
  type EpisodeView,
  type LibraryView,
  type PanelContent,
  type PanelView,
  type PluginStatusView,
  type SearchView,
  type Weights,
} from "./api";

/* ------------------------------------------------------------------ state */

type Route =
  | { kind: "discovery" }
  | { kind: "latest" }
  | { kind: "show"; showId: string }
  | { kind: "panel"; pluginIndex: number; panelId: string }
  | { kind: "settings" }
  | { kind: "search" };

const DEFAULT_WEIGHTS: Weights = {
  topic_affinity: 1.0,
  completion_signal: 0.5,
  recency: 0.35,
  novelty: 0.55,
  duration_fit: 0.25,
  explicit_penalty: 1.0,
  diversity: 0.4,
};

const WEIGHT_LABELS: [keyof Weights, string][] = [
  ["topic_affinity", "Topic match"],
  ["completion_signal", "Your follow-through"],
  ["recency", "Newness"],
  ["novelty", "Novelty"],
  ["duration_fit", "Length fit"],
  ["explicit_penalty", "Explicit filter"],
  ["diversity", "Variety"],
];

const DISCOVERY_LIMIT = 24;

const state = {
  library: { shows: [], latest: [], in_progress: [] } as LibraryView,
  route: { kind: "discovery" } as Route,
  panels: [] as PanelView[],
  statuses: [] as PluginStatusView[],
  pluginsDir: "",
  query: "",
  search: { shows: [], episodes: [], web: [] } as SearchView,
  searchPending: false,
  discovery: [] as DiscoveryItemView[],
  discoveryLoading: false,
  weights: { ...DEFAULT_WEIGHTS },
  avoidExplicit: false,
  episodes: [] as EpisodeView[],
  panelContent: null as PanelContent | null,
  /** Panel content by `<plugin_index>:<panel_id>`, so the docked and popout
   * panels can render from cache without a round trip per frame. */
  panelContents: new Map<string, PanelContent>(),
  current: null as EpisodeView | null,
  playing: false,
  position: 0,
  duration: 0,
  rate: 1,
  episodeCache: new Map<string, EpisodeView[]>(),
};

/* --------------------------------------------------------------- dom utils */

interface ElOptions {
  class?: string;
  text?: string;
  attrs?: Record<string, string>;
  on?: Record<string, EventListener>;
  title?: string;
}

function el(
  tag: string,
  options: ElOptions = {},
  children: (Node | string | null)[] = [],
): HTMLElement {
  const node = document.createElement(tag);
  if (options.class) node.className = options.class;
  if (options.text !== undefined) node.textContent = options.text;
  if (options.title) node.title = options.title;
  if (options.attrs) {
    for (const [key, value] of Object.entries(options.attrs)) {
      node.setAttribute(key, value);
    }
  }
  if (options.on) {
    for (const [event, handler] of Object.entries(options.on)) {
      node.addEventListener(event, handler);
    }
  }
  for (const child of children) {
    if (child !== null) node.append(child);
  }
  return node;
}

function button(
  tag: string,
  className: string,
  label: string,
  on?: EventListener,
  children: Node[] = [],
): HTMLButtonElement {
  const node = el(tag, {
    class: className,
    attrs: { type: "button" },
    title: label,
    on: on ? { click: on } : {},
  }) as HTMLButtonElement;
  node.setAttribute("aria-label", label);
  for (const child of children) node.append(child);
  return node;
}

/**
 * Warm a URL in the image cache. Re-rendering a list re-creates every
 * `artwork()` node; a warmed URL is served from memory and paints in the same
 * frame, so a rebuild is invisible instead of a refill-from-network blink.
 */
const warm = new Map<string, Promise<unknown>>();
function warmImage(url: string): void {
  if (warm.has(url)) return;
  const image = new Image();
  image.src = url;
  warm.set(url, image.decode().catch(() => {}));
}

/** Artwork with a quiet monochrome fallback so a missing image is invisible. */
function artwork(url: string | null, className: string): HTMLElement {
  const fallback = () =>
    el("span", { class: `${className} artwork-fallback`, attrs: { "aria-hidden": "true" } });

  if (!url) return fallback();
  warmImage(url);

  const img = document.createElement("img");
  img.className = className;
  img.loading = "lazy";
  img.decoding = "async";
  img.alt = "";
  img.src = url;
  img.addEventListener(
    "error",
    () => {
      img.replaceWith(fallback());
    },
    { once: true },
  );
  return img;
}

/**
 * Replace a persistent container's contents — but only when they actually
 * changed, and then without losing scroll state.
 *
 * This is the anti-flicker primitive the app leans on: most render calls are
 * *state refreshes* (play/pause, a preference committed, a plugin reloaded)
 * that paint the same tree. Rebuilding identical content is what makes images
 * blink, sliders jump under the pointer and scroll positions reset, so a
 * candidate tree is built offscreen first. When it does change, scroll
 * offsets are recorded and re-applied.
 */
function swap(container: HTMLElement, build: () => Node[]): void {
  const candidate = el("div", {}, build());
  if (renderedSignature(candidate) === renderedSignature(container)) return;

  // A widget is mid-drag: replacing its node would kill the gesture. Defer the
  // swap to the next idle moment instead of fighting the user's pointer.
  if (isInteracting()) {
    deferWhileInteracting(() => swap(container, build));
    return;
  }

  // Scrollable regions inside the subtree are being replaced wholesale; keep
  // their offsets, matched by position, so nothing scrolls back to the top.
  const containerTop = container.scrollTop;
  const before = [...container.querySelectorAll<HTMLElement>(".pane__scroll")].map(
    (node) => node.scrollTop,
  );

  container.replaceChildren(...candidate.childNodes);

  container.scrollTop = Math.min(containerTop, container.scrollHeight);
  const fresh = [...container.querySelectorAll<HTMLElement>(".pane__scroll")];
  fresh.forEach((node, index) => {
    node.scrollTop = before[index] ?? 0;
  });
}

const deferred: (() => void)[] = [];

function deferWhileInteracting(fn: () => void): void {
  deferred.push(fn);
}

/**
 * Flush queued UI updates after every pointer release; while a drag is still
 * held (several defers may stack), nothing runs.
 */
function installDeferredFlush(): void {
  window.addEventListener("pointerup", () => {
    window.setTimeout(() => {
      if (isInteracting() || deferred.length === 0) return;
      for (const task of deferred.splice(0)) task();
    }, 0);
  });
}

/**
 * The serialized DOM of a subtree, with live-painted state neutralized so it
 * never counts as a content change. Plugin meters are repainted by the meter
 * loop between renders; their fill width, readout text and hot class are
 * engine output, not content, so they are rewritten to the canonical values a
 * fresh render would produce before comparison.
 */
function renderedSignature(container: HTMLElement): string {
  const copy = container.cloneNode(true) as HTMLElement;
  for (const node of copy.querySelectorAll<HTMLElement>("[data-meter]")) {
    const fill = node.querySelector<HTMLElement>(".meter__fill");
    if (fill) fill.style.width = "";
    const readout = node.querySelector<HTMLElement>(".meter__readout");
    if (readout) readout.textContent = "—";
    node.classList.remove("meter--hot");
  }
  return copy.innerHTML;
}

function svg(paths: string, size = 14): SVGElement {
  const node = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  node.setAttribute("width", String(size));
  node.setAttribute("height", String(size));
  node.setAttribute("viewBox", "0 0 16 16");
  node.setAttribute("fill", "none");
  node.setAttribute("aria-hidden", "true");
  node.innerHTML = paths;
  return node;
}

const GLYPH = {
  discovery:
    '<circle cx="8" cy="8" r="5.6" stroke="currentColor" stroke-width="1.2"/><path d="M8 2.4v1.4M8 12.2v1.4M2.4 8h1.4M12.2 8h1.4" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/><circle cx="8" cy="8" r="1.6" fill="currentColor"/>',
  latest:
    '<path d="M3 4.2h10M3 8h10M3 11.8h6.4" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>',
  panel:
    '<rect x="2.6" y="2.6" width="10.8" height="10.8" rx="2" stroke="currentColor" stroke-width="1.2"/><path d="M2.6 6.4h10.8M6.4 6.4v7" stroke="currentColor" stroke-width="1.2"/>',
  play: '<path d="M5.4 3.6l7 4.4-7 4.4z" fill="currentColor"/>',
  pause: '<path d="M5.6 3.8h1.8v8.4H5.6zM8.6 3.8h1.8v8.4H8.6z" fill="currentColor"/>',
  back15:
    '<path d="M7.2 3.2 4 6.2l3.2 3" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/><path d="M4.2 6.2h4.6a4 4 0 1 1-4 5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>',
  fwd15:
    '<path d="M8.8 3.2 12 6.2l-3.2 3" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/><path d="M11.8 6.2H7.2a4 4 0 1 0 4 5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>',
  search:
    '<circle cx="7" cy="7" r="4.4" stroke="currentColor" stroke-width="1.3"/><path d="M10.2 10.2 13 13" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>',
  settings:
    '<circle cx="8" cy="8" r="2.2" stroke="currentColor" stroke-width="1.2"/><path d="M8 2v2M8 12v2M2 8h2M12 8h2M3.8 3.8l1.4 1.4M10.8 10.8l1.4 1.4M12.2 3.8l-1.4 1.4M5.2 10.8l-1.4 1.4" stroke="currentColor" stroke-width="1.1" stroke-linecap="round"/>',
  reload:
    '<path d="M12.4 8a4.4 4.4 0 1 1-1.3-3.1" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/><path d="M12.4 1.6v3.3H9.1" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/>',
};

function fmtDuration(secs: number | null): string {
  if (!secs || secs <= 0) return "";
  const hours = Math.floor(secs / 3600);
  const minutes = Math.round((secs % 3600) / 60);
  return hours > 0 ? `${hours}h ${String(minutes).padStart(2, "0")}m` : `${minutes}m`;
}

function fmtClock(secs: number): string {
  if (!Number.isFinite(secs) || secs < 0) return "0:00";
  const total = Math.floor(secs);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  const pad = (value: number) => String(value).padStart(2, "0");
  return hours > 0
    ? `${hours}:${pad(minutes)}:${pad(seconds)}`
    : `${minutes}:${pad(seconds)}`;
}

function fmtDate(iso: string | null): string {
  if (!iso) return "";
  const then = new Date(iso);
  if (Number.isNaN(then.getTime())) return "";
  const days = Math.floor((Date.now() - then.getTime()) / 86_400_000);
  if (days <= 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days < 7) return `${days} days ago`;
  if (days < 30) return `${Math.floor(days / 7)} weeks ago`;
  return then.toLocaleDateString(undefined, { day: "numeric", month: "short" });
}

/* -------------------------------------------------------------------- dom */

const dom = {
  app: document.getElementById("app") as HTMLElement,
  library: document.getElementById("pane-library") as HTMLElement,
  libraryScroll: document.getElementById("library-scroll") as HTMLElement,
  libraryFooterNav: document.getElementById("library-footer-nav") as HTMLElement,
  middle: document.getElementById("pane-middle") as HTMLElement,
  now: document.getElementById("pane-now") as HTMLElement,
  titlebar: document.querySelector(".titlebar") as HTMLElement,
  status: document.getElementById("status") as HTMLElement,
  refresh: document.getElementById("btn-refresh") as HTMLButtonElement,
  settings: document.getElementById("btn-settings") as HTMLButtonElement,
  minimize: document.getElementById("btn-minimize") as HTMLButtonElement,
  close: document.getElementById("btn-close") as HTMLButtonElement,
  search: document.getElementById("library-search") as HTMLInputElement,
  searchClear: document.getElementById("library-search-clear") as HTMLButtonElement,
  panelDialog: document.getElementById("panel-dialog") as HTMLDialogElement,
  panelDialogTitle: document.getElementById("panel-dialog-title") as HTMLElement,
  panelDialogBody: document.getElementById("panel-dialog-body") as HTMLElement,
  panelDialogClose: document.getElementById("panel-dialog-close") as HTMLButtonElement,
  addFeedOpen: document.getElementById("add-feed-open") as HTMLButtonElement,
  addFeedDialog: document.getElementById("add-feed") as HTMLDialogElement,
  addFeedForm: document.getElementById("add-feed-form") as HTMLFormElement,
  addFeedInput: document.getElementById("add-feed-input") as HTMLInputElement,
  addFeedError: document.getElementById("add-feed-error") as HTMLElement,
  addFeedSubmit: document.getElementById("add-feed-submit") as HTMLButtonElement,
  addFeedCancel: document.getElementById("add-feed-cancel") as HTMLButtonElement,
};

function installSearchField(): void {
  const input = dom.search;
  input.enterKeyHint = "search";

  input.addEventListener("input", onSearchInput);
  input.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      input.value = "";
      onSearchInput();
    }
  });

  // Mousedown is what normally blurs the field before the click lands; stop
  // that so clearing keeps the caret.
  dom.searchClear.addEventListener("mousedown", (event) => {
    event.preventDefault();
  });
  dom.searchClear.addEventListener("click", () => {
    input.value = "";
    onSearchInput();
  });
}

function renderSearchClear(): void {
  dom.searchClear.classList.toggle("is-visible", dom.search.value !== "");
}

let searchSeq = 0;
let searchDebounce: number | undefined;

/** The "Matching shows" rail updates instantly from state — no rebuild, no
 * await, so typing into the field is never interrupted. The full results pane
 * (episodes + Apple Podcasts) follows on a short debounce.
 */
function onSearchInput(): void {
  const query = dom.search.value;
  state.query = query;
  renderSearchClear();
  window.clearTimeout(searchDebounce);

  if (query === "") {
    state.search = { shows: [], episodes: [], web: [] };
    state.searchPending = false;
    if (state.route.kind === "search") state.route = { kind: "latest" };
    render();
    return;
  }

  state.searchPending = true;
  if (state.route.kind !== "search") state.route = { kind: "search" };
  renderLibrary(); // reroutes the "Matching shows" rail from the last snapshot

  searchDebounce = window.setTimeout(() => void runSearch(query), 60);
}

async function runSearch(query: string): Promise<void> {
  const seq = ++searchSeq;

  let result: SearchView;
  try {
    result = await api.search(query);
  } catch {
    result = { shows: [], episodes: [], web: [] };
  }
  if (seq !== searchSeq) return;
  if (query !== state.query) return;

  state.search = result;
  state.searchPending = false;
  if (state.route.kind !== "search") state.route = { kind: "search" };
  renderLibrary();
  renderMiddle();
}

let statusTimer: number | undefined;

/// The titlebar moves the window. Poddies ships its own handler rather than
/// relying only on the injected `data-tauri-drag-region` script, so dragging
/// works even when that script is unavailable — and buttons inside the bar stay
/// clickable, because a drag only starts on the chrome itself.
function installWindowDragging(): void {
  dom.titlebar.addEventListener("mousedown", (event) => {
    if (event.button !== 0) return;
    const target = event.target as HTMLElement | null;
    // Interactive chrome opts out; everything else in the bar is a handle.
    if (target && target.closest("button, input, img")) return;
    void api.appStartDrag();
  });

  // Double-clicking the chrome toggles maximise, as on a native titlebar.
  dom.titlebar.addEventListener("dblclick", (event) => {
    const target = event.target as HTMLElement | null;
    if (target && target.closest("button, input, img")) return;
    void api.appToggleMaximise();
  });
}

function showStatus(message: string): void {
  dom.status.textContent = message;
  dom.status.classList.add("is-visible");
  window.clearTimeout(statusTimer);
  statusTimer = window.setTimeout(() => {
    dom.status.classList.remove("is-visible");
  }, 4000);
}

/* --------------------------------------------------------------- playback */

const audio = new Audio();
audio.preload = "metadata";

let lastReported = 0;

/* Player discipline (modelled on how players like VLC and Apple Podcasts treat
 * themselves): a media element is fed same-origin URLs through the app's media
 * proxy whenever its bytes are routed through the plugin graph, a resume is
 * prepared before playback (metadata first, playhead placed as soon as the
 * duration is known), scrubbing is a scrub→intent→commit interaction rather
 * than a seek-per-pixel, and finishing an episode advances to the next one.
 */

const streamCache = new Map<string, Promise<string>>();

/** The playback URL for an enclosure. Direct when the element plays as itself;
 * same-origin proxied when (or once) plugin units route its samples, since
 * routed cross-origin media is silenced by the browser. */
function playableUrl(episode: EpisodeView): Promise<string> {
  const direct = episode.enclosure_url || "";
  if (!engine.isRouted() || !direct) {
    return Promise.resolve(direct);
  }
  const existing = streamCache.get(direct);
  if (existing) return existing;
  const pending = api
    .streamFor(direct)
    .catch((error) => {
      streamCache.delete(direct);
      throw error;
    });
  streamCache.set(direct, pending);
  return pending;
}

/** Where an episode resumes: nowhere for a tail you already played. */
function resumePositionFor(episode: EpisodeView): number {
  const duration = episode.duration_secs ?? 0;
  if (episode.completed || (duration > 0 && episode.position_secs >= duration - 15)) return 0;
  return episode.position_secs > 5 ? episode.position_secs : 0;
}

/** Set at play time when there is a position to restore; consumed when the
 * metadata makes seeking into the stream legal. */
let pendingSeekSecs: number | null = null;

function reportProgress(force: boolean, completed = false): void {
  const episode = state.current;
  if (!episode) return;
  const now = Date.now();
  if (!force && now - lastReported < 5000) return;
  lastReported = now;
  const position = completed ? state.duration : audio.currentTime;
  const duration = Number.isFinite(audio.duration) && audio.duration > 0
    ? Math.round(audio.duration)
    : state.duration || null;
  void api.recordProgress(episode.id, position, duration, completed);
}

async function playEpisode(episode: EpisodeView): Promise<void> {
  if (state.current?.id === episode.id) {
    togglePlay();
    return;
  }

  reportProgress(true);
  state.current = episode;
  state.duration = episode.duration_secs ?? 0;
  const from = resumePositionFor(episode);
  state.position = from;
  pendingSeekSecs = from > 0 ? from : null;

  const src = await playableUrl(episode);
  if (!src) {
    showStatus("This episode has no audio attached");
    return;
  }
  if (audio.src !== src) audio.src = src;
  if (!pendingSeekSecs) audio.currentTime = 0;
  audio.playbackRate = state.rate;

  try {
    await audio.play();
    state.playing = true;
  } catch {
    state.playing = false;
    showStatus("Couldn't play this episode");
  }

  // A browser keeps the audio context suspended until a gesture.
  void engine.resume();

  void api.playbackStarted(episode.id);
  updateMediaSession(episode);
  renderMiddle();
  renderNowPlaying();
}

/** The play/pause press: resume from a restored position on a fresh open. */
function togglePlay(): void {
  if (!state.current) return;
  if (audio.paused) {
    void engine.resume();
    void audio.play().then(() => {
      state.playing = true;
      syncTransport();
    });
  } else {
    audio.pause();
  }
}

/**
 * Apple Podcasts-style resume: the episode is *ready* before it is played.
 * Called once the audio graph has settled, because the proxy decision depends
 * on whether plugin units route the element. The stream preloads metadata and
 * the playhead is placed the instant seeking into it is legal, but playback
 * waits for the user's press: a fresh open of the app resumes, it does not
 * autostart.
 */
function prepareResume(): void {
  const episode = state.current;
  if (!episode || state.playing) return;
  pendingSeekSecs = state.position > 0 ? state.position : null;
  void (async () => {
    try {
      const src = await playableUrl(episode);
      // Only swap the source when it is not already the intended one.
      if (src && !state.playing && audio.src !== src) {
        audio.src = src;
        audio.playbackRate = state.rate;
      }
    } catch {
      /* network trouble at open: the play button will ask again */
    }
  })();
}

/** The episode after `current` in the list the user is listening along — the
 * natural "up next" of a show view, Latest as the default. */
function nextEpisodeAfter(currentId: string): EpisodeView | null {
  const list =
    state.route.kind === "show" && state.episodes.length > 0
      ? state.episodes
      : state.library.latest;
  const index = list.findIndex((episode) => episode.id === currentId);
  return index >= 0 ? list[index + 1] ?? null : null;
}

function seekBy(delta: number): void {
  if (!state.current) return;
  const limit = Number.isFinite(audio.duration) ? audio.duration : state.position + delta;
  audio.currentTime = Math.max(0, Math.min(limit, audio.currentTime + delta));
  reportProgress(true);
  refreshScrubber();
}

function updateMediaSession(episode: EpisodeView): void {
  if (!("mediaSession" in navigator)) return;
  navigator.mediaSession.metadata = new MediaMetadata({
    title: episode.title,
    artist: episode.show_title,
    album: "Poddies",
    artwork: episode.image_url ? [{ src: episode.image_url }] : [],
  });
  navigator.mediaSession.playbackState = "playing";
}

function installAudioHandlers(): void {
  audio.addEventListener("timeupdate", () => {
    state.position = audio.currentTime;
    refreshScrubber();
    reportProgress(false);
  });

  audio.addEventListener("loadedmetadata", () => {
    if (Number.isFinite(audio.duration) && audio.duration > 0) {
      state.duration = Math.round(audio.duration);
    }
    // The scrubber and the clock live on persistent nodes; updating them
    // in place avoids a whole-pane rebuild right as playback starts.
    if (pendingSeekSecs !== null) {
      // Seeking before metadata is legal is silently ignored; this is the
      // first instant the restored playhead can be placed.
      audio.currentTime = pendingSeekSecs;
      state.position = pendingSeekSecs;
      pendingSeekSecs = null;
    }
    refreshScrubber();
  });

  audio.addEventListener("play", () => {
    state.playing = true;
    syncTransport();
    renderMiddle();
  });

  audio.addEventListener("pause", () => {
    state.playing = false;
    reportProgress(true);
    syncTransport();
    renderMiddle();
  });

  audio.addEventListener("ended", () => {
    state.playing = false;
    state.position = state.duration;
    reportProgress(true, true);
    syncTransport();
    refreshScrubber();
    renderMiddle();
    // Continue listening the way Apple Podcasts does: the next episode of the
    // list in play follows the one that ended.
    const next = state.current ? nextEpisodeAfter(state.current.id) : null;
    if (next && next.enclosure_url) {
      void playEpisode(next).then(() => {
        // Autoplay after a user-initiated stream that just ended is permitted;
        // a refusal falls back to a ready state rather than an error banner.
        if (!state.playing) showStatus("Paused: couldn't play the next episode");
      });
    } else {
      renderNowPlaying();
    }
  });

  audio.addEventListener("error", () => {
    if (state.current) showStatus("Couldn't play this episode");
    state.playing = false;
    syncTransport();
  });
}

/* -------------------------------------------------------------- rendering */

function renderLibrary(): void {
  // Only a plugin the user wants running but which is not counts as a failure;
  // switching one off is not a problem to report.
  const failed = state.statuses.filter((status) => !status.ok && status.enabled);

  const primary: Node[] = [
    navItem("discovery", GLYPH.discovery, "Discovery", "", () =>
      setRoute({ kind: "discovery" }),
    ),
    navItem("latest", GLYPH.latest, "Latest episodes", String(state.library.latest.length), () =>
      setRoute({ kind: "latest" }),
    ),
  ];

  // Only panels that asked for the library's navigation appear there; docked
  // and popout panels are placed elsewhere.
  for (const panel of state.panels.filter((entry) => entry.placement === "sidebar")) {
    primary.push(
      navItem(
        `panel:${panel.plugin_id}:${panel.plugin_index}:${panel.panel_id}`,
        GLYPH.panel,
        panel.title,
        "",
        () => setRoute({ kind: "panel", pluginIndex: panel.plugin_index, panelId: panel.panel_id }),
      ),
    );
  }

  // While searching, the subscription list doubles as a jump list of matches.
  const searching = state.query !== "";
  const subscriptions = (searching ? state.search.shows : state.library.shows).map((show) =>
    navItem(
      `show:${show.id}`,
      null,
      show.title,
      String(show.episode_count),
      () => setRoute({ kind: "show", showId: show.id }),
      artwork(show.image_url, "nav__thumb"),
      // Unsubscribing is meaningless for a search hit that is not subscribed.
      searching
        ? undefined
        : {
            label: `Unsubscribe from ${show.title}`,
            onActivate: () => void unsubscribe(show.id, show.title),
          },
    ),
  );

  const scroll: Node[] = [
    ...(failed.length > 0
      ? [
          el("div", { class: "notice" }, [
            `${failed.length} plugin${failed.length === 1 ? "" : "s"} couldn't start — ${failed[0].detail}`,
          ]),
        ]
      : []),
    el("div", { class: "nav" }, primary),
    el("div", { class: "hairline" }),
  ];

  if (state.query !== "") {
    if (state.searchPending) {
      scroll.push(el("div", { class: "searchless" }, ["Searching…"]));
    } else if (subscriptions.length > 0 || state.search.episodes.length > 0) {
      scroll.push(
        el("div", { class: "section-label", text: "Matching shows" }),
        subscriptions.length > 0
          ? el("div", { class: "nav" }, subscriptions)
          : el("p", { class: "searchless", text: "No shows match." }),
      );
    } else {
      scroll.push(
        el("div", { class: "searchless" }, ["Nothing in your library matches."]),
      );
    }
    scroll.push(el("div", { class: "hairline" }));
  } else if (subscriptions.length > 0) {
    scroll.push(
      el("div", { class: "section-label", text: "Subscriptions" }),
      el("div", { class: "nav" }, subscriptions),
    );
  } else {
    scroll.push(
      el("div", { class: "empty" }, [
        el("p", { class: "empty__body", text: "Nothing subscribed yet. Add a feed to begin." }),
      ]),
    );
  }

  // Settings is pinned in the footer with "Add a feed", so it is always in reach
  // however long the subscription list grows — and its highlight lines up with
  // the nav rows above.
  swap(dom.libraryFooterNav, () => [
    navItem("settings", GLYPH.settings, "Settings", "", () => setRoute({ kind: "settings" })),
  ]);

  // The search field and the footer live outside this scroll region, so a
  // rebuild never touches a focused input.
  swap(dom.libraryScroll, () => scroll);
}

/**
 * A navigation row.
 *
 * The highlight lives on the wrapper rather than the button, so a row can carry
 * a second control — the unsubscribe action on a subscription — without nesting
 * one button inside another. The optional `action` crossfades with `meta` in
 * the same slot, so the row's geometry never moves on hover.
 */
function navItem(
  key: string,
  glyph: string | null,
  label: string,
  meta: string,
  onActivate: () => void,
  art?: HTMLElement,
  action?: { label: string; onActivate: () => void },
): HTMLElement {
  const current = routeKey() === key;

  const main = el(
    "button",
    {
      class: "nav__item",
      attrs: { type: "button", "aria-current": current ? "true" : "false" },
      on: { click: onActivate },
    },
    [
      art ?? el("span", { class: "nav__glyph" }, [glyph ? svg(glyph, 15) : el("span")]),
      el("span", { class: "nav__label", text: label }),
      meta ? el("span", { class: "nav__meta", text: meta }) : null,
    ],
  );

  return el("div", { class: "nav__row", attrs: { "data-current": String(current) } }, [
    main,
    action
      ? el("button", {
          class: "nav__action",
          attrs: { type: "button", "aria-label": action.label, title: action.label },
          text: "\u00d7",
          on: { click: action.onActivate },
        })
      : null,
  ]);
}

function routeKey(): string {
  const route = state.route;
  switch (route.kind) {
    case "discovery":
      return "discovery";
    case "latest":
      return "latest";
    case "show":
      return `show:${route.showId}`;
    case "panel":
      return `panel:${findPanel(route.pluginIndex, route.panelId)?.plugin_id}:${route.pluginIndex}:${route.panelId}`;
    case "settings":
      return "settings";
    case "search":
      return "search";
  }
}

/// Panels are addressed by the plugin that owns them, not by their position in
/// the panels array — a plugin may contribute several, and several plugins may
/// contribute one each.
function findPanel(pluginIndex: number, panelId: string): PanelView | undefined {
  return state.panels.find(
    (panel) => panel.plugin_index === pluginIndex && panel.panel_id === panelId,
  );
}

/** Drop a subscription, keeping its episodes in the library. */
async function unsubscribe(showId: string, title: string): Promise<void> {
  try {
    state.library = await api.unsubscribe(showId);
    if (state.route.kind === "show" && state.route.showId === showId) {
      state.episodes = [];
      state.route = { kind: "latest" };
    }
    render();
    showStatus(`Unsubscribed from ${title}`);
  } catch (error) {
    showStatus(String(error));
  }
}

/**
 * The add-feed dialog. It is a native `<dialog>`, so the browser gives us the
 * focus trap, the backdrop and Escape-to-dismiss for free; all this wires the
 * form's own lifecycle.
 */
function installAddFeedDialog(): void {
  const { addFeedDialog: dialog, addFeedInput: input, addFeedError: error } = dom;

  dom.addFeedOpen.addEventListener("click", () => {
    error.textContent = "";
    input.value = "";
    dialog.showModal();
    input.focus();
  });

  dom.addFeedCancel.addEventListener("click", () => dialog.close());

  // Clicking the backdrop closes: the dialog element itself is the whole
  // scrollable box, so a click landing on it is a click outside the form.
  dialog.addEventListener("click", (event) => {
    if (event.target === dialog) dialog.close();
  });

  dom.addFeedForm.addEventListener("submit", async (event) => {
    event.preventDefault();
    const url = input.value.trim();
    if (!url) {
      error.textContent = "Enter a feed address.";
      input.focus();
      return;
    }

    const submit = dom.addFeedSubmit;
    submit.disabled = true;
    submit.textContent = "Adding";
    error.textContent = "";

    try {
      const show = await api.subscribe(url);
      dialog.close();
      await reload();
      state.episodeCache.delete(show.id);
      setRoute({ kind: "show", showId: show.id });
      showStatus(`Added ${show.title}`);
    } catch (failure) {
      // Show the reason in place, so the address can be corrected without
      // losing the dialog.
      error.textContent = String(failure);
      input.focus();
      input.select();
    } finally {
      submit.disabled = false;
      submit.textContent = "Add";
    }
  });
}

function renderMiddle(): void {
  swap(dom.middle, () => [el("div", { class: "pane__scroll" }, [buildMiddle()])]);
}

function buildMiddle(): HTMLElement {
  const route = state.route;

  switch (route.kind) {
    case "discovery":
      return discoveryView();
    case "search":
      return searchResults();
    case "settings":
      return settingsView();
    case "latest":
      return el("div", {}, [
        header("Latest episodes", String(state.library.latest.length), true),
        episodeRows(state.library.latest),
      ]);
    case "panel":
      return panelView(route.pluginIndex, route.panelId);
    case "show": {
      const show = state.library.shows.find((item) => item.id === route.showId);
      const episodes = state.episodes;
      return el("div", {}, [
        header(show?.title ?? "Show", String(episodes.length), true, show?.author ?? ""),
        show ? showActions(show.id) : el("span"),
        episodeRows(episodes),
      ]);
    }
  }
}

function header(title: string, count: string, withCount: boolean, subtitle = ""): HTMLElement {
  const nodes: Node[] = [
    el("h2", { class: "pane__title", text: title }),
  ];
  if (withCount && count) nodes.push(el("span", { class: "pane__count", text: count }));
  const wrap = el("div", { class: "pane__header" }, nodes);
  if (!subtitle) return wrap;
  return el("div", {}, [
    wrap,
    el("p", { class: "pane__subtitle", text: subtitle }),
  ]);
}

function showActions(showId: string): HTMLElement {
  return el("div", { class: "pane__subtitle" }, [
    el("button", {
      class: "link-btn",
      attrs: { type: "button" },
      text: "Unsubscribe",
      on: {
        click: async () => {
          state.library = await api.unsubscribe(showId);
          state.episodes = [];
          setRoute({ kind: "latest" });
        },
      },
    }),
    el("button", {
      class: "link-btn",
      attrs: { type: "button" },
      text: "Remove and forget",
      on: {
        click: async () => {
          state.library = await api.removeShow(showId);
          state.episodeCache.delete(showId);
          state.episodes = [];
          setRoute({ kind: "latest" });
        },
      },
    }),
  ]);
}

function episodeRows(episodes: EpisodeView[]): HTMLElement {
  if (episodes.length === 0) {
    return el("div", { class: "empty" }, [
      el("p", { class: "empty__body", text: "Nothing here yet." }),
    ]);
  }

  const rows = episodes.map((episode) => {
    const current = state.current?.id === episode.id;
    const progress = episode.position_secs;
    const showProgress = !episode.completed && progress > 20 && (episode.duration_secs ?? 0) > 0;
    const ratio = showProgress ? Math.min(1, progress / (episode.duration_secs ?? 1)) : 0;

    const meta: Node[] = [];
    if (episode.published) meta.push(el("span", { text: fmtDate(episode.published) }));
    const duration = fmtDuration(episode.duration_secs);
    if (duration) meta.push(el("span", { text: duration }));
    if (episode.completed) meta.push(el("span", { text: "Played" }));

    const row = el(
      "button",
      {
        class: `row${episode.completed ? " row--played" : ""}`,
        attrs: {
          type: "button",
          "aria-current": current ? "true" : "false",
        },
        on: { click: () => void playEpisode(episode) },
      },
      [
        artwork(episode.image_url, "row__art"),
        el("div", { class: "row__body" }, [
          el("p", { class: "row__title", text: episode.title }),
          meta.length > 0 ? el("div", { class: "row__meta" }, meta) : null,
        ]),
        el("span", {
          class: "row__trailing",
          text: current && state.playing ? "Playing" : "",
        }),
      ],
    );

    if (showProgress) {
      row.append(
        el("div", { class: "row__progress", attrs: { "aria-hidden": "true" } }, [
          el("span", { attrs: { style: `width:${(ratio * 100).toFixed(1)}%` } }),
        ]),
      );
    }

    return row;
  });

  return el("div", { class: "rows" }, rows);
}

/// Search results: matching shows and episodes first, then Apple Podcasts so a
/// query can surface shows the library does not have yet.
function searchResults(): HTMLElement {
  const { shows, episodes, web } = state.search;
  const total = shows.length + episodes.length + web.length;

  if (total === 0) {
    return el("div", {}, [
      header("Search", "", false, state.query),
      el("div", { class: "empty" }, [
        el("p", { class: "empty__body", text: "Nothing matches that in your library or on Apple Podcasts." }),
      ]),
    ]);
  }

  return el("div", {}, [
    header(`Results for “${state.query}”`, String(total), total > 0),
    shows.length > 0 ? el("div", { class: "section-label", text: "Your shows" }) : null,
    shows.length > 0 ? el("div", { class: "rows" }, shows.map(showRow)) : null,
    web.length > 0 ? el("div", { class: "section-label", text: "On Apple Podcasts" }) : null,
    web.length > 0 ? el("div", { class: "rows" }, web.map(discoveryRow)) : null,
    episodes.length > 0 ? el("div", { class: "section-label", text: "Episodes" }) : null,
    episodeRows(episodes),
  ].filter((node): node is HTMLElement => node !== null));
}

/// A subscription as a row, so search results read like the rest of the app.
function showRow(show: LibraryView["shows"][number]): HTMLElement {
  const meta: Node[] = [];
  if (show.author) meta.push(el("span", { text: show.author }));
  if (show.categories.length > 0) {
    meta.push(el("span", { text: show.categories.slice(0, 3).join(", ") }));
  }

  return el(
    "button",
    {
      class: "row",
      attrs: { type: "button" },
      on: { click: () => void setRoute({ kind: "show", showId: show.id }) },
    },
    [
      artwork(show.image_url, "row__art"),
      el("div", { class: "row__body" }, [
        el("p", { class: "row__title", text: show.title }),
        meta.length > 0 ? el("div", { class: "row__meta" }, meta) : null,
      ]),
      el("span", { class: "row__trailing", text: `${show.episode_count}` }),
    ],
  );
}

function discoveryView(): HTMLElement {
  const body = state.discoveryLoading && state.discovery.length === 0
    ? el("div", { class: "empty" }, [el("p", { class: "empty__body", text: "Looking for shows…" })])
    : state.discovery.length === 0
      ? el("div", { class: "empty" }, [
          el("p", { class: "empty__title", text: "No suggestions yet" }),
          el("p", {
            class: "empty__body",
            text: "Discovery is fed by plugins. A plugin with the discovery-source capability supplies candidates here, and your own listening ranks them.",
          }),
        ])
      : el("div", { class: "rows" }, state.discovery.map(discoveryRow));

  return el("div", {}, [
    header("Discovery", String(state.discovery.length), state.discovery.length > 0),
    body,
  ]);
}

function weightSlider(key: keyof Weights, label: string): HTMLElement {
  const value = state.weights[key];
  const output = el("span", { class: "slider__value", text: value.toFixed(2) });

  const input = el("input", {
    attrs: {
      type: "range",
      min: "0",
      max: "1",
      step: "0.05",
      "aria-label": label,
    },
    on: {
      input: (event) => {
        const next = Number((event.target as HTMLInputElement).value);
        state.weights[key] = next;
        output.textContent = next.toFixed(2);
        schedulePreferences();
      },
    },
  }) as HTMLInputElement;
  input.value = String(value);

  return el("div", { class: "slider" }, [
    el("span", { class: "slider__label", text: label }),
    input,
    output,
  ]);
}

/// The settings pane: the ranking model, then the plugin manager.
function settingsView(): HTMLElement {
  return el("div", {}, [
    header("Settings", "", false),
    el("div", { class: "settings" }, [
      rankingSection(),
      pluginsSection(),
    ]),
  ]);
}

function rankingSection(): HTMLElement {
  const sliders = el(
    "div",
    {},
    WEIGHT_LABELS.map(([key, label]) => weightSlider(key, label)),
  );

  const checkbox = el("input", {
    attrs: { type: "checkbox" },
    on: {
      change: (event) => {
        state.avoidExplicit = (event.target as HTMLInputElement).checked;
        schedulePreferences();
      },
    },
  }) as HTMLInputElement;
  checkbox.checked = state.avoidExplicit;

  return el("section", { class: "settings__section" }, [
    el("div", { class: "settings__head" }, [
      el("h3", { class: "settings__title", text: "How Discovery ranks" }),
      el("button", {
        class: "link-btn",
        attrs: { type: "button" },
        text: "Reset",
        on: {
          click: async () => {
            state.weights = { ...DEFAULT_WEIGHTS };
            state.avoidExplicit = false;
            await api.setWeights(state.weights);
            await api.setAvoidExplicit(false);
            state.discovery = [];
            renderMiddle();
          },
        },
      }),
    ]),
    el("p", {
      class: "widget__text",
      text: "A straight sum of these factors, weighted as you like. Every suggestion shows which parts moved it, so a slider always changes something you can see.",
    }),
    sliders,
    el("div", { class: "settings__check" }, [
      el("label", { class: "check" }, [checkbox, el("span", { text: "Avoid explicit episodes" })]),
    ]),
  ]);
}

function pluginsSection(): HTMLElement {
  const heading = el("div", { class: "settings__head" }, [
    el("h3", { class: "settings__title", text: "Plugins" }),
  ]);

  const folder = el("div", { class: "plugin-folder" }, [
    el("p", {
      class: "plugin-path",
      text: state.pluginsDir,
      title: "Drop a plugin directory here to install it",
    }),
    el("button", {
      class: "link-btn",
      attrs: { type: "button" },
      text: "Open folder",
      on: {
        click: async () => {
          try {
            await api.openPluginsFolder();
          } catch (error) {
            showStatus(String(error));
          }
        },
      },
    }),
  ]);

  if (state.statuses.length === 0) {
    return el("section", { class: "settings__section" }, [
      heading,
      el("p", {
        class: "widget__text",
        text: "No plugins are installed. Drop a plugin directory into the folder below and it appears here and in the library.",
      }),
      folder,
    ]);
  }

  return el("section", { class: "settings__section" }, [
    heading,
    el(
      "div",
      { class: "plugin-list" },
      state.statuses.map((status) => pluginRow(status)),
    ),
    panelsList(),
    folder,
  ]);
}

/**
 * Where each contributed panel lives. Popout panels get a button to open their
 * window; the others are already placed, so they just say where.
 */
function panelsList(): HTMLElement | null {
  if (state.panels.length === 0) return null;

  const where: Record<string, string> = {
    sidebar: "In the library",
    now_playing: "Under the speed control",
    popout: "Own window",
  };

  return el("div", { class: "panel-list" }, [
    el("div", { class: "section-label", text: "Panels" }),
    ...state.panels.map((panel) =>
      el("div", { class: "panel-row" }, [
        el("div", { class: "panel-row__body" }, [
          el("div", { class: "plugin-row__name" }, [
            panel.title,
            el("span", { class: "plugin-row__version", text: panel.plugin_name }),
          ]),
          el("div", { class: "plugin-row__detail", text: where[panel.placement] ?? "" }),
        ]),
        panel.placement === "popout"
          ? el("button", {
              class: "link-btn",
              attrs: { type: "button" },
              text: "Open",
              on: { click: () => void openPanelOverlay(panel) },
            })
          : null,
      ]),
    ),
  ]);
}

function pluginRow(status: PluginStatusView): HTMLElement {
  const dot = status.ok
    ? ""
    : status.enabled
      ? " status-dot--off"
      : " status-dot--disabled";

  const actions: Node[] = [];

  if (typeof status.index === "number") {
    actions.push(
      el("button", {
        class: "plugin-row__icon",
        attrs: { type: "button", "aria-label": `Reload ${status.name}`, title: "Reload" },
        on: { click: () => void reloadPlugin(status) },
      }, [svg(GLYPH.reload, 14)]),
    );
  }

  actions.push(
    el("button", {
      class: "link-btn plugin-row__toggle",
      attrs: { type: "button" },
      text: status.enabled ? "Disable" : "Enable",
      on: { click: () => void setPluginEnabled(status, !status.enabled) },
    }),
  );

  return el("div", { class: "plugin-row" }, [
    el("span", {
      class: `status-dot${dot}`,
      title: status.ok ? "Running" : status.detail,
    }),
    el("div", { class: "plugin-row__body" }, [
      el(
        "div",
        { class: "plugin-row__name" },
        [
          status.name,
          status.version
            ? el("span", { class: "plugin-row__version", text: status.version })
            : null,
        ].filter((node): node is HTMLElement => node !== null),
      ),
      status.ok ? null : el("div", { class: "plugin-row__detail", text: status.detail }),
    ]),
    el("div", { class: "plugin-row__actions" }, actions),
  ]);
}

/// Re-read panels and status together: enabling or disabling changes both, and
/// panel indices shift when a plugin leaves the host's list.
async function refreshPlugins(): Promise<void> {
  const [panels, statuses] = await Promise.all([api.pluginPanels(), api.pluginStatus()]);
  state.panels = panels;
  state.statuses = statuses;
  state.panelContent = null;
  // Drop cached content for panels that no longer exist, then refill.
  const live = new Set(panels.map((panel) => panelKey(panel.plugin_index, panel.panel_id)));
  for (const key of [...state.panelContents.keys()]) {
    if (!live.has(key)) state.panelContents.delete(key);
  }

  // The panel that was open may belong to the plugin that just changed.
  if (state.route.kind === "panel") {
    const stillThere = panels.some(
      (panel) =>
        panel.plugin_index === (state.route as { pluginIndex: number }).pluginIndex &&
        panel.panel_id === (state.route as { panelId: string }).panelId,
    );
    if (!stillThere) state.route = { kind: "latest" };
  }
  // A disabled discovery source leaves a stale queue behind.
  state.discovery = [];

  await loadAllPanels();
  await refreshAudio();
}

async function reloadPlugin(status: PluginStatusView): Promise<void> {
  if (typeof status.index !== "number") return;
  try {
    showStatus(`Reloading ${status.name}…`);
    await api.pluginReload(status.index);
    await refreshPlugins();
    showStatus(`${status.name} reloaded`);
  } catch (error) {
    showStatus(String(error));
  }
  render();
}

async function setPluginEnabled(status: PluginStatusView, enabled: boolean): Promise<void> {
  try {
    showStatus(`${enabled ? "Enabling" : "Disabling"} ${status.name}…`);
    await api.pluginSetEnabled(status.id, enabled);
    await refreshPlugins();
    showStatus(`${status.name} ${enabled ? "enabled" : "disabled"}`);
  } catch (error) {
    showStatus(String(error));
  }
  render();
}

function discoveryRow(item: DiscoveryItemView): HTMLElement {
  const reasons = el(
    "div",
    { class: "reasons" },
    item.reasons.map((reason) => {
      const magnitude = Math.min(1, Math.abs(reason.contribution));
      return el("span", { class: "reason" }, [
        el("span", { class: "reason__bar" }, [
          el("span", { attrs: { style: `width:${(magnitude * 100).toFixed(0)}%` } }),
        ]),
        reason.label,
      ]);
    }),
  );

  const body = el("div", { class: "row__body" }, [
    el("p", { class: "row__title", text: item.title }),
    el("div", { class: "row__meta" }, [
      item.author ? el("span", { text: item.author }) : null,
      item.categories.length > 0 ? el("span", { text: item.categories.join(", ") }) : null,
      el("span", { class: "discovery__source", text: item.source }),
    ].filter((node): node is HTMLElement => node !== null)),
    reasons,
  ]);

  const action = item.subscribed
    ? el("span", { class: "row__trailing", text: "Subscribed" })
    : button(
        "button",
        "link-btn",
        `Subscribe to ${item.title}`,
        async () => {
          try {
            await api.subscribe(item.feed_url);
            await reload();
            await loadDiscovery();
            showStatus(`Added ${item.title}`);
          } catch (error) {
            showStatus(String(error));
          }
        },
        [document.createTextNode("Subscribe")],
      );

  return el("div", { class: "row row--rich" }, [
    artwork(item.image_url, "row__art"),
    body,
    action,
  ]);
}

function panelView(pluginIndex: number, panelId: string): HTMLElement {
  const panel = findPanel(pluginIndex, panelId);
  const content = state.panelContent;

  if (!content || content.panel_id !== panelId) {
    return el("div", {}, [
      header(panel?.title ?? "Panel", "", false, panel?.plugin_name ?? ""),
      el("div", { class: "empty" }, [el("p", { class: "empty__body", text: "Loading…" })]),
    ]);
  }

  return el("div", {}, [
    header(panel?.title ?? "Panel", "", false, panel?.plugin_name ?? ""),
    panelWidgets(content, pluginIndex, panelId),
  ]);
}

/** Render a panel's widgets with a host that relays control movements back. */
function panelWidgets(
  content: PanelContent,
  pluginIndex: number,
  panelId: string,
): HTMLElement {
  const host = panelHost(pluginIndex, panelId);
  const root = el(
    "div",
    { class: "widgets" },
    content.widgets
      .map((widget) => renderPluginWidget(widget, host))
      .filter((node): node is HTMLElement => node !== null),
  );
  root.dataset.panelKey = panelKey(pluginIndex, panelId);
  return root;
}

/**
 * Forward a control movement to the plugin, then pick up what changed.
 *
 * The notification is fire-and-forget so a drag never stutters; the panel
 * re-render and the audio graph refresh are throttled, and the last one always
 * lands so the final value is never dropped.
 */
function changeWidget(
  pluginIndex: number,
  panelId: string,
  widgetId: string,
  value: unknown,
): void {
  void api.pluginPanelChange(pluginIndex, panelId, widgetId, value).catch(() => {});

  pendingChanges.set(`${pluginIndex}:${panelId}`, { pluginIndex, panelId });
  window.clearTimeout(changeTimer);
  changeTimer = window.setTimeout(flushPanelChanges, 90);
}

let changeTimer: number | undefined;
const pendingChanges = new Map<string, { pluginIndex: number; panelId: string }>();

async function flushPanelChanges(): Promise<void> {
  const panels = [...pendingChanges.values()];
  pendingChanges.clear();
  if (panels.length === 0) return;

  await Promise.all(
    panels.map(async ({ pluginIndex, panelId }) => {
      try {
        const content = await api.pluginPanelContent(pluginIndex, panelId);
        state.panelContents.set(panelKey(pluginIndex, panelId), content);
        if (state.route.kind === "panel" && state.route.panelId === panelId) {
          state.panelContent = content;
        }
        // In-place widget refresh: the knob the user turned stays mounted with
        // its drag intact; only its display is pulled to the new value.
        refreshPanelDisplays(pluginIndex, panelId, content);
      } catch {
        /* the plugin may have stopped mid-drag */
      }
    }),
  );

  await refreshAudio();
}

/**
 * Every mounted copy of a panel gets fresh widget values without a rebuild —
 * the docked player control, an open popout, the full pane, wherever it is.
 */
function refreshPanelDisplays(pluginIndex: number, panelId: string, content: PanelContent): void {
  const key = panelKey(pluginIndex, panelId);
  const roots = [
    ...dom.now.querySelectorAll<HTMLElement>(".widgets[data-panel-key]"),
    ...dom.middle.querySelectorAll<HTMLElement>(".widgets[data-panel-key]"),
  ].filter((node) => node.dataset.panelKey === key);
  const dialog = dom.panelDialog.open
    ? dom.panelDialogBody.querySelector<HTMLElement>(".widgets[data-panel-key]")
    : null;
  if (dialog && dialog.dataset.panelKey === key) roots.push(dialog);

  for (const root of roots) {
    if (patchWidgets(root, content, panelHost(pluginIndex, panelId)) === "patched") continue;
    // A rebuild is the only honest answer when the widget structure changed;
    // `swap` keeps it from flashing and its interaction guard keeps drags alive.
    swap(root, () => [panelWidgets(content, pluginIndex, panelId)]);
  }
}

function panelHost(pluginIndex: number, panelId: string): WidgetHost {
  return {
    onChange: (widgetId, value) => void changeWidget(pluginIndex, panelId, widgetId, value),
  };
}

function panelKey(pluginIndex: number, panelId: string): string {
  return `${pluginIndex}:${panelId}`;
}

/**
 * Rebuild the playback chain from what the plugins currently want. Called on
 * start-up, whenever a plugin is loaded or unloaded, and after any control
 * movement that could change a parameter. The pipeline itself decides whether
 * the element gets routed: only an enabled unit opens that one-way door.
 */
async function refreshAudio(): Promise<void> {
  try {
    engine.refresh(audio, await api.audioGraph());
  } catch (error) {
    console.error("[poddies] audio graph refresh failed:", error);
  }
}

/**
 * Panels a plugin asked to dock in the now-playing pane — controls you want at
 * hand while listening, like the compressor's one knob.
 */
function dockedPanels(): HTMLElement | null {
  const docked = state.panels.filter((panel) => panel.placement === "now_playing");
  if (docked.length === 0) return null;

  const blocks = docked.map((panel) => {
    const content = state.panelContents.get(panelKey(panel.plugin_index, panel.panel_id));
    return el("section", { class: "dock" }, [
      el("div", { class: "dock__head" }, [
        el("span", { class: "dock__title", text: panel.title }),
        panel.plugin_name ? el("span", { class: "dock__source", text: panel.plugin_name }) : null,
      ]),
      content
        ? panelWidgets(content, panel.plugin_index, panel.panel_id)
        : el("p", { class: "widget__text", text: "Loading…" }),
    ]);
  });

  return el("div", { class: "docks" }, blocks);
}

interface NowRefs {
  scrub: HTMLInputElement;
  elapsed: HTMLElement;
  total: HTMLElement;
  play: HTMLButtonElement;
  rates: HTMLButtonElement[];
}

const RATES = [0.8, 1, 1.2, 1.5, 2];
let nowRefs: NowRefs | null = null;

let scrubbing = false;
let scrubIntent: number | null = null;
let scrubTimer: number | undefined;

/** Apply the user's latest scrub intent to the real playhead at a restrained
 * cadence — one seek per 150 ms while dragging (quick scrubbing), and the
 * exact value on release. */
function commitScrubIntent(): void {
  window.clearTimeout(scrubTimer);
  scrubTimer = window.setTimeout(() => {
    if (!scrubbing || scrubIntent === null) return;
    const target = scrubIntent;
    audio.currentTime = target;
    state.position = target;
  }, 150);
}

function endScrub(): void {
  scrubbing = false;
  scrubIntent = null;
}

function renderNowPlaying(): void {
  const episode = state.current;

  if (!episode) {
    swap(dom.now, () => [
      el("div", { class: "pane__scroll" }, [
        el("div", { class: "now" }, [
          el("div", { class: "now__art now__art--empty", attrs: { "aria-hidden": "true" } }, [
            svg(GLYPH.play, 22),
          ]),
          el("div", { class: "now__meta" }, [
            el("p", { class: "now__episode", text: "Choose an episode to begin." }),
          ]),
          // Controls a plugin docked here stay available with nothing playing.
          dockedPanels(),
        ].filter((node): node is HTMLElement => node !== null)),
      ]),
    ]);
    nowRefs = null;
    return;
  }

  const known = state.duration || episode.duration_secs || 0;
  const scrub = el("input", {
    attrs: {
      type: "range",
      min: "0",
      max: String(Math.max(1, known)),
      step: "1",
      "aria-label": "Seek",
    },
    on: {
      // A drag is intent, not seeks: the clock follows the thumb, the real
      // seek is throttled behind the playhead instead of per pixel, and the
      // final value is committed at release (`change`).
      input: (event) => {
        const value = Number((event.target as HTMLInputElement).value);
        scrubbing = true;
        scrubIntent = value;
        state.position = value;
        if (nowRefs) nowRefs.elapsed.textContent = fmtClock(value);
        commitScrubIntent();
      },
      change: (event) => {
        const value = Number((event.target as HTMLInputElement).value);
        endScrub();
        audio.currentTime = value;
        state.position = value;
        if (nowRefs) nowRefs.elapsed.textContent = fmtClock(value);
        reportProgress(true);
      },
    },
  }) as HTMLInputElement;
  scrub.value = String(Math.min(state.position, known || 1));

  const elapsed = el("span", { text: fmtClock(state.position) });
  const total = el("span", { text: fmtClock(known) });

  const playButton = button(
    "button",
    "transport-btn transport-btn--primary",
    state.playing ? "Pause" : "Play",
    () => togglePlay(),
    [svg(state.playing ? GLYPH.pause : GLYPH.play, 18)],
  );

  const rates = RATES.map((rate) =>
    el("button", {
      class: "rate-btn",
      attrs: {
        type: "button",
        "aria-pressed": state.rate === rate ? "true" : "false",
      },
      text: rate === 1 ? "1×" : `${rate}×`,
      on: {
        click: () => {
          state.rate = rate;
          audio.playbackRate = rate;
          reportProgress(true);
          syncTransport();
        },
      },
    }),
  ) as HTMLButtonElement[];

  nowRefs = { scrub, elapsed, total, play: playButton, rates };

  const canPlay = Boolean(episode.enclosure_url);

  swap(dom.now, () => [
    el("div", { class: "pane__scroll" }, [
      el("div", { class: "now" }, [
        artwork(episode.image_url, "now__art artwork-fallback--lg"),
        el("div", { class: "now__meta" }, [
          el("p", { class: "now__show", text: episode.show_title }),
          el("p", { class: "now__episode", text: episode.title }),
        ]),
        el("div", { class: "now__scrub" }, [scrub]),
        el("div", { class: "now__times" }, [elapsed, total]),
        el("div", { class: "now__transport" }, [
          button("button", "transport-btn", "Back 15 seconds", () => seekBy(-15), [
            svg(GLYPH.back15, 17),
          ]),
          playButton,
          button("button", "transport-btn", "Forward 15 seconds", () => seekBy(15), [
            svg(GLYPH.fwd15, 17),
          ]),
        ]),
        el("div", { class: "now__rate" }, rates),
        dockedPanels(),
        !canPlay
          ? el("p", { class: "widget__text", text: "This episode has no audio attached." })
          : null,
      ].filter((node): node is HTMLElement => node !== null)),
    ]),
  ]);
}

/**
 * Keep the transport state current without rebuilding the pane: play/pause and
 * rate changes flip glyphs and highlighted buttons in place, so nothing under
 * the user's pointer resets while audio changes state. Seek state is handled
 * separately by `refreshScrubber`.
 */
function syncTransport(): void {
  if (!nowRefs) return;
  const label = state.playing ? "Pause" : "Play";
  nowRefs.play.setAttribute("aria-label", label);
  nowRefs.play.title = label;
  nowRefs.play.replaceChildren(svg(state.playing ? GLYPH.pause : GLYPH.play, 18));
  nowRefs.rates.forEach((node, index) => {
    node.setAttribute("aria-pressed", String(RATES[index] === state.rate));
  });
}

function refreshScrubber(): void {
  if (!nowRefs) return;
  // The user's thumb is in charge while they scrub the clock happens follows
  // intent, and a fresh render takes over again here the drag ends.
  if (scrubbing) return;
  const known = state.duration || 0;
  if (known > 0) {
    nowRefs.scrub.max = String(known);
    nowRefs.total.textContent = fmtClock(known);
  }
  nowRefs.scrub.value = String(Math.min(audio.currentTime, Number(nowRefs.scrub.max)));
  nowRefs.elapsed.textContent = fmtClock(audio.currentTime);
}

function render(): void {
  renderLibrary();
  renderMiddle();
  renderNowPlaying();
}

/* ------------------------------------------------------------------ loads */

async function reload(): Promise<void> {
  state.library = await api.snapshot();
  if (state.route.kind === "show") {
    await loadEpisodes(state.route.showId);
  }
}

async function loadEpisodes(showId: string): Promise<void> {
  const cached = state.episodeCache.get(showId);
  if (cached) {
    state.episodes = cached;
    return;
  }
  state.episodes = await api.episodesForShow(showId);
  state.episodeCache.set(showId, state.episodes);
}

async function loadDiscovery(): Promise<void> {
  state.discoveryLoading = true;
  renderMiddle();
  try {
    state.discovery = await api.discovery(DISCOVERY_LIMIT);
  } catch {
    state.discovery = [];
    showStatus("Discovery source unavailable");
  } finally {
    state.discoveryLoading = false;
    renderMiddle();
  }
}

let preferenceTimer: number | undefined;

/**
 * Persist a preference, debounced so dragging a slider writes once.
 *
 * The queue is marked stale rather than re-ranked here: a re-render while the
 * user is mid-drag would drop the slider, and the weights only matter the next
 * time Discovery is opened.
 */
function schedulePreferences(): void {
  window.clearTimeout(preferenceTimer);
  preferenceTimer = window.setTimeout(async () => {
    try {
      await api.setWeights(state.weights);
      await api.setAvoidExplicit(state.avoidExplicit);
    } catch {
      showStatus("Couldn't save settings");
    }
    if (state.route.kind !== "discovery") state.discovery = [];
  }, 320);
}

async function setRoute(route: Route): Promise<void> {
  // Search is a lens over the library, not a place; leaving it clears the field.
  if (state.query !== "" && route.kind !== "search") {
    state.query = "";
    state.search = { shows: [], episodes: [], web: [] };
    dom.search.value = "";
  }

  state.route = route;

  if (route.kind === "show") {
    await loadEpisodes(route.showId);
  }

  if (route.kind === "panel") {
    await loadPanel(route.pluginIndex, route.panelId);
    state.panelContent =
      state.panelContents.get(panelKey(route.pluginIndex, route.panelId)) ?? null;
  }

  if (route.kind === "discovery" && state.discovery.length === 0) {
    void loadDiscovery();
  }

  render();
}

/* --------------------------------------------------------- panel overlay */

/** Fetch one panel's content into the cache. */
async function loadPanel(pluginIndex: number, panelId: string): Promise<void> {
  try {
    const content = await api.pluginPanelContent(pluginIndex, panelId);
    state.panelContents.set(panelKey(pluginIndex, panelId), content);
  } catch (error) {
    console.error(`[poddies] panel '${panelId}' failed:`, error);
  }
}

/** Load every panel the plugins advertise, for the dock and the overlay. */
async function loadAllPanels(): Promise<void> {
  await Promise.all(
    state.panels.map((panel) => loadPanel(panel.plugin_index, panel.panel_id)),
  );
}

/**
 * Present a plugin panel that asked for `popout` placement.
 *
 * The host presents it as a floating sheet over the app rather than a second OS
 * window: it inherits the window's Mica, typography and motion, it needs no
 * separate webview (and therefore no separate permissions or asset load), and a
 * plugin author gets the same rendering as every other surface for free.
 */
async function openPanelOverlay(panel: PanelView): Promise<void> {
  dom.panelDialogTitle.textContent = panel.title;

  const key = panelKey(panel.plugin_index, panel.panel_id);
  if (!state.panelContents.has(key)) {
    await loadPanel(panel.plugin_index, panel.panel_id);
  }

  const content = state.panelContents.get(key);
  dom.panelDialogBody.replaceChildren(
    content
      ? panelWidgets(content, panel.plugin_index, panel.panel_id)
      : el("p", { class: "widget__text", text: "That panel is not responding." }),
  );

  if (!dom.panelDialog.open) dom.panelDialog.showModal();
}

function installPanelDialog(): void {
  dom.panelDialogClose.addEventListener("click", () => dom.panelDialog.close());
  dom.panelDialog.addEventListener("click", (event) => {
    if (event.target === dom.panelDialog) dom.panelDialog.close();
  });
}

/* ------------------------------------------------------------------- boot */

async function boot(): Promise<void> {
  installDeferredFlush();
  installAudioHandlers();
  installSearchField();
  installWindowDragging();
  installAddFeedDialog();
  installPanelDialog();

  dom.minimize.addEventListener("click", () => void api.appHide());
  dom.close.addEventListener("click", () => void api.appHide());

  dom.settings.addEventListener("click", () => void setRoute({ kind: "settings" }));

  dom.refresh.addEventListener("click", async () => {
    dom.refresh.disabled = true;
    showStatus("Checking for new episodes…");
    try {
      const summary = await api.refreshAll();
      await reload();
      state.episodeCache.clear();
      const parts = [
        `${summary.checked} checked`,
        `${summary.new_episodes} new`,
      ];
      if (summary.failed.length > 0) parts.push(`${summary.failed.length} failed`);
      showStatus(parts.join(" · "));
    } catch {
      showStatus("Refresh failed");
    } finally {
      dom.refresh.disabled = false;
      render();
    }
  });

  // Space toggles playback unless a field or control has focus.
  window.addEventListener("keydown", (event) => {
    if (event.code !== "Space") return;
    const target = event.target as HTMLElement | null;
    if (target && /^(INPUT|TEXTAREA|BUTTON|SELECT)$/.test(target.tagName)) return;
    if (!state.current) return;
    event.preventDefault();
    togglePlay();
  });

  // The signature reveal: played whenever the window returns from the tray.
  await listen("poddies://reveal", () => {
    dom.app.classList.remove("is-revealing");
    // Force a reflow so the animation restarts on a repeat reveal.
    void dom.app.offsetWidth;
    dom.app.classList.add("is-revealing");
    window.setTimeout(() => dom.app.classList.remove("is-revealing"), 700);
  });

  const [library, preferences, panels, statuses, pluginsDir] = await Promise.all([
    api.snapshot(),
    api.preferences(),
    api.pluginPanels(),
    api.pluginStatus(),
    api.pluginsDirectory(),
  ]);

  state.library = library;
  state.weights = preferences.discovery;
  state.avoidExplicit = preferences.avoid_explicit;
  state.panels = panels;
  state.statuses = statuses;
  state.pluginsDir = pluginsDir;

  if (state.library.in_progress.length > 0) {
    state.current = state.library.in_progress[0];
    state.duration = state.current.duration_secs ?? 0;
    const from = resumePositionFor(state.current);
    state.position = from;
  }

  render();
  await loadAllPanels();
  await refreshAudio();
  prepareResume();
  render();

  render();
  await loadAllPanels();
  await refreshAudio();
  render();
  installMeterLoop();
  void loadDiscovery();
}

/**
 * Paint the plugin meters from the live audio graph. Values never travel
 * through a plugin, so this is a DOM write per frame while something plays.
 */
function installMeterLoop(): void {
  const tick = () => {
    const meters = document.querySelectorAll("[data-meter]");
    if (meters.length > 0) {
      const { peakDb, reductionDb } = engine.readMeters();
      paintMeters(document, peakDb, reductionDb);
    }
    window.setTimeout(tick, 80);
  };
  window.setTimeout(tick, 400);
}

void boot();

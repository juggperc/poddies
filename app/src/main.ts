import { listen } from "@tauri-apps/api/event";

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
  type Widget,
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

/** Artwork with a quiet monochrome fallback so a missing image is invisible. */
function artwork(url: string | null, className: string): HTMLElement {
  const fallback = () =>
    el("span", { class: `${className} artwork-fallback`, attrs: { "aria-hidden": "true" } });

  if (!url) return fallback();

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
  addForm: null as HTMLElement | null,
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
  state.position = episode.position_secs;
  state.duration = episode.duration_secs ?? 0;
  audio.src = episode.enclosure_url;
  audio.playbackRate = state.rate;

  if (episode.position_secs > 5 && (!episode.duration_secs || episode.position_secs < episode.duration_secs - 15)) {
    audio.currentTime = episode.position_secs;
  }

  try {
    await audio.play();
    state.playing = true;
  } catch {
    state.playing = false;
    showStatus("Couldn't play this episode");
  }

  void api.playbackStarted(episode.id);
  updateMediaSession(episode);
  renderMiddle();
  renderNowPlaying();
}

function togglePlay(): void {
  if (!state.current) return;
  if (audio.paused) {
    void audio.play().then(() => {
      state.playing = true;
      renderNowPlaying();
    });
  } else {
    audio.pause();
  }
}

function seekBy(delta: number): void {
  if (!state.current) return;
  const limit = Number.isFinite(audio.duration) ? audio.duration : state.position + delta;
  audio.currentTime = Math.max(0, Math.min(limit, audio.currentTime + delta));
  reportProgress(true);
  renderNowPlaying();
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
    renderNowPlaying();
  });

  audio.addEventListener("play", () => {
    state.playing = true;
    renderNowPlaying();
    renderMiddle();
  });

  audio.addEventListener("pause", () => {
    state.playing = false;
    reportProgress(true);
    renderNowPlaying();
    renderMiddle();
  });

  audio.addEventListener("ended", () => {
    state.playing = false;
    state.position = state.duration;
    reportProgress(true, true);
    renderNowPlaying();
    renderMiddle();
  });

  audio.addEventListener("error", () => {
    if (state.current) showStatus("Couldn't play this episode");
    state.playing = false;
    renderNowPlaying();
  });
}

/* -------------------------------------------------------------- rendering */

function renderLibrary(): void {
  const failed = state.statuses.filter((status) => !status.ok);

  const primary: Node[] = [
    navItem("discovery", GLYPH.discovery, "Discovery", "", () =>
      setRoute({ kind: "discovery" }),
    ),
    navItem("latest", GLYPH.latest, "Latest episodes", String(state.library.latest.length), () =>
      setRoute({ kind: "latest" }),
    ),
  ];

  for (const panel of state.panels) {
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
  const subscriptions = (
    state.query ? state.search.shows : state.library.shows
  ).map((show) =>
    navItem(
      `show:${show.id}`,
      null,
      show.title,
      String(show.episode_count),
      () => setRoute({ kind: "show", showId: show.id }),
      artwork(show.image_url, "nav__thumb"),
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
        el("p", { class: "empty__body", text: "Add a feed address below to start your library." }),
      ]),
    );
  }

  scroll.push(
    el("div", { class: "hairline" }),
    navItem("settings", GLYPH.settings, "Settings", "", () => setRoute({ kind: "settings" })),
  );

  // The search field and the add form live outside this scroll region, so a
  // rebuild never touches a focused input.
  dom.libraryScroll.replaceChildren(...scroll);
}

function navItem(
  key: string,
  glyph: string | null,
  label: string,
  meta: string,
  onActivate: () => void,
  art?: HTMLElement,
): HTMLElement {
  const active = routeKey() === key;
  const node = el(
    "button",
    {
      class: "nav__item",
      attrs: { type: "button", "aria-current": active ? "true" : "false" },
      on: { click: onActivate },
    },
    [
      art ??
        el("span", { class: "nav__glyph" }, [
          glyph ? svg(glyph, 15) : el("span"),
        ]),
      el("span", { class: "nav__label", text: label }),
      meta ? el("span", { class: "nav__meta", text: meta }) : null,
    ],
  );
  return node;
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

function addForm(): HTMLElement {
  const input = el("input", {
    attrs: { type: "text", placeholder: "Feed address", spellcheck: "false" },
  }) as HTMLInputElement;

  const submit = el("button", { attrs: { type: "submit" }, text: "Add" }) as HTMLButtonElement;

  const form = el(
    "form",
    {
      class: "add-form",
      on: {
        submit: async (event) => {
          event.preventDefault();
          const url = input.value.trim();
          if (!url) return;
          submit.disabled = true;
          submit.textContent = "Adding";
          try {
            const show = await api.subscribe(url);
            input.value = "";
            await reload();
            setRoute({ kind: "show", showId: show.id });
            showStatus(`Added ${show.title}`);
          } catch (error) {
            showStatus(String(error));
          } finally {
            submit.disabled = false;
            submit.textContent = "Add";
          }
        },
      },
    },
    [input, submit],
  );

  return form;
}

function renderMiddle(): void {
  dom.middle.replaceChildren(
    el("div", { class: "pane__scroll" }, [buildMiddle()]),
  );
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
  // Ranking lives in Settings so the queue itself stays clean; the link keeps
  // the connection between the two a single click away.
  const tune = el("div", { class: "discovery__tune" }, [
    el("span", {
      class: "discovery__tune-text",
      text: "Ranked from your listening, adjusted in Settings",
    }),
    el("button", {
      class: "link-btn",
      attrs: { type: "button" },
      text: "Tune",
      on: { click: () => void setRoute({ kind: "settings" }) },
    }),
  ]);

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
    tune,
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
  if (state.statuses.length === 0) {
    return el("section", { class: "settings__section" }, [
      el("div", { class: "settings__head" }, [
        el("h3", { class: "settings__title", text: "Plugins" }),
      ]),
      el("p", {
        class: "widget__text",
        text: "No plugins are installed. Drop a plugin directory into the folder below and they appear here and in the library.",
      }),
      el("p", { class: "plugin-path", text: state.pluginsDir }),
    ]);
  }

  return el("section", { class: "settings__section" }, [
    el("div", { class: "settings__head" }, [
      el("h3", { class: "settings__title", text: "Plugins" }),
    ]),
    el(
      "div",
      { class: "plugin-list" },
      state.statuses.map((status) =>
        el("div", { class: "plugin-row" }, [
          el("span", {
            class: `status-dot${status.ok ? "" : " status-dot--off"}`,
            title: status.ok ? "Running" : status.detail,
          }),
          el("div", { class: "plugin-row__body" }, [
            el("div", { class: "plugin-row__name" }, [
              status.name,
              status.version ? el("span", { class: "plugin-row__version", text: status.version }) : null,
            ].filter((node): node is HTMLElement => node !== null)),
            status.ok
              ? null
              : el("div", { class: "plugin-row__detail", text: status.detail }),
          ]),
          typeof status.index === "number"
            ? el("button", {
                class: "plugin-row__reload",
                attrs: { type: "button", "aria-label": `Reload ${status.name}` },
                on: {
                  click: async () => {
                    try {
                      showStatus(`Reloading ${status.name}…`);
                      await api.pluginReload(status.index as number);
                      const [panels, statuses] = await Promise.all([
                        api.pluginPanels(),
                        api.pluginStatus(),
                      ]);
                      state.panels = panels;
                      state.statuses = statuses;
                      state.panelContent = null;
                      showStatus(`${status.name} reloaded`);
                    } catch (error) {
                      showStatus(String(error));
                    }
                    render();
                  },
                },
              }, [svg(GLYPH.reload, 14)])
            : null,
        ]),
      ),
    ),
    el("p", { class: "plugin-path", text: state.pluginsDir, title: "Drop a plugin directory here to install it" }),
  ]);
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
    el(
      "div",
      { class: "widgets" },
      content.widgets
        .map(renderWidget)
        .filter((node): node is HTMLElement => node !== null),
    ),
  ]);
}

/**
 * Widgets are a closed set, but a plugin built against a newer minor protocol
 * can send a type this build doesn't know. Unknown types are skipped rather
 * than rendered as garbage.
 */
function renderWidget(widget: Widget): HTMLElement | null {
  switch (widget.type) {
    case "heading":
      return el("div", { class: "widget__heading", text: widget.text });
    case "text":
      return el("p", { class: "widget__text", text: widget.text });
    case "divider":
      return el("div", { class: "widget__divider" });
    case "metric":
      return el("div", {}, [
        el("div", { class: "metric__value", text: widget.value }),
        el("div", { class: "metric__label", text: widget.label }),
      ]);
    case "bar": {
      const ratio = widget.max > 0 ? Math.min(1, Math.max(0, widget.value / widget.max)) : 0;
      return el("div", { class: "bar" }, [
        el("div", { class: "bar__head" }, [
          el("span", { text: widget.label }),
          el("span", { text: `${Math.round(ratio * 100)}%` }),
        ]),
        el("div", { class: "bar__track" }, [
          el("span", { class: "bar__fill", attrs: { style: `width:${(ratio * 100).toFixed(1)}%` } }),
        ]),
      ]);
    }
    case "list":
      return el(
        "div",
        { class: "list" },
        widget.items.map((item) =>
          el("div", { class: "list__row" }, [
            el("span", { text: item.primary }),
            item.secondary ? el("span", { class: "list__secondary", text: item.secondary }) : null,
          ]),
        ),
      );
    default:
      return null;
  }
}

let nowRefs: { scrub: HTMLInputElement; elapsed: HTMLElement; total: HTMLElement } | null = null;

function renderNowPlaying(): void {
  const episode = state.current;

  if (!episode) {
    dom.now.replaceChildren(
      el("div", { class: "now" }, [
        el("div", { class: "now__art now__art--empty", attrs: { "aria-hidden": "true" } }, [
          svg(GLYPH.play, 22),
        ]),
        el("div", { class: "now__meta" }, [
          el("p", { class: "now__episode", text: "Choose an episode to begin." }),
        ]),
      ]),
    );
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
      input: (event) => {
        const value = Number((event.target as HTMLInputElement).value);
        audio.currentTime = value;
        state.position = value;
        if (nowRefs) nowRefs.elapsed.textContent = fmtClock(value);
      },
      change: () => reportProgress(true),
    },
  }) as HTMLInputElement;
  scrub.value = String(Math.min(state.position, known || 1));

  const elapsed = el("span", { text: fmtClock(state.position) });
  const total = el("span", { text: fmtClock(known) });
  nowRefs = { scrub, elapsed, total };

  const playButton = button(
    "button",
    "transport-btn transport-btn--primary",
    state.playing ? "Pause" : "Play",
    () => togglePlay(),
    [svg(state.playing ? GLYPH.pause : GLYPH.play, 18)],
  );

  const rates = [0.8, 1, 1.2, 1.5, 2].map((rate) =>
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
          renderNowPlaying();
        },
      },
    }),
  );

  const canPlay = Boolean(episode.enclosure_url);

  dom.now.replaceChildren(
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
        !canPlay
          ? el("p", { class: "widget__text", text: "This episode has no audio attached." })
          : null,
      ].filter((node): node is HTMLElement => node !== null)),
    ]),
  );
}

function refreshScrubber(): void {
  if (!nowRefs) return;
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
    state.panelContent = null;
    try {
      state.panelContent = await api.pluginPanelContent(route.pluginIndex, route.panelId);
    } catch (error) {
      showStatus(String(error));
    }
  }

  if (route.kind === "discovery" && state.discovery.length === 0) {
    void loadDiscovery();
  }

  render();
}

/* ------------------------------------------------------------------- boot */

async function boot(): Promise<void> {
  installAudioHandlers();
  installSearchField();
  installWindowDragging();

  // The add-feed form is created once and kept: its input must not reset on a
  // library re-render.
  dom.addForm = addForm();
  dom.library.appendChild(dom.addForm);

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
    state.position = state.current.position_secs;
    state.duration = state.current.duration_secs ?? 0;
  }

  render();
  void loadDiscovery();
}

void boot();

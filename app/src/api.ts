import { invoke } from "@tauri-apps/api/core";

export interface ShowView {
  id: string;
  title: string;
  author: string | null;
  description: string | null;
  image_url: string | null;
  categories: string[];
  explicit: boolean;
  episode_count: number;
}

export interface EpisodeView {
  id: string;
  show_id: string;
  show_title: string;
  title: string;
  description: string | null;
  image_url: string | null;
  enclosure_url: string;
  duration_secs: number | null;
  published: string | null;
  position_secs: number;
  completed: boolean;
  play_count: number;
}

export interface LibraryView {
  shows: ShowView[];
  latest: EpisodeView[];
  in_progress: EpisodeView[];
}

export interface ReasonView {
  label: string;
  weight: number;
  contribution: number;
}

export interface DiscoveryItemView {
  title: string;
  feed_url: string;
  image_url: string | null;
  author: string | null;
  categories: string[];
  source: string;
  explicit: boolean;
  subscribed: boolean;
  score: number;
  reasons: ReasonView[];
}

export interface PanelView {
  plugin_index: number;
  plugin_id: string;
  plugin_name: string;
  panel_id: string;
  title: string;
  placement: Placement;
}

export interface SearchView {
  shows: ShowView[];
  episodes: EpisodeView[];
  /** Apple Podcasts results: shows the library does not have yet. */
  web: DiscoveryItemView[];
}

export interface PluginStatusView {
  id: string;
  name: string;
  version: string;
  ok: boolean;
  /** The user wants this plugin on; false means they switched it off. */
  enabled: boolean;
  detail: string;
  index?: number;
}

export interface RefreshSummary {
  checked: number;
  updated: number;
  new_episodes: number;
  failed: string[];
}

export type EqBandKind = "peaking" | "low_shelf" | "high_shelf";

export interface EqBand {
  id: string;
  label: string;
  frequency: number;
  gain_db: number;
  q: number;
  kind: EqBandKind;
}

export type KnobStyle = "modern" | "vintage";
export type MeterSource = "peak" | "gain_reduction";
export type Placement = "sidebar" | "now_playing" | "popout";

export type Widget =
  | { type: "heading"; text: string }
  | { type: "metric"; label: string; value: string }
  | { type: "text"; text: string }
  | { type: "divider" }
  | { type: "bar"; label: string; value: number; max: number }
  | { type: "list"; items: { primary: string; secondary: string | null }[] }
  | {
      type: "knob";
      id: string;
      label: string;
      value: number;
      min: number;
      max: number;
      unit: string;
      style: KnobStyle;
      readout: string | null;
    }
  | {
      type: "slider";
      id: string;
      label: string;
      value: number;
      min: number;
      max: number;
      step: number;
      unit: string;
    }
  | { type: "toggle"; id: string; label: string; value: boolean }
  | {
      type: "eq";
      id: string;
      bands: EqBand[];
      min_gain_db: number;
      max_gain_db: number;
      min_frequency: number;
      max_frequency: number;
    }
  | {
      type: "meter";
      id: string;
      label: string;
      source: MeterSource;
      min_db: number;
      max_db: number;
    };

/** A DSP unit a plugin wants in the playback chain. The host builds the nodes. */
export type AudioUnit =
  | { type: "parametric_eq"; id: string; enabled?: boolean; bands: EqBand[] }
  | {
      type: "compressor";
      id: string;
      enabled?: boolean;
      threshold_db: number;
      ratio: number;
      attack_ms: number;
      release_ms: number;
      knee_db?: number;
      makeup_db?: number;
    };

export interface PanelContent {
  panel_id: string;
  widgets: Widget[];
}

export interface Weights {
  topic_affinity: number;
  completion_signal: number;
  recency: number;
  novelty: number;
  duration_fit: number;
  explicit_penalty: number;
  diversity: number;
}

export interface PreferencesView {
  avoid_explicit: boolean;
  discovery: Weights;
}

export const api = {
  snapshot: () => invoke<LibraryView>("snapshot"),
  episodesForShow: (showId: string) =>
    invoke<EpisodeView[]>("episodes_for_show", { showId }),
  subscribe: (url: string) => invoke<ShowView>("subscribe", { url }),
  unsubscribe: (showId: string) => invoke<LibraryView>("unsubscribe", { showId }),
  removeShow: (showId: string) => invoke<LibraryView>("remove_show", { showId }),
  refreshAll: () => invoke<RefreshSummary>("refresh_all"),
  discovery: (limit: number) =>
    invoke<DiscoveryItemView[]>("discovery", { limit }),
  search: (query: string) => invoke<SearchView>("search", { query }),
  preferences: () => invoke<PreferencesView>("preferences"),
  setWeights: (weights: Weights) => invoke<void>("set_weights", { weights }),
  setAvoidExplicit: (value: boolean) =>
    invoke<void>("set_avoid_explicit", { value }),
  pluginPanels: () => invoke<PanelView[]>("plugin_panels"),
  pluginPanelContent: (pluginIndex: number, panelId: string) =>
    invoke<PanelContent>("plugin_panel_content", { pluginIndex, panelId }),
  /** Relay a control movement to the plugin that owns the widget. */
  pluginPanelChange: (
    pluginIndex: number,
    panelId: string,
    widgetId: string,
    value: unknown,
  ) => invoke<void>("plugin_panel_change", { pluginIndex, panelId, widgetId, value }),
  /** The audio units every enabled plugin wants in the playback chain. */
  audioGraph: () => invoke<AudioUnit[]>("audio_graph"),
  pluginStatus: () => invoke<PluginStatusView[]>("plugin_status"),
  pluginsDirectory: () => invoke<string>("plugins_directory"),
  pluginReload: (pluginIndex: number) =>
    invoke<void>("plugin_reload", { pluginIndex }),
  pluginSetEnabled: (pluginId: string, enabled: boolean) =>
    invoke<void>("plugin_set_enabled", { pluginId, enabled }),
  openPluginsFolder: () => invoke<void>("open_plugins_folder"),
  playbackStarted: (episodeId: string) =>
    invoke<void>("playback_started", { episodeId }),
  recordProgress: (
    episodeId: string,
    positionSecs: number,
    durationSecs: number | null,
    completed: boolean,
  ) =>
    invoke<void>("record_progress", {
      episodeId,
      positionSecs,
      durationSecs,
      completed,
    }),
  appHide: () => invoke<void>("app_hide"),
  appStartDrag: () => invoke<void>("app_start_drag"),
  appToggleMaximise: () => invoke<void>("app_toggle_maximise"),
};

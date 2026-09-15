/**
 * The playback audio graph.
 *
 * Plugins do not process samples — they hand the host a list of units and the
 * parameters for them, and this module builds real Web Audio nodes. That keeps
 * DSP on the audio thread at zero added latency: nothing crosses the plugin
 * process boundary per sample, so a plugin written in Python is just as viable
 * as one written in Rust.
 *
 * The pristine path matters. Routing an `<audio>` element through Web Audio is
 * a one-way door (`MediaElementSource` cannot be undone) and a rerouted element
 * is subject to stricter browser rules than a plain one — a context that is
 * suspended, and cross-origin media without CORS headers, both come out as
 * silence. So the element is only ever rerouted when a plugin has a unit with
 * `enabled: true`; disabled-only graphs and empty graphs leave it untouched.
 * Once rerouted the chain always stays connected to the destination.
 */

import type { AudioUnit } from "./api";

const FFT_SIZE = 1024;

/** Anything outside these ranges is clamped, not trusted. */
const LIMITS = {
  eqFrequency: { min: 20, max: 20_000 },
  eqQuality: { min: 0.05, max: 20 },
  eqGain: { min: -30, max: 30 },
  threshold: { min: -100, max: 0 },
  ratio: { min: 1, max: 20 },
  attack: { min: 0, max: 1 },
  release: { min: 0, max: 1 },
  knee: { min: 0, max: 40 },
  makeup: { min: -24, max: 24 },
} as const;

let context: AudioContext | null = null;
let head: GainNode | null = null;
let analyser: AnalyserNode | null = null;
let chain: AudioNode[] = [];
let compressor: DynamicsCompressorNode | null = null;
let scratch: Float32Array<ArrayBuffer> | null = null;
let live = false;
let running = false;
let lastShape: string | null = null;

/**
 * Route the element through the graph. Safe to call repeatedly; only the first
 * call does anything. Deliberately the only place a context is created.
 */
function attach(element: HTMLAudioElement): void {
  if (live) return;

  try {
    const created = new AudioContext();
    const source = created.createMediaElementSource(element);
    const gain = created.createGain();
    const meter = created.createAnalyser();
    meter.fftSize = FFT_SIZE;
    meter.smoothingTimeConstant = 0.4;

    source.connect(gain);
    gain.connect(meter);
    meter.connect(created.destination);

    context = created;
    head = gain;
    analyser = meter;
    scratch = new Float32Array(FFT_SIZE);
    live = true;
  } catch (error) {
    // Without a graph the app still plays; it just has no plugin DSP.
    console.error("[poddies] audio graph unavailable:", error);
  }
}

/** Whether the element is currently routed through the graph. When it is, the
 * element's bytes must be same-origin — routed cross-origin media is silenced
 * by the browser, which is why playback URLs go through the media proxy. */
export function isRouted(): boolean {
  return live;
}

/** Browsers start the context suspended until a gesture; call this on play.
 * Returns whether the graph is usable for audio, so callers can surface a
 * problem instead of leaving the element silent inside a dead context. */
export async function resume(): Promise<boolean> {
  if (!context) return false;
  if (context.state !== "running") {
    try {
      await context.resume();
    } catch {
      /* fall through to the state report below */
    }
  }
  running = context.state === "running";
  if (!running) {
    console.warn("[poddies] audio graph still", context.state);
  }
  return running;
}

/** A signature of the unit list — identical signatures skip the rebuild. */
function shapeOf(units: AudioUnit[]): string {
  return JSON.stringify(
    units.map((unit) => ({
      type: unit.type,
      enabled: unit.enabled,
      // Ordered keys, so a reordered band is a different shape (order matters).
      bands: unit.type === "parametric_eq" ? unit.bands : undefined,
      threshold_db: unit.type === "compressor" ? [unit.threshold_db, unit.ratio, unit.attack_ms, unit.release_ms, unit.knee_db, unit.makeup_db] : undefined,
    })),
  );
}

/**
 * Bring the pipeline in line with what the plugins want. One call: routes the
 * element only if an enabled unit asks for it, rebuilds the chain, and does
 * nothing when the request is unchanged.
 */
export function refresh(element: HTMLAudioElement, units: AudioUnit[]): void {
  const enabled = units.filter((unit) => unit.enabled !== false);
  if (enabled.length > 0) attach(element);

  const shape = shapeOf(enabled);
  if (!live || !context || !head || !analyser) return;
  // An unchanged graph between panel change round-trips is skipped entirely:
  // rebuilding mid-stream makes clicks for no audible difference.
  if (shape === lastShape && (enabled.length > 0 || chain.length === 0)) return;
  lastShape = shape;

  for (const node of chain) {
    try {
      node.disconnect();
    } catch {
      /* already detached */
    }
  }
  chain = [];
  compressor = null;
  head.disconnect();

  // The chain always ends connected, whatever happens below: a failing unit
  // costs its own effects, never the route to the speakers.
  let tail: AudioNode = head;
  const reconnect = () => {
    tail.connect(analyser!);
  };

  for (const unit of enabled) {
    try {
      if (unit.type === "parametric_eq") tail = buildEq(tail, unit);
      else if (unit.type === "compressor") tail = buildCompressor(tail, unit);
    } catch (error) {
      console.error("[poddies] audio unit refused:", unit.type, error);
    }
  }
  reconnect();
}

function buildEq(tail: AudioNode, unit: Extract<AudioUnit, { type: "parametric_eq" }>): AudioNode {
  for (const band of unit.bands) {
    const filter = context!.createBiquadFilter();
    filter.type =
      band.kind === "low_shelf"
        ? "lowshelf"
        : band.kind === "high_shelf"
          ? "highshelf"
          : "peaking";
    filter.frequency.value = clamp(band.frequency, LIMITS.eqFrequency);
    filter.Q.value = clamp(band.q, LIMITS.eqQuality);
    filter.gain.value = clamp(band.gain_db, LIMITS.eqGain);
    tail.connect(filter);
    chain.push(filter);
    tail = filter;
  }
  return tail;
}

function buildCompressor(
  tail: AudioNode,
  unit: Extract<AudioUnit, { type: "compressor" }>,
): AudioNode {
  const dynamics = context!.createDynamicsCompressor();
  dynamics.threshold.value = clamp(unit.threshold_db, LIMITS.threshold);
  dynamics.ratio.value = clamp(unit.ratio, LIMITS.ratio);
  dynamics.attack.value = clamp(unit.attack_ms / 1000, LIMITS.attack);
  dynamics.release.value = clamp(unit.release_ms / 1000, LIMITS.release);
  dynamics.knee.value = clamp(unit.knee_db ?? 0, LIMITS.knee);

  const makeup = context!.createGain();
  makeup.gain.value = dbToGain(clamp(unit.makeup_db ?? 0, LIMITS.makeup));

  tail.connect(dynamics);
  chain.push(dynamics);
  dynamics.connect(makeup);
  chain.push(makeup);
  compressor = dynamics;
  return makeup;
}

export interface Meters {
  /** Peak of the output buffer, dBFS. -Infinity when silent. */
  peakDb: number;
  /** How far a compressor is pulling the signal down, dB (0 when idle). */
  reductionDb: number;
}

/** Read the current levels. Cheap enough to call on a timer. */
export function readMeters(): Meters {
  if (!running || !analyser || !scratch) return { peakDb: -Infinity, reductionDb: 0 };

  analyser.getFloatTimeDomainData(scratch);
  let peak = 0;
  for (let index = 0; index < scratch.length; index += 1) {
    const value = Math.abs(scratch[index]);
    if (value > peak) peak = value;
  }

  return {
    peakDb: peak > 0 ? 20 * Math.log10(peak) : -Infinity,
    reductionDb: compressor ? compressor.reduction : 0,
  };
}

function clamp(value: number, range: { min: number; max: number }): number {
  if (!Number.isFinite(value)) return range.min;
  return Math.min(range.max, Math.max(range.min, value));
}

function dbToGain(db: number): number {
  return Math.pow(10, db / 20);
}

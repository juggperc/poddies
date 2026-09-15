/**
 * The playback audio graph.
 *
 * Plugins do not process samples — they hand the host a list of units and the
 * parameters for them, and this module builds real Web Audio nodes. That keeps
 * DSP on the audio thread at zero added latency: nothing crosses the plugin
 * process boundary per sample, so a plugin written in Python is just as viable
 * as one written in Rust.
 *
 * The pristine path matters: if no plugin asks for audio, the `<audio>` element
 * is never touched and plays directly. Once a graph has been built the element
 * is permanently routed through it (a `MediaElementSource` cannot be undone),
 * so the chain is always left connected to the destination.
 */

import type { AudioUnit } from "./api";

const FFT_SIZE = 1024;

let context: AudioContext | null = null;
let head: GainNode | null = null;
let analyser: AnalyserNode | null = null;
let chain: AudioNode[] = [];
let compressor: DynamicsCompressorNode | null = null;
let scratch: Float32Array<ArrayBuffer> | null = null;
let live = false;

/** True once the element is routed through the graph. */
export function isRouted(): boolean {
  return live;
}

/**
 * Route the element through the graph. Safe to call repeatedly; only the first
 * call does anything.
 */
export function attach(element: HTMLAudioElement): void {
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

/** Browsers start the context suspended until a gesture; call this on play. */
export async function resume(): Promise<void> {
  if (context?.state === "suspended") {
    try {
      await context.resume();
    } catch {
      /* the element still plays through the graph in most cases */
    }
  }
}

/** Rebuild the chain from a plugin-supplied unit list. */
export function apply(units: AudioUnit[]): void {
  if (!context || !head || !analyser) return;

  const wanted = units.filter((unit) => unit.enabled !== false);
  if (wanted.length === 0 && chain.length === 0) return;

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

  let tail: AudioNode = head;

  for (const unit of wanted) {
    if (unit.type === "parametric_eq") {
      for (const band of unit.bands) {
        const filter = context.createBiquadFilter();
        filter.type =
          band.kind === "low_shelf"
            ? "lowshelf"
            : band.kind === "high_shelf"
              ? "highshelf"
              : "peaking";
        filter.frequency.value = clamp(band.frequency, 20, 20_000);
        filter.Q.value = clamp(band.q, 0.05, 20);
        filter.gain.value = clamp(band.gain_db, -30, 30);
        tail.connect(filter);
        chain.push(filter);
        tail = filter;
      }
    } else if (unit.type === "compressor") {
      const dynamics = context.createDynamicsCompressor();
      dynamics.threshold.value = clamp(unit.threshold_db, -100, 0);
      dynamics.ratio.value = clamp(unit.ratio, 1, 20);
      dynamics.attack.value = clamp(unit.attack_ms / 1000, 0, 1);
      dynamics.release.value = clamp(unit.release_ms / 1000, 0, 1);
      dynamics.knee.value = clamp(unit.knee_db ?? 0, 0, 40);

      const makeup = context.createGain();
      makeup.gain.value = dbToGain(clamp(unit.makeup_db ?? 0, -24, 24));

      tail.connect(dynamics);
      chain.push(dynamics);
      dynamics.connect(makeup);
      chain.push(makeup);
      tail = makeup;
      compressor = dynamics;
    }
  }

  tail.connect(analyser);
}

export interface Meters {
  /** Peak of the output buffer, dBFS. -Infinity when silent. */
  peakDb: number;
  /** How far a compressor is pulling the signal down, dB (0 when idle). */
  reductionDb: number;
}

/** Read the current levels. Cheap enough to call on a timer. */
export function readMeters(): Meters {
  if (!analyser || !scratch) return { peakDb: -Infinity, reductionDb: 0 };

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

function clamp(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) return min;
  return Math.min(max, Math.max(min, value));
}

function dbToGain(db: number): number {
  return Math.pow(10, db / 20);
}

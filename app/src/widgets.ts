/**
 * Widget rendering.
 *
 * Plugins describe their UI; this module draws it. Interactive widgets report
 * changes through `host.onChange(widgetId, value)` — the host forwards that to
 * the plugin, which answers with fresh state. Keeping rendering here is what
 * lets a plugin panel look like the rest of the app, in the window and in a
 * popout, without the plugin shipping any markup.
 */

import type { EqBand, PanelContent, Widget } from "./api";

export interface WidgetHost {
  onChange(widgetId: string, value: unknown): void;
}

/**
 * Live interaction tracking.
 *
 * A widget mid-drag is a gesture bound to a specific DOM node; replacing that
 * node — with a rebuilt panel whose values "just changed" — kills the drag, so
 * the renderer must defer any rebuild until the gesture ends. Knobs and EQ
 * nodes drag through window listeners; sliders and toggles are native inputs
 * whose gestures are broken the same way by removal. Every control here marks
 * the window while it is held.
 */
let interactions = 0;
export function isInteracting(): boolean {
  return interactions > 0;
}
export function beginInteraction(): void {
  interactions += 1;
}
export function endInteraction(): void {
  interactions = Math.max(0, interactions - 1);
}

const SVG_NS = "http://www.w3.org/2000/svg";

export function renderWidget(widget: Widget, host: WidgetHost): HTMLElement | null {
  const root = buildWidget(widget, host);
  if (!root) return null;
  root.dataset.widgetId = (widget as { id?: string }).id ?? "";
  root.dataset.widgetType = widget.type;
  return root;
}

function buildWidget(widget: Widget, host: WidgetHost): HTMLElement | null {
  switch (widget.type) {
    case "heading":
      return element("div", "widget__heading", widget.text);
    case "text":
      return element("p", "widget__text", widget.text);
    case "divider":
      return element("div", "widget__divider");
    case "metric":
      return element("div", undefined, [
        element("div", "metric__value", widget.value),
        element("div", "metric__label", widget.label),
      ]);
    case "bar":
      return bar(widget);
    case "list":
      return element(
        "div",
        "list",
        widget.items.map((item) =>
          element("div", "list__row", [
            element("span", undefined, item.primary),
            item.secondary ? element("span", "list__secondary", item.secondary) : null,
          ]),
        ),
      );
    case "toggle":
      return toggle(widget, host);
    case "slider":
      return slider(widget, host);
    case "knob":
      return knob(widget, host);
    case "eq":
      return eqCurve(widget, host);
    case "meter":
      return meter(widget);
    default:
      // A widget from a newer minor protocol version; skip it rather than
      // drawing something wrong.
      return null;
  }
}

/* ------------------------------------------------------------------ primitives */

function element(
  tag: string,
  className?: string,
  children?: (Node | string | null)[] | string,
): HTMLElement {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (typeof children === "string") {
    node.textContent = children;
  } else if (children) {
    for (const child of children) {
      if (child !== null) node.append(child);
    }
  }
  return node;
}

function svg(tag: string, attrs: Record<string, string | number>): SVGElement {
  const node = document.createElementNS(SVG_NS, tag);
  for (const [key, value] of Object.entries(attrs)) {
    node.setAttribute(key, String(value));
  }
  return node;
}

const clamp = (value: number, min: number, max: number) =>
  Math.min(max, Math.max(min, value));

function normalise(value: number, min: number, max: number): number {
  if (max === min) return 0;
  return clamp((value - min) / (max - min), 0, 1);
}

function format(value: number, unit: string): string {
  const magnitude = Math.abs(value);
  const digits = magnitude >= 100 ? 0 : magnitude >= 10 ? 1 : 2;
  return `${value.toFixed(digits)}${unit ? ` ${unit}` : ""}`;
}

/* ---------------------------------------------------------------------- bar */

function bar(widget: Extract<Widget, { type: "bar" }>): HTMLElement {
  const ratio = widget.max > 0 ? clamp(widget.value / widget.max, 0, 1) : 0;
  return element("div", "bar", [
    element("div", "bar__head", [
      element("span", undefined, widget.label),
      element("span", undefined, `${Math.round(ratio * 100)}%`),
    ]),
    element("div", "bar__track", [
      Object.assign(element("span", "bar__fill"), {
        style: `width:${(ratio * 100).toFixed(1)}%`,
      }),
    ]),
  ]);
}

/* ------------------------------------------------------------------- toggle */

function toggle(widget: Extract<Widget, { type: "toggle" }>, host: WidgetHost): HTMLElement {
  const input = document.createElement("input");
  input.type = "checkbox";
  input.checked = widget.value;
  input.setAttribute("aria-label", widget.label);
  input.addEventListener("change", () => host.onChange(widget.id, input.checked));

  return element("div", "settings__check", [
    element("label", "check", [input, element("span", undefined, widget.label)]),
  ]);
}

/* ------------------------------------------------------------------- slider */

function slider(widget: Extract<Widget, { type: "slider" }>, host: WidgetHost): HTMLElement {
  const output = element("span", "slider__value", format(widget.value, widget.unit));

  const input = document.createElement("input");
  input.type = "range";
  input.min = String(widget.min);
  input.max = String(widget.max);
  input.step = String(widget.step > 0 ? widget.step : (widget.max - widget.min) / 100);
  input.value = String(widget.value);
  input.setAttribute("aria-label", widget.label);
  input.addEventListener("input", () => {
    const value = Number(input.value);
    output.textContent = format(value, widget.unit);
    host.onChange(widget.id, value);
  });
  // A native range drag runs from pointerdown to pointerup on the input; a
  // panel rebuild in between would remove it mid-gesture.
  input.addEventListener("pointerdown", () => {
    beginInteraction();
    window.addEventListener(
      "pointerup",
      () => {
        endInteraction();
      },
      { once: true },
    );
  });

  return element("div", "slider", [
    element("span", "slider__label", widget.label),
    input,
    output,
  ]);
}

/* --------------------------------------------------------------------- knob */

/**
 * A rotary control. Vertical drag turns it; hold shift for fine movement.
 * `vintage` draws the heavier face — knurled rim, ivory pointer, tick marks.
 */
/**
 * knob geometry, shared by the builder and the patcher so both agree on the
 * drawing. Vertical drag turns the knob; shift for fine movement.
 */
const KNOB_SWEEP = 270;
const KNOB_START = -135;

function knobSize(vintage: boolean): number {
  return vintage ? 96 : 72;
}

function knobPointOn(vintage: boolean, ratio: number, distance: number): {
  x: number;
  y: number;
} {
  const size = knobSize(vintage);
  const centre = size / 2;
  const radians = ((KNOB_START + KNOB_SWEEP * ratio - 90) * Math.PI) / 180;
  return {
    x: centre + Math.cos(radians) * distance,
    y: centre + Math.sin(radians) * distance,
  };
}

function knobArcPath(vintage: boolean, from: number, to: number, distance: number): string {
  const a = knobPointOn(vintage, from, distance);
  const b = knobPointOn(vintage, to, distance);
  const large = Math.abs(to - from) * KNOB_SWEEP > 180 ? 1 : 0;
  return `M ${a.x} ${a.y} A ${distance} ${distance} 0 ${large} 1 ${b.x} ${b.y}`;
}

function knob(widget: Extract<Widget, { type: "knob" }>, host: WidgetHost): HTMLElement {
  const vintage = widget.style === "vintage";
  const size = knobSize(vintage);
  const radius = size / 2 - 6;
  const centre = size / 2;
  const pointOn = (ratio: number, distance: number) => knobPointOn(vintage, ratio, distance);
  const arcPath = (from: number, to: number, distance: number) =>
    knobArcPath(vintage, from, to, distance);

  const root = svg("svg", {
    class: `knob knob--${vintage ? "vintage" : "modern"}`,
    width: size,
    height: size,
    viewBox: `0 0 ${size} ${size}`,
    role: "slider",
    tabindex: "0",
    "aria-label": widget.label,
  });

  const position = () => normalise(widget.value, widget.min, widget.max);

  // Track, then the filled portion up to the current value.
  root.append(
    svg("path", {
      d: arcPath(0, 1, radius),
      class: "knob__track",
      fill: "none",
    }),
  );
  const fill = svg("path", {
    d: arcPath(0, Math.max(position(), 0.001), radius),
    class: "knob__fill",
    fill: "none",
  });
  root.append(fill);

  if (vintage) {
    // Tick marks every 1/8th, heavier at the extremes.
    for (let step = 0; step <= 8; step += 1) {
      const ratio = step / 8;
      const inner = pointOn(ratio, radius - 10);
      const outer = pointOn(ratio, radius - (step % 4 === 0 ? 19 : 16));
      root.append(
        svg("line", {
          x1: inner.x,
          y1: inner.y,
          x2: outer.x,
          y2: outer.y,
          class: step % 4 === 0 ? "knob__tick knob__tick--major" : "knob__tick",
        }),
      );
    }
  }

  root.append(
    svg("circle", { cx: centre, cy: centre, r: radius - (vintage ? 20 : 16), class: "knob__face" }),
  );
  if (vintage) {
    root.append(
      svg("circle", {
        cx: centre,
        cy: centre,
        r: radius - 12,
        class: "knob__knurl",
        fill: "none",
      }),
    );
  }

  const pointer = pointOn(position(), radius - (vintage ? 20 : 16));
  const pointerInner = pointOn(position(), vintage ? 10 : 12);
  const needle = svg("line", {
    x1: pointerInner.x,
    y1: pointerInner.y,
    x2: pointer.x,
    y2: pointer.y,
    class: "knob__pointer",
    "stroke-linecap": "round",
  });
  root.append(needle);

  const readout = element("div", "knob__readout", [
    element("div", "knob__value", format(widget.value, widget.unit)),
    widget.readout ? element("div", "knob__derived", widget.readout) : null,
  ]);

  let current = widget.value;

  const commit = (value: number) => {
    current = clamp(value, widget.min, widget.max);
    const ratio = normalise(current, widget.min, widget.max);
    fill.setAttribute("d", arcPath(0, Math.max(ratio, 0.001), radius));
    const outer = pointOn(ratio, radius - (vintage ? 20 : 16));
    const inner = pointOn(ratio, vintage ? 10 : 12);
    needle.setAttribute("x2", String(outer.x));
    needle.setAttribute("y2", String(outer.y));
    needle.setAttribute("x1", String(inner.x));
    needle.setAttribute("y1", String(inner.y));
    readout.firstElementChild!.textContent = format(current, widget.unit);
    root.setAttribute("aria-valuenow", current.toFixed(3));
    host.onChange(widget.id, current);
  };

  let dragging = false;
  let startY = 0;
  let startValue = 0;

  const onMove = (event: PointerEvent) => {
    if (!dragging) return;
    const range = widget.max - widget.min;
    const scale = event.shiftKey ? 400 : 140;
    commit(startValue + ((startY - event.clientY) / scale) * range);
  };

  const onUp = () => {
    dragging = false;
    endInteraction();
    root.classList.remove("is-dragging");
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
  };

  root.addEventListener("pointerdown", (event) => {
    const pointer = event as PointerEvent;
    pointer.preventDefault();
    dragging = true;
    // A drag starts from the value currently on display, not the value from
    // when this node was built — an external update (another panel, a fresh
    // plugin reply) may have moved it since.
    const onDisplay = Number(root.getAttribute("aria-valuenow"));
    if (Number.isFinite(onDisplay)) current = onDisplay;
    beginInteraction();
    startY = pointer.clientY;
    startValue = current;
    root.classList.add("is-dragging");
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  });

  root.addEventListener("keydown", (event) => {
    const key = (event as KeyboardEvent).key;
    const step = (widget.max - widget.min) / (event.shiftKey ? 100 : 20);
    if (key === "ArrowUp" || key === "ArrowRight") commit(current + step);
    else if (key === "ArrowDown" || key === "ArrowLeft") commit(current - step);
    else return;
    event.preventDefault();
  });

  root.addEventListener("dblclick", () => commit((widget.min + widget.max) / 2));
  root.setAttribute("aria-valuemin", String(widget.min));
  root.setAttribute("aria-valuemax", String(widget.max));
  root.setAttribute("aria-valuenow", String(widget.value));

  return element("div", "knob-wrap", [root, readout, element("div", "knob__label", widget.label)]);
}

/* ----------------------------------------------------------------------- eq */

/**
 * A parametric EQ. The curve is the true summed magnitude response of the
 * bands, computed from the same RBJ coefficients the audio graph uses, so what
 * you see is what you hear. Drag a node vertically for gain, horizontally for
 * frequency.
 */
function eqCurve(widget: Extract<Widget, { type: "eq" }>, host: WidgetHost): HTMLElement {
  const width = 380;
  const height = 150;
  const padding = 10;
  const minFrequency = widget.min_frequency;
  const maxFrequency = widget.max_frequency;
  const minGain = widget.min_gain_db;
  const maxGain = widget.max_gain_db;
  const sampleRate = 48_000;

  const root = svg("svg", {
    class: "eq",
    viewBox: `0 0 ${width} ${height}`,
    preserveAspectRatio: "none",
    role: "group",
    "aria-label": "Equaliser curve",
  });

  const frequencyToX = (frequency: number) => {
    const ratio =
      Math.log(clamp(frequency, minFrequency, maxFrequency) / minFrequency) /
      Math.log(maxFrequency / minFrequency);
    return padding + ratio * (width - padding * 2);
  };
  const xToFrequency = (x: number) => {
    const ratio = clamp((x - padding) / (width - padding * 2), 0, 1);
    return minFrequency * Math.pow(maxFrequency / minFrequency, ratio);
  };
  const gainToY = (gain: number) =>
    padding + (1 - normalise(gain, minGain, maxGain)) * (height - padding * 2);
  const yToGain = (y: number) =>
    minGain + (1 - clamp((y - padding) / (height - padding * 2), 0, 1)) * (maxGain - minGain);

  // A few reference frequencies and the unity line.
  for (const frequency of [100, 1_000, 10_000]) {
    root.append(
      svg("line", {
        x1: frequencyToX(frequency),
        y1: padding,
        x2: frequencyToX(frequency),
        y2: height - padding,
        class: "eq__grid",
      }),
    );
  }
  root.append(
    svg("line", {
      x1: padding,
      y1: gainToY(0),
      x2: width - padding,
      y2: gainToY(0),
      class: "eq__unity",
    }),
  );

  const bands: EqBand[] = widget.bands.map((band) => ({ ...band }));

  const curve = svg("path", { class: "eq__curve", fill: "none" });
  root.append(curve);

  const nodes = bands.map((band) => {
    const node = svg("circle", { class: "eq__node", r: 5.5, tabindex: "0" });
    node.setAttribute("aria-label", band.label);
    root.append(node);
    return node;
  });

  const redraw = () => {
    const steps = 160;
    const parts: string[] = [];
    for (let index = 0; index <= steps; index += 1) {
      const x = padding + (index / steps) * (width - padding * 2);
      const frequency = xToFrequency(x);
      let gain = 0;
      for (const band of bands) {
        gain += biquadGainDb(band, frequency, sampleRate);
      }
      parts.push(`${index === 0 ? "M" : "L"} ${x.toFixed(2)} ${gainToY(gain).toFixed(2)}`);
    }
    curve.setAttribute("d", parts.join(" "));

    bands.forEach((band, index) => {
      nodes[index].setAttribute("cx", String(frequencyToX(band.frequency)));
      nodes[index].setAttribute("cy", String(gainToY(band.gain_db)));
    });
  };

  bands.forEach((band, index) => {
    let dragging = false;
    const node = nodes[index];

    const onMove = (event: PointerEvent) => {
      if (!dragging) return;
      const box = root.getBoundingClientRect();
      const x = ((event.clientX - box.left) / box.width) * width;
      const y = ((event.clientY - box.top) / box.height) * height;
      band.frequency = clamp(xToFrequency(x), minFrequency, maxFrequency);
      band.gain_db = clamp(yToGain(y), minGain, maxGain);
      redraw();
      host.onChange(widget.id, {
        band_id: band.id,
        gain_db: Number(band.gain_db.toFixed(2)),
        frequency: Math.round(band.frequency),
      });
    };

    const onUp = () => {
      dragging = false;
      endInteraction();
      node.classList.remove("is-dragging");
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };

    node.addEventListener("pointerdown", (event) => {
      (event as PointerEvent).preventDefault();
      dragging = true;
      beginInteraction();
      node.classList.add("is-dragging");
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
    });

    node.addEventListener("keydown", (event) => {
      const key = (event as KeyboardEvent).key;
      const step = event.shiftKey ? 0.5 : 2;
      if (key === "ArrowUp") band.gain_db = clamp(band.gain_db + step, minGain, maxGain);
      else if (key === "ArrowDown") band.gain_db = clamp(band.gain_db - step, minGain, maxGain);
      else return;
      event.preventDefault();
      redraw();
      host.onChange(widget.id, {
        band_id: band.id,
        gain_db: Number(band.gain_db.toFixed(2)),
        frequency: Math.round(band.frequency),
      });
    });

    node.addEventListener("dblclick", () => {
      band.gain_db = 0;
      redraw();
      host.onChange(widget.id, {
        band_id: band.id,
        gain_db: 0,
        frequency: Math.round(band.frequency),
      });
    });
  });

  redraw();
  return element("div", "eq-wrap", [root]);
}

/** Magnitude of one band at one frequency, in dB. RBJ cookbook coefficients. */
function biquadGainDb(band: EqBand, frequency: number, sampleRate: number): number {
  if (band.gain_db === 0) return 0;

  const A = Math.pow(10, band.gain_db / 40);
  const w0 = (2 * Math.PI * clamp(band.frequency, 10, sampleRate / 2 - 100)) / sampleRate;
  const cos = Math.cos(w0);
  const sin = Math.sin(w0);
  const alpha = sin / (2 * Math.max(band.q, 0.05));
  const shelfAlpha = (sin / 2) * Math.sqrt((A + 1 / A) * (1 / 0.9 - 1) + 2);

  let b0: number, b1: number, b2: number, a0: number, a1: number, a2: number;

  switch (band.kind) {
    case "low_shelf": {
      const twoSqrtAAlpha = 2 * Math.sqrt(A) * shelfAlpha;
      b0 = A * (A + 1 - (A - 1) * cos + twoSqrtAAlpha);
      b1 = 2 * A * (A - 1 - (A + 1) * cos);
      b2 = A * (A + 1 - (A - 1) * cos - twoSqrtAAlpha);
      a0 = A + 1 + (A - 1) * cos + twoSqrtAAlpha;
      a1 = -2 * (A - 1 + (A + 1) * cos);
      a2 = A + 1 + (A - 1) * cos - twoSqrtAAlpha;
      break;
    }
    case "high_shelf": {
      const twoSqrtAAlpha = 2 * Math.sqrt(A) * shelfAlpha;
      b0 = A * (A + 1 + (A - 1) * cos + twoSqrtAAlpha);
      b1 = -2 * A * (A - 1 + (A + 1) * cos);
      b2 = A * (A + 1 + (A - 1) * cos - twoSqrtAAlpha);
      a0 = A + 1 - (A - 1) * cos + twoSqrtAAlpha;
      a1 = 2 * (A - 1 - (A + 1) * cos);
      a2 = A + 1 - (A - 1) * cos - twoSqrtAAlpha;
      break;
    }
    default: {
      b0 = 1 + alpha * A;
      b1 = -2 * cos;
      b2 = 1 - alpha * A;
      a0 = 1 + alpha / A;
      a1 = -2 * cos;
      a2 = 1 - alpha / A;
    }
  }

  const w = (2 * Math.PI * frequency) / sampleRate;
  const cosW = Math.cos(w);
  const sinW = Math.sin(w);
  const cos2W = Math.cos(2 * w);
  const sin2W = Math.sin(2 * w);

  const numRe = b0 + b1 * cosW + b2 * cos2W;
  const numIm = -(b1 * sinW + b2 * sin2W);
  const denRe = a0 + a1 * cosW + a2 * cos2W;
  const denIm = -(a1 * sinW + a2 * sin2W);

  const magnitude = Math.hypot(numRe, numIm) / (Math.hypot(denRe, denIm) || 1);
  return 20 * Math.log10(magnitude || 1);
}

/* -------------------------------------------------------------------- meter */

/**
 * A live meter. The value comes from the audio engine, not the plugin, so the
 * host paints it directly on a timer — see `paintMeters`.
 */
function meter(widget: Extract<Widget, { type: "meter" }>): HTMLElement {
  const fill = element("div", "meter__fill");
  const readout = element("span", "meter__readout", "—");

  const root = element("div", "meter", [
    element("div", "meter__head", [
      element("span", "meter__label", widget.label),
      readout,
    ]),
    element("div", "meter__track", [fill]),
  ]);
  root.dataset.meter = widget.source;
  root.dataset.minDb = String(widget.min_db);
  root.dataset.maxDb = String(widget.max_db);

  return root;
}

/**
 * Paint every meter currently mounted. Called on a timer while audio plays; the
 * values never round-trip through a plugin.
 */
export function paintMeters(root: ParentNode, peakDb: number, reductionDb: number): void {
  for (const node of root.querySelectorAll<HTMLElement>("[data-meter]")) {
    const source = node.dataset.meter;
    const min = Number(node.dataset.minDb ?? -24);
    const max = Number(node.dataset.maxDb ?? 0);
    const value = source === "gain_reduction" ? reductionDb : peakDb;

    // A gain-reduction meter reads backwards: no reduction (0 dB, the top of
    // the range) is an empty bar, and heavy reduction fills it.
    const ratio =
      source === "gain_reduction"
        ? 1 - clamp(normalise(value, min, max), 0, 1)
        : clamp(normalise(value, min, max), 0, 1);

    const fill = node.querySelector<HTMLElement>(".meter__fill");
    const readout = node.querySelector<HTMLElement>(".meter__readout");
    // Write only on change: these run every tick while anything plays, and an
    // unguarded write forces style recalculation on the whole subtree.
    const nextWidth = `${(ratio * 100).toFixed(1)}%`;
    if (fill && fill.style.width !== nextWidth) fill.style.width = nextWidth;
    if (readout) {
      let nextLabel = "—";
      if (Number.isFinite(value)) {
        // A reading of nothing should read as 0.0, not -0.0.
        const shown = Math.abs(value) < 0.05 ? 0 : value;
        nextLabel = `${shown > 0 ? "+" : ""}${shown.toFixed(1)} dB`;
      }
      if (readout.textContent !== nextLabel) readout.textContent = nextLabel;
    }
    node.classList.toggle("meter--hot", Number.isFinite(value) && value > -3);
  }
}

/* ------------------------------------------------------------------ patching */

export type PanelPatch = "patched" | "mismatch";

/**
 * Refresh the widgets already mounted in `container` from fresh plugin content,
 * without rebuilding the container.
 *
 * This is how a plugin control updates without the page flashing: the knob you
 * turned, the slider you moved, stay exactly where the pointer left them and
 * only their display is pulled to the new value. Returns `"mismatch"` when the
 * widget list changed shape beyond a display refresh (different type at some
 * slot, different min/max geometry, a differently-ordered EQ) so the caller can
 * rebuild that panel properly.
 */
export function patchWidgets(
  container: HTMLElement,
  content: PanelContent,
  host: WidgetHost,
): PanelPatch {
  // A drag holds a gesture on a specific node; a structurally-different
  // answer must not be forced under it. The caller rebuilds it later.
  if (isInteracting()) return "mismatch";

  const current = [...container.children].filter(
    (node): node is HTMLElement =>
      node instanceof HTMLElement && node.dataset.widgetType !== undefined,
  );
  const incoming = content.widgets;
  const paired = Math.min(current.length, incoming.length);

  for (let index = 0; index < paired; index += 1) {
    if (current[index].dataset.widgetType !== incoming[index].type) {
      return "mismatch";
    }
    if (!updateWidgetDisplay(current[index], incoming[index])) {
      return "mismatch";
    }
  }

  for (let index = current.length - 1; index >= incoming.length; index -= 1) {
    current[index].remove();
  }
  for (let index = paired; index < incoming.length; index += 1) {
    const node = renderWidget(incoming[index], host);
    if (node) container.append(node);
  }
  return "patched";
}

/** Pull one mounted widget's display to the incoming widget. `false` = cannot
 * be patched in place (geometry changed); the caller rebuilds the panel. */
function updateWidgetDisplay(root: HTMLElement, widget: Widget): boolean {
  switch (widget.type) {
    case "heading":
    case "text":
      if (root.textContent !== widget.text) root.textContent = widget.text;
      return true;
    case "divider":
      return true;
    case "metric": {
      const [value, label] = [...root.children] as [HTMLElement, HTMLElement];
      setIfChanged(value, widget.value);
      setIfChanged(label, widget.label);
      return true;
    }
    case "bar": {
      const head = root.querySelector<HTMLElement>(".bar__head");
      if (!head) return false;
      setIfChanged(head.children[0] as HTMLElement | undefined, widget.label);
      const ratio = widget.max > 0 ? clamp(widget.value / widget.max, 0, 1) : 0;
      setIfChanged(head.children[1] as HTMLElement | undefined, `${Math.round(ratio * 100)}%`);
      const fill = root.querySelector<HTMLElement>(".bar__fill");
      const nextWidth = `${(ratio * 100).toFixed(1)}%`;
      if (fill && fill.style.width !== nextWidth) fill.style.width = nextWidth;
      return true;
    }
    case "list": {
      const rows = widget.items.map((item) =>
        element("div", "list__row", [
          element("span", undefined, item.primary),
          item.secondary ? element("span", "list__secondary", item.secondary) : null,
        ]),
      );
      root.replaceChildren(...rows);
      return true;
    }
    case "toggle": {
      const input = root.querySelector<HTMLInputElement>("input");
      if (!input) return false;
      if (input.checked !== widget.value) input.checked = widget.value;
      input.setAttribute("aria-label", widget.label);
      setIfChanged(root.querySelector("span") ?? undefined, widget.label);
      return true;
    }
    case "slider": {
      const input = root.querySelector<HTMLInputElement>("input");
      if (!input) return false;
      const nextStep = String(widget.step > 0 ? widget.step : (widget.max - widget.min) / 100);
      const same =
        input.min === String(widget.min) && input.max === String(widget.max) && input.step === nextStep;
      if (!same) return false;
      const next = String(widget.value);
      if (input.value !== next) input.value = next;
      const output = root.querySelector<HTMLElement>(".slider__value");
      if (output) setIfChanged(output, format(widget.value, widget.unit));
      setIfChanged(root.firstElementChild as HTMLElement | undefined, widget.label);
      return true;
    }
    case "knob": {
      if (
        widget.style === "vintage" !== root.contains(root.querySelector(".knob__tick")) ||
        root.getAttribute("aria-valuemin") !== String(widget.min) ||
        root.getAttribute("aria-valuemax") !== String(widget.max)
      ) {
        // Geometry parameters changed; a fresh widget is safer than patching.
        return false;
      }
      const svg = root.querySelector<SVGElement>("svg.knob");
      const needle = root.querySelector<SVGElement>("line.knob__pointer");
      const fill = root.querySelector<SVGElement>("path.knob__fill");
      const value = root.querySelector<HTMLElement>(".knob__value");
      const derived = root.querySelector<HTMLElement>(".knob__derived");
      if (!svg || !needle || !fill || !value) return false;

      const vintage = widget.style === "vintage";
      const radius = knobSize(vintage) / 2 - 6;
      const ratio = normalise(widget.value, widget.min, widget.max);
      fill.setAttribute("d", knobArcPath(vintage, 0, Math.max(ratio, 0.001), radius));
      const outer = knobPointOn(vintage, ratio, radius - (vintage ? 20 : 16));
      const inner = knobPointOn(vintage, ratio, vintage ? 10 : 12);
      needle.setAttribute("x1", String(inner.x));
      needle.setAttribute("y1", String(inner.y));
      needle.setAttribute("x2", String(outer.x));
      needle.setAttribute("y2", String(outer.y));
      svg.setAttribute("aria-valuenow", widget.value.toFixed(3));
      setIfChanged(value, format(widget.value, widget.unit));
      if (derived) setIfChanged(derived, widget.readout ?? "");
      setIfChanged(
        root.querySelector(".knob__label") as HTMLElement | undefined,
        widget.label,
      );
      return true;
    }
    case "eq":
    case "meter":
      // The EQ curve and meters repaint from engine state; rebuild the widget.
      return false;
    default:
      return false;
  }
}

function setIfChanged(node: HTMLElement | undefined, text: string): void {
  if (node && node.textContent !== text) node.textContent = text;
}

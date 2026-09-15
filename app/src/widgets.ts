/**
 * Widget rendering.
 *
 * Plugins describe their UI; this module draws it. Interactive widgets report
 * changes through `host.onChange(widgetId, value)` — the host forwards that to
 * the plugin, which answers with fresh state. Keeping rendering here is what
 * lets a plugin panel look like the rest of the app, in the window and in a
 * popout, without the plugin shipping any markup.
 */

import type { EqBand, Widget } from "./api";

export interface WidgetHost {
  onChange(widgetId: string, value: unknown): void;
}

const SVG_NS = "http://www.w3.org/2000/svg";

export function renderWidget(widget: Widget, host: WidgetHost): HTMLElement | null {
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
function knob(widget: Extract<Widget, { type: "knob" }>, host: WidgetHost): HTMLElement {
  const vintage = widget.style === "vintage";
  const size = vintage ? 96 : 72;
  const radius = size / 2 - 6;
  const centre = size / 2;
  const sweep = 270;
  const start = -135;

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
  const angleFor = (ratio: number) => start + sweep * ratio;
  const pointOn = (ratio: number, distance: number) => {
    const radians = ((angleFor(ratio) - 90) * Math.PI) / 180;
    return {
      x: centre + Math.cos(radians) * distance,
      y: centre + Math.sin(radians) * distance,
    };
  };
  const arcPath = (from: number, to: number, distance: number) => {
    const a = pointOn(from, distance);
    const b = pointOn(to, distance);
    const large = Math.abs(to - from) * sweep > 180 ? 1 : 0;
    return `M ${a.x} ${a.y} A ${distance} ${distance} 0 ${large} 1 ${b.x} ${b.y}`;
  };

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
    root.classList.remove("is-dragging");
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
  };

  root.addEventListener("pointerdown", (event) => {
    const pointer = event as PointerEvent;
    pointer.preventDefault();
    dragging = true;
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
      node.classList.remove("is-dragging");
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };

    node.addEventListener("pointerdown", (event) => {
      (event as PointerEvent).preventDefault();
      dragging = true;
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
    if (fill) fill.style.width = `${(ratio * 100).toFixed(1)}%`;
    if (readout) {
      if (!Number.isFinite(value)) {
        readout.textContent = "—";
      } else {
        // A reading of nothing should read as 0.0, not -0.0.
        const shown = Math.abs(value) < 0.05 ? 0 : value;
        readout.textContent = `${shown > 0 ? "+" : ""}${shown.toFixed(1)} dB`;
      }
    }
    node.classList.toggle("meter--hot", Number.isFinite(value) && value > -3);
  }
}

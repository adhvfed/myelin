// The sign-in art: a myelin sheath in cross-section. Concentric layers around a core, generated
// from a small rule (thickness on a Fibonacci scale, runs on a 24-step angular grid) rather than
// drawn, so different pages can show different cuts of the same form.
import { For, Show } from "solid-js";

/** One layer: its thickness in units, and its runs as [start step, sweep in steps]. */
export type SheathRing = readonly [thickness: number, runs: readonly (readonly [number, number])[]];

export interface SheathCut {
  /** Radius of the innermost layer, and the space between layers, in units. */
  inner: number;
  gap: number;
  /** The one run drawn in the accent colour: [ring index, run index]. */
  accent: readonly [number, number];
  rings: readonly SheathRing[];
}

/** A settled sheath: long sweeping runs, weight low and left. */
export const SIGN_IN_CUT: SheathCut = {
  inner: 6.3,
  gap: 1.5,
  accent: [4, 1],
  rings: [
    [1, [[2, 9]]],
    [1, [[14, 7]]],
    [2, [[5, 8], [19, 3]]],
    [3, [[9, 9]]],
    [5, [[3, 5], [13, 6]]],
    [8, [[7, 7], [20, 2]]],
    [13, [[11, 8]]],
    [21, [[5, 5]]],
  ],
};

/** A sheath that can close: one long run per layer, meant to be drawn partially. */
export const JOIN_CUT: SheathCut = {
  inner: 6.3,
  gap: 1.6,
  accent: [5, 0],
  rings: [
    [1, [[3, 21]]],
    [1, [[15, 21]]],
    [2, [[6, 20]]],
    [3, [[10, 20]]],
    [5, [[2, 19]]],
    [5, [[13, 22]]],
    [8, [[8, 10]]],
    [13, [[11, 5]]],
  ],
};

const STEP_DEGREES = 360 / 24;
const UNIT = 7;
const CENTRE = { x: 420, y: 440 };
export const SHEATH_VIEW_BOX = "0 0 840 800";

interface Run {
  d: string;
  accent: boolean;
}

interface Ring {
  radius: number;
  width: number;
  runs: Run[];
}

function point(radius: number, degrees: number): string {
  const radians = (degrees * Math.PI) / 180;
  const x = CENTRE.x + radius * Math.cos(radians);
  const y = CENTRE.y + radius * Math.sin(radians);
  return `${x.toFixed(2)} ${y.toFixed(2)}`;
}

function layout(cut: SheathCut): Ring[] {
  let inner = cut.inner * UNIT;
  return cut.rings.map(([thickness, runs], ringIndex) => {
    const width = thickness * UNIT;
    const radius = inner + width / 2;
    inner += width + cut.gap * UNIT;
    return {
      radius,
      width,
      runs: runs.map(([start, sweep], runIndex) => ({
        d: `M${point(radius, start * STEP_DEGREES)} A${radius} ${radius} 0 ${
          sweep * STEP_DEGREES > 180 ? 1 : 0
        } 1 ${point(radius, (start + sweep) * STEP_DEGREES)}`,
        accent: cut.accent[0] === ringIndex && cut.accent[1] === runIndex,
      })),
    };
  });
}

export function Sheath(props: {
  cut: SheathCut;
  /** How much of each layer is drawn, 0–1, by ring index. Omit for a fully drawn sheath. */
  fill?: (ring: number) => number;
  /** Faint full circles under the layers, so a partly drawn sheath still has a shape. */
  guides?: boolean;
  /** The layers turn while something is in flight. */
  busy?: boolean;
}) {
  const rings = () => layout(props.cut);
  return (
    <svg
      class="sheath"
      viewBox={SHEATH_VIEW_BOX}
      preserveAspectRatio="xMidYMid slice"
      aria-hidden="true"
      data-busy={props.busy ? "" : undefined}
    >
      <For each={rings()}>
        {(ring, index) => (
          <g class="sheath-ring" style={{ "--i": index() }}>
            <Show when={props.guides}>
              <circle class="sheath-guide" cx={CENTRE.x} cy={CENTRE.y} r={ring.radius} />
            </Show>
            <For each={ring.runs}>
              {(run) => (
                <path
                  class="sheath-run"
                  classList={{ "sheath-run--accent": run.accent }}
                  d={run.d}
                  pathLength="1"
                  stroke-width={ring.width}
                  style={
                    props.fill
                      ? { "stroke-dasharray": `${Math.max(0, Math.min(1, props.fill(index())))} 1` }
                      : undefined
                  }
                />
              )}
            </For>
          </g>
        )}
      </For>
      <circle class="sheath-core" cx={CENTRE.x} cy={CENTRE.y} r={UNIT * 1.3} />
    </svg>
  );
}

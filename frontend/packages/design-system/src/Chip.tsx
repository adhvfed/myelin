// Shared inline reference for issues, runs, docs, commits, messages, links, and agents. Withheld
// references omit their title and render as neutral, non-link text.
import { Show, mergeProps, type JSX } from "solid-js";
import { Icon } from "./Icon";
import { type IconName } from "./icon-names";

/** The reference kind → its pre-resolution glyph (from the 42-icon registry). */
export type ChipType = "issue" | "run" | "doc" | "message" | "commit" | "link" | "agent";

/** The resolved reference state (reference-chip §5). `live` renders normally; the rest carry a small
 *  state pill; `no_access`/`tombstoned` withhold the title entirely (non-leak). */
export type ChipState =
  | "live"
  | "no_access"
  | "moved"
  | "outdated"
  | "tombstoned"
  | "cross_cell"
  | "degraded";

export interface ChipProps {
  type: ChipType;
  /** The visible label. For `no_access`/`tombstoned` this is a neutral collapsed label ("Restricted"). */
  label: string;
  state?: ChipState;
  /** A visible status word rendered after the label ("failed" / "in progress"). */
  statusLabel?: string;
  /** The navigation target. Absent (or a withheld state) → the chip renders as a non-link span. */
  href?: string;
  /** A residency tag for a cross-cell reference ("eu-central-1 · Frankfurt"). */
  region?: string;
  /** Called on activation when there is no href (e.g. open-in-pane later). */
  onActivate?: () => void;
  style?: JSX.CSSProperties;
}

const TYPE_GLYPH: Record<ChipType, IconName> = {
  issue: "issue",
  run: "run",
  doc: "doc",
  message: "message",
  commit: "commit",
  link: "link",
  agent: "agent",
};

/** The visible state label for a non-live reference. */
function stateWord(state: ChipState): string | null {
  switch (state) {
    case "moved":
      return "moved";
    case "outdated":
      return "outdated";
    case "tombstoned":
      return "removed";
    case "degraded":
      return "can't refresh";
    case "no_access":
      return "restricted";
    case "cross_cell":
      return null; // the region tag carries this
    default:
      return null;
  }
}

/**
 * `<Chip>` — an inline reference chip. Renders as an anchor when `href` is present and the reference is
 * readable; otherwise a span (a withheld/absent reference is never a dead link).
 */
export function Chip(rawProps: ChipProps): JSX.Element {
  const props = mergeProps({ state: "live" as ChipState }, rawProps);
  const withheld = () => props.state === "no_access" || props.state === "tombstoned";
  const sword = () => stateWord(props.state);
  // The accessible name spells out the type + label + any status word (a screen reader gets the full
  // meaning; the glyph is decorative).
  const ariaLabel = () =>
    [props.type, props.label, props.statusLabel, sword(), props.region]
      .filter(Boolean)
      .join(", ");

  const inner = (
    <>
      <Icon name={TYPE_GLYPH[props.type]} title={props.type} />
      <span style={{ "font-weight": "500" }}>{props.label}</span>
      <Show when={props.statusLabel}>
        <span style={{ color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>
          {props.statusLabel}
        </span>
      </Show>
      <Show when={sword()}>
        <span
          class="chip-state"
          style={{
            "font-size": "var(--fs-caption)",
            color: "var(--text-subtle)",
            border: "var(--hairline) solid var(--border)",
            "border-radius": "var(--radius-pill)",
            padding: "0 var(--space-1)",
          }}
        >
          {sword()}
        </span>
      </Show>
      <Show when={props.region}>
        <span style={{ color: "var(--text-subtle)", "font-family": "var(--font-mono)", "font-size": "var(--fs-caption)" }}>
          {props.region}
        </span>
      </Show>
    </>
  );

  const chipStyle = (): JSX.CSSProperties => ({
    display: "inline-flex",
    "align-items": "center",
    gap: "var(--space-1)",
    padding: "var(--space-1) var(--space-2)",
    border: "var(--hairline) solid var(--border)",
    "border-radius": "var(--radius-1)",
    background: "var(--surface-raised)",
    "max-width": "100%",
    "text-decoration": "none",
    color: withheld() ? "var(--text-muted)" : "var(--text-primary)",
    ...props.style,
  });

  return (
    <Show
      when={props.href && !withheld()}
      fallback={
        <Show
          when={props.onActivate && !withheld()}
          fallback={
            <span class="chip" data-chip-state={props.state} aria-label={ariaLabel()} style={chipStyle()}>
              {inner}
            </span>
          }
        >
          <button
            type="button"
            class="chip"
            data-chip-state={props.state}
            aria-label={ariaLabel()}
            style={{ ...chipStyle(), appearance: "none", cursor: "pointer", font: "inherit" }}
            onClick={() => props.onActivate?.()}
          >
            {inner}
          </button>
        </Show>
      }
    >
      <a class="chip" data-chip-state={props.state} href={props.href} aria-label={ariaLabel()} style={chipStyle()}>
        {inner}
      </a>
    </Show>
  );
}

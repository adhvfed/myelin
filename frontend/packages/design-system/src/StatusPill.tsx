// StatusPill — the shared glyph+label status primitive (R3.1, contributed DOWN into the design
// system per gate Q6). ONE pill that every status cell reaches for: the PR state (open/draft/merged/
// closed), Issue state, and the checks-summary verdict (pass/fail/running/none/unavailable). Two hard rules from
// DESIGN-MANUAL §3.1 / WCAG 1.4.1, enforced by construction here so no surface re-invents them:
//   • **Status is TEXT, never colour alone** — every pill renders a visible label AND a `title`, and
//     colour only ever tints the GLYPH (never the label, so a colour-blind or greyscale reader still
//     reads the state).
//   • **The verdict RING is reserved for the CI trio** — `check-verdict` uses the ring glyphs
//     (check-pass/fail/pending); `pr-state` uses non-ring glyphs (pull-request/edit/merge/close), so
//     the ring keeps meaning "a CI verdict", never a PR lifecycle state.
//
// Semantic tokens only; the accent is never used as text (a §3.1 hard rail).
import { Match, Switch, type JSX } from "solid-js";
import { Icon } from "./Icon";
import { type IconName } from "./icon-names";

/** The PR lifecycle state the `pr-state` variant renders. */
export type PrStateValue = "open" | "draft" | "merged" | "closed";
/** The checks-summary verdict the `check-verdict` variant renders (mirrors the edge `verdict` token). */
export type CheckVerdict = "pass" | "fail" | "running" | "none" | "unavailable";
/** Frozen cross-project Issues workflow categories. The visible label remains the project's state. */
export type IssueStateCategory = "unstarted" | "started" | "completed" | "cancelled";

export interface PrStatePillProps {
  kind: "pr-state";
  state: PrStateValue;
  /** For a merged PR, the checks were green at merge — lets the caller nuance the label upstream. */
  style?: JSX.CSSProperties;
}

export interface CheckVerdictPillProps {
  kind: "check-verdict";
  verdict: CheckVerdict;
  /** Required contexts currently passing (drives "N passing"/"N running"). */
  passing?: number;
  /** Required contexts witnessed failing. */
  failing?: number;
  /** Required contexts in total. */
  total?: number;
  /** A merged PR whose checks were green — renders "merged green" rather than "all passing". */
  merged?: boolean;
  style?: JSX.CSSProperties;
}

export interface IssueStatePillProps {
  kind: "issue-state";
  category: IssueStateCategory;
  label: string;
  style?: JSX.CSSProperties;
}

export type StatusPillProps = PrStatePillProps | CheckVerdictPillProps | IssueStatePillProps;

// ── pr-state: glyph tint per state; the label is always readable text ──
const PR_STATE: Record<PrStateValue, { icon: IconName; label: string; glyph: string; labelColor: string }> = {
  open:   { icon: "pull-request", label: "Open",   glyph: "var(--success)",     labelColor: "var(--text-primary)" },
  draft:  { icon: "edit",         label: "Draft",  glyph: "var(--text-subtle)", labelColor: "var(--text-muted)" },
  merged: { icon: "merge",        label: "Merged", glyph: "var(--agent)",       labelColor: "var(--text-primary)" },
  closed: { icon: "close",        label: "Closed", glyph: "var(--danger)",      labelColor: "var(--text-primary)" },
};

const ISSUE_STATE: Record<IssueStateCategory, { icon: IconName; glyph: string }> = {
  unstarted: { icon: "issue", glyph: "var(--text-subtle)" },
  started: { icon: "cycle", glyph: "var(--warning)" },
  completed: { icon: "issue", glyph: "var(--success)" },
  cancelled: { icon: "close", glyph: "var(--danger)" },
};

/** The verdict label — TEXT, derived from the counts so a reader never depends on colour. */
export function checkVerdictLabel(value: CheckVerdictPillProps): string {
  const passing = value.passing ?? 0;
  const failing = value.failing ?? 0;
  const total = value.total ?? 0;
  switch (value.verdict) {
    case "pass":
      return value.merged ? "merged green" : total > 0 ? "all passing" : "passing";
    case "fail":
      return failing > 0 ? `${failing} failing` : "failing";
    case "running": {
      const pending = Math.max(total - passing - failing, 1);
      return `${pending} running`;
    }
    case "unavailable":
      return "checks unavailable";
    case "none":
    default:
      return "no checks";
  }
}

const VERDICT_GLYPH: Record<CheckVerdict, { icon: IconName; glyph: string }> = {
  pass:        { icon: "check-pass",    glyph: "var(--success)" },
  fail:        { icon: "check-fail",    glyph: "var(--danger)" },
  running:     { icon: "check-pending", glyph: "var(--warning)" },
  none:        { icon: "check-pending", glyph: "var(--text-subtle)" },
  unavailable: { icon: "check-pending", glyph: "var(--text-subtle)" },
};

/**
 * The shared status pill. `pr-state` renders a bordered pill (the row's leading state chip);
 * `check-verdict` renders an inline glyph+label (the row's right-cluster checks cell). Both carry a
 * `title` + visible label so status is legible without colour.
 */
export function StatusPill(props: StatusPillProps): JSX.Element {
  return (
    <Switch fallback={<CheckVerdictCell {...(props as CheckVerdictPillProps)} />}>
      <Match when={props.kind === "pr-state"}>
        <PrStateChip {...(props as PrStatePillProps)} />
      </Match>
      <Match when={props.kind === "issue-state"}>
        <IssueStateChip {...(props as IssueStatePillProps)} />
      </Match>
    </Switch>
  );
}

function PrStateChip(props: PrStatePillProps): JSX.Element {
  const spec = () => PR_STATE[props.state];
  return (
    <span
      title={`State: ${spec().label}`}
      style={{
        display: "inline-flex",
        "align-items": "center",
        gap: "var(--space-1)",
        "font-size": "var(--fs-caption)",
        "font-weight": "var(--weight-medium)",
        border: "var(--hairline) solid var(--border)",
        "border-radius": "var(--radius-pill)",
        padding: "0 var(--space-2)",
        color: spec().labelColor,
        "white-space": "nowrap",
        ...props.style,
      }}
    >
      <span style={{ color: spec().glyph, display: "inline-flex" }}>
        <Icon name={spec().icon} size={12} />
      </span>
      {spec().label}
    </span>
  );
}

function IssueStateChip(props: IssueStatePillProps): JSX.Element {
  const spec = () => ISSUE_STATE[props.category];
  return (
    <span
      title={`State: ${props.label}`}
      style={{
        display: "inline-flex",
        "align-items": "center",
        gap: "var(--space-1)",
        "font-size": "var(--fs-caption)",
        "font-weight": "var(--weight-medium)",
        border: "var(--hairline) solid var(--border)",
        "border-radius": "var(--radius-pill)",
        padding: "0 var(--space-2)",
        color: "var(--text-primary)",
        "white-space": "nowrap",
        ...props.style,
      }}
    >
      <span style={{ color: spec().glyph, display: "inline-flex" }}>
        <Icon name={spec().icon} size={12} />
      </span>
      {props.label}
    </span>
  );
}

function CheckVerdictCell(props: CheckVerdictPillProps): JSX.Element {
  const spec = () => VERDICT_GLYPH[props.verdict];
  const label = () => checkVerdictLabel(props);
  return (
    <span
      title={`Checks: ${label()}`}
      style={{
        display: "inline-flex",
        "align-items": "center",
        gap: "var(--space-1)",
        color: "var(--text-muted)",
        "font-size": "var(--fs-caption)",
        "white-space": "nowrap",
      }}
    >
      <span style={{ color: spec().glyph, display: "inline-flex" }}>
        <Icon name={spec().icon} size={13} />
      </span>
      {label()}
    </span>
  );
}

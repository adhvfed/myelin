// Labelled context-pane section with independent loading and error states.
import { ErrorBoundary, Show, createUniqueId, type JSX } from "solid-js";
import { Icon } from "./Icon";
import { Skeleton } from "./Skeleton";

export interface PaneSectionProps {
  /** The visible slot label (rendered as an h3 — the heading an SR jumps to). */
  label: string;
  /** True while the slot's data is loading → the label stays, the body is a skeleton, `aria-busy`. */
  busy?: boolean;
  /** The skeleton's line count while busy. */
  skeletonRows?: number;
  /** A scoped failure message if this section throws. */
  failLabel?: string;
  children?: JSX.Element;
  style?: JSX.CSSProperties;
}

/**
 * `<PaneSection>` — one labelled, independently-loading, fail-static context-pane slot.
 */
export function PaneSection(props: PaneSectionProps): JSX.Element {
  const headingId = createUniqueId();
  return (
    <section
      aria-labelledby={headingId}
      aria-busy={props.busy ? "true" : undefined}
      style={{
        display: "flex",
        "flex-direction": "column",
        gap: "var(--space-2)",
        ...props.style,
      }}
    >
      <h3
        id={headingId}
        style={{
          "font-size": "var(--fs-caption)",
          "text-transform": "uppercase",
          "letter-spacing": "0.04em",
          color: "var(--text-muted)",
          margin: "0",
        }}
      >
        {props.label}
      </h3>
      <Show
        when={!props.busy}
        fallback={<Skeleton label={`Loading ${props.label}…`} rows={props.skeletonRows ?? 2} rowHeight="1.5rem" />}
      >
        <ErrorBoundary
          fallback={() => (
            <div
              role="note"
              style={{
                display: "flex",
                "align-items": "center",
                gap: "var(--space-2)",
                color: "var(--text-muted)",
                "font-size": "var(--fs-caption)",
              }}
            >
              <Icon name="link" />
              <span>{props.failLabel ?? `${props.label} unavailable`}</span>
            </div>
          )}
        >
          {props.children}
        </ErrorBoundary>
      </Show>
    </section>
  );
}

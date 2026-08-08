import { Show } from "solid-js";
import { A } from "@solidjs/router";
import { Icon } from "@myelin/design-system";
import {
  CI_ERR_PREFIX,
  CiRouteError,
  type CiErrorKind,
} from "~/lib/api";

export function ciErrKind(error: unknown): CiErrorKind {
  if (error instanceof CiRouteError) return error.kind;
  const message = error instanceof Error ? error.message : String(error ?? "");
  if (message.startsWith(CI_ERR_PREFIX)) {
    const kind = message.slice(CI_ERR_PREFIX.length);
    if (kind === "bad-input" || kind === "not-found" || kind === "stale" ||
        kind === "unavailable" || kind === "error") return kind;
  }
  return "error";
}

const card = {
  border: "var(--hairline) solid var(--border)",
  "border-radius": "var(--radius-1)",
  padding: "var(--space-5)",
  background: "var(--surface-raised)",
  display: "flex",
  "flex-direction": "column",
  "align-items": "center",
  gap: "var(--space-3)",
  "text-align": "center",
} as const;

const action = {
  display: "inline-flex",
  "align-items": "center",
  gap: "var(--space-1)",
  padding: "var(--space-2) var(--space-3)",
  border: "var(--hairline) solid var(--border)",
  "border-radius": "var(--radius-1)",
  color: "var(--text-primary)",
  background: "var(--surface)",
} as const;

export function CiErrorState(props: {
  kind: CiErrorKind;
  latestHref?: string;
  onRetry?: () => void;
}) {
  return (
    <Show when={props.kind === "stale"} fallback={<OrdinaryCiError {...props} />}>
      <div role="alert" aria-live="assertive" data-testid="ci-error" data-kind="stale" style={card}>
        <Icon name="cycle" size={28} title="Runs changed" />
        <h2 style={{ "font-size": "var(--fs-h3)", margin: "0" }}>The run list changed</h2>
        <p style={{ color: "var(--text-muted)", margin: "0", "max-width": "42ch" }}>
          Repository visibility or the selected filter changed while you were paging. Reload from
          the latest authorized run.
        </p>
        {/* A stale cursor is a failed route resource. Use a document navigation so the boundary and
            the router query cache are both rebuilt from a cursor-free request. */}
        <a href={props.latestHref ?? "/ci"} target="_self" style={action}>
          <Icon name="cycle" /> Reload latest runs
        </a>
      </div>
    </Show>
  );
}

function OrdinaryCiError(props: {
  kind: CiErrorKind;
  latestHref?: string;
  onRetry?: () => void;
}) {
  const unavailable = () => props.kind === "unavailable" || props.kind === "error";
  return (
    <Show
      when={props.kind === "not-found"}
      fallback={
        <Show
          when={props.kind === "bad-input"}
          fallback={
            <div role="alert" aria-live="assertive" data-testid="ci-error" data-kind="unavailable" style={card}>
              <Icon name="check-fail" size={28} title="Unavailable" />
              <h2 style={{ "font-size": "var(--fs-h3)", margin: "0" }}>CI data is unavailable</h2>
              <p style={{ color: "var(--text-muted)", margin: "0", "max-width": "42ch" }}>
                We couldn&rsquo;t read the durable CI projection right now. This is on our side.
              </p>
              <Show when={unavailable() && props.onRetry}>
                <button type="button" onClick={() => props.onRetry?.()} style={{ ...action, cursor: "pointer" }}>
                  <Icon name="rerun" /> Retry
                </button>
              </Show>
            </div>
          }
        >
          <div role="note" data-testid="ci-error" data-kind="bad-input" style={card}>
            <Icon name="search" size={28} title="Invalid address" />
            <h2 style={{ "font-size": "var(--fs-h3)", margin: "0" }}>That CI address is invalid</h2>
            <p style={{ color: "var(--text-muted)", margin: "0", "max-width": "42ch" }}>
              Return to the run list and choose an available run.
            </p>
            <A href="/ci" style={action}><Icon name="nav-ci" /> CI runs</A>
          </div>
        </Show>
      }
    >
      <div role="note" data-testid="ci-error" data-kind="not-found" style={card}>
        <Icon name="search" size={28} title="Not available" />
        <h2 style={{ "font-size": "var(--fs-h3)", margin: "0" }}>This run is not available to you</h2>
        <p style={{ color: "var(--text-muted)", margin: "0", "max-width": "42ch" }}>
          It may not exist, or its repository may not be visible to your current identity.
        </p>
        <A href="/ci" style={action}><Icon name="nav-ci" /> CI runs</A>
      </div>
    </Show>
  );
}

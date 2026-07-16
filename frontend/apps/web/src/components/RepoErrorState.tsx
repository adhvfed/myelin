// The shared dignified error trio (R3.4 / R-21) — no-access · not-found · retryable-error. Spec'd
// ONCE and reused by every repo-browsing route so the distinction is enforced uniformly and the raw
// `String(err.message ?? err)` fallback (findings 7) is never rendered as content. Anti-oracle: the
// edge serves the 0-leak 404 on a Pull deny, so no-access can be indistinguishable from not-found —
// the copy never claims more than the signal warrants. Semantic tokens only; a11y per the manual.
import { Show } from "solid-js";
import { A } from "@solidjs/router";
import { Icon } from "@myelin/design-system";
import { REPO_ERR_PREFIX, RepoRouteError, type RepoErrorKind } from "~/lib/api";

/** Extract the mapped kind from a thrown error (the `RepoRouteError.kind`, or the `REPO_ERR:<kind>`
 *  message prefix that survives the server→client boundary), defaulting to the retryable `error`. */
export function errKind(err: unknown): RepoErrorKind {
  if (err instanceof RepoRouteError) return err.kind;
  const msg = err instanceof Error ? err.message : String(err ?? "");
  if (msg.startsWith(REPO_ERR_PREFIX)) {
    const k = msg.slice(REPO_ERR_PREFIX.length);
    if (k === "no-access" || k === "not-found" || k === "error") return k;
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

const btn = {
  display: "inline-flex",
  "align-items": "center",
  gap: "var(--space-1)",
  padding: "var(--space-2) var(--space-3)",
  border: "var(--hairline) solid var(--border)",
  "border-radius": "var(--radius-1)",
  color: "var(--text-primary)",
  background: "var(--surface)",
  cursor: "pointer",
} as const;

export interface RepoErrorStateProps {
  kind: RepoErrorKind;
  /** The repo slug (bare) — drives the not-found copy + the "Repo home" action. */
  repo?: string;
  /** Retry handler (the ErrorBoundary `reset`) — wired on the retryable `error` kind. */
  onRetry?: () => void;
}

export function RepoErrorState(props: RepoErrorStateProps) {
  return (
    <Show when={props.kind === "no-access"} fallback={<NotFoundOrError {...props} />}>
      <div role="note" data-testid="repo-error" data-kind="no-access" style={card}>
        <Icon name="gate" size={28} title="No access" />
        <h2 style={{ "font-size": "var(--fs-h3)", margin: "0" }}>
          This repository is not available to you
        </h2>
        <p style={{ color: "var(--text-muted)", margin: "0", "max-width": "36ch" }}>
          You don&rsquo;t have access, or it may not exist. Ask an owner to grant you access.
        </p>
        <A href="/git/repos" style={btn}>
          <Icon name="nav-code" /> Back to repositories
        </A>
      </div>
    </Show>
  );
}

function NotFoundOrError(props: RepoErrorStateProps) {
  return (
    <Show when={props.kind === "not-found"} fallback={<RetryableError {...props} />}>
      <div role="note" data-testid="repo-error" data-kind="not-found" style={card}>
        <Icon name="search" size={28} title="Not found" />
        <h2 style={{ "font-size": "var(--fs-h3)", margin: "0" }}>We couldn&rsquo;t find that</h2>
        <p style={{ color: "var(--text-muted)", margin: "0", "max-width": "40ch" }}>
          This ref or path doesn&rsquo;t exist{props.repo ? ` on ${props.repo}` : ""}.
        </p>
        <div style={{ display: "flex", gap: "var(--space-2)", "flex-wrap": "wrap", "justify-content": "center" }}>
          <Show when={props.repo}>
            <A href={`/git/repos/${props.repo}`} style={btn}>
              <Icon name="repo" /> Repo home
            </A>
          </Show>
          <A href="/git/repos" style={btn}>
            <Icon name="nav-code" /> Repositories
          </A>
        </div>
      </div>
    </Show>
  );
}

function RetryableError(props: RepoErrorStateProps) {
  // The retryable failure — role="alert" (assertive) so the settle is announced; a Retry that KEEPS
  // context (never announces the raw error text).
  return (
    <div
      role="alert"
      aria-live="assertive"
      data-testid="repo-error"
      data-kind="error"
      style={card}
    >
      <Icon name="check-fail" size={28} title="Error" />
      <h2 style={{ "font-size": "var(--fs-h3)", margin: "0" }}>Something went wrong</h2>
      <p style={{ color: "var(--text-muted)", margin: "0", "max-width": "40ch" }}>
        We couldn&rsquo;t load this right now. This is on our side &mdash; your place is kept.
      </p>
      <Show when={props.onRetry}>
        <button
          type="button"
          style={{ ...btn, background: "var(--c-btn-primary-bg, var(--surface))" }}
          onClick={() => props.onRetry?.()}
        >
          <Icon name="rerun" /> Retry
        </button>
      </Show>
    </div>
  );
}

// PR overview (GT-004) — `/git/repos/{repo}/prs/{n}`. Composes the durable PR record (state/refs/
// author) with the checks + merge-gate projection: the checks panel (required / green / endorsed), the
// fork-trust badge for un-endorsed untrusted-fork runs (the signed-off X-1 affordance — a fork's own
// green NEVER reads as gating-green), and merge readiness that REFLECTS the server's authoritative
// `gate_admitted` (the UI never recomputes policy; a blocked gate names WHY, read-only). The merge +
// review ACTIONS are GT-004b. Unglamorous states first-class. Semantic tokens only.
import { ErrorBoundary, For, Show, Suspense, createMemo } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, useParams } from "@solidjs/router";
import { Icon, type IconName } from "@myelin/design-system";
import { getPr, getPrChecks, type PrChecksVM } from "~/lib/api";
import { NotAvailable } from "~/components/NotAvailable";

interface StatePill { icon: IconName; color: string; label: string }
const STATE_STYLE: Record<string, StatePill> = {
  open: { icon: "pull-request", color: "var(--info)", label: "open" },
  draft: { icon: "edit", color: "var(--text-muted)", label: "draft" },
  merged: { icon: "merge", color: "var(--success)", label: "merged" },
  closed: { icon: "close", color: "var(--danger)", label: "closed" },
};

const card = {
  border: "var(--hairline) solid var(--border)",
  "border-radius": "var(--radius-1)",
  padding: "var(--space-3)",
  background: "var(--surface-raised)",
} as const;

export default function PrOverviewScreen() {
  const params = useParams();
  // Guard the route segments: a deep-link missing {repo,n} (or a non-numeric n) renders a not-found.
  const ready = () => Boolean(params.repo && params.n && Number.isFinite(Number(params.n)));
  const pr = createAsync(async () => {
    const repo = params.repo;
    const n = Number(params.n);
    return repo && Number.isFinite(n) ? getPr({ repo, n }) : undefined;
  });
  const checks = createAsync(async () => {
    const repo = params.repo;
    const n = Number(params.n);
    return repo && Number.isFinite(n) ? getPrChecks({ repo, n }) : undefined;
  });

  return (
    <section aria-labelledby="pr-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-3)" }}>
      <Title>PR #{params.n} · {params.repo} · Myelin</Title>
      <nav aria-label="Breadcrumb" style={{ "font-size": "var(--fs-caption)", display: "flex", gap: "var(--space-1)" }}>
        <A href="/git/repos" style={{ color: "var(--text-muted)" }}>Repositories</A>
        <span aria-hidden="true">/</span>
        <A href={`/git/repos/${params.repo}`} style={{ color: "var(--text-muted)" }}>{params.repo}</A>
      </nav>

      <ErrorBoundary
        fallback={(err) => (
          <div role="note" data-testid="pr-restricted" style={{ ...card, color: "var(--text-muted)", display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
            <Icon name="gate" /> <span>This pull request is not available: {String(err.message ?? err)}</span>
          </div>
        )}
      >
        <Suspense fallback={<p style={{ color: "var(--text-muted)" }}>Loading pull request…</p>}>
          <Show when={ready()} fallback={<NotAvailable kind="pull request" />}>
          <Show when={pr()} keyed>
            {(p) => {
              const s: StatePill = STATE_STYLE[p.pr_state] ?? { icon: "pull-request", color: "var(--info)", label: p.pr_state };
              return (
                <>
                  <h1 id="pr-heading" style={{ "font-size": "var(--fs-h1)", margin: "0", display: "flex", "align-items": "center", gap: "var(--space-2)", "flex-wrap": "wrap" }}>
                    <span>Pull request #{p.number}</span>
                    <span data-testid="pr-state" style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", "font-size": "var(--fs-caption)", padding: "var(--space-1) var(--space-2)", border: `var(--hairline) solid ${s.color}`, "border-radius": "var(--radius-pill)", color: s.color }}>
                      <Icon name={s.icon} /> {s.label}
                    </span>
                  </h1>
                  <p style={{ color: "var(--text-muted)", margin: "0" }}>
                    <code style={{ "font-family": "var(--font-mono)" }}>{p.head_ref}</code> → <code style={{ "font-family": "var(--font-mono)" }}>{p.base_ref}</code>
                    {" · "}by {p.author} · {p.reviews} review{p.reviews === 1 ? "" : "s"}
                  </p>

                  <Show when={checks()} keyed fallback={<p style={{ color: "var(--text-muted)" }}>Loading checks…</p>}>
                    {(ck) => <ChecksAndMerge checks={ck} />}
                  </Show>

                  <p style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)", margin: "0" }}>
                    Submitting a review and merging from the browser are GT-004b. This page reflects the server-side gate; it never bypasses it.
                  </p>
                </>
              );
            }}
          </Show>
          </Show>
        </Suspense>
      </ErrorBoundary>
    </section>
  );
}

function ChecksAndMerge(props: { checks: PrChecksVM }) {
  const greenSet = createMemo(() => new Set(props.checks.green_contexts));
  const forkSet = createMemo(() => new Set(props.checks.fork_unendorsed_contexts));
  const blockedReasons = createMemo(() => {
    const reasons: string[] = [];
    for (const ctx of props.checks.required_contexts) {
      if (forkSet().has(ctx)) reasons.push(`${ctx} awaiting fork trust`);
      else if (!greenSet().has(ctx)) reasons.push(`${ctx} not green`);
    }
    if (props.checks.required_approvals > 0) reasons.push(`${props.checks.required_approvals} approval(s) required`);
    return reasons;
  });

  return (
    <>
      <section aria-labelledby="checks-heading" style={{ ...card, display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
        <h2 id="checks-heading" style={{ "font-size": "var(--fs-h3)", margin: "0" }}>Checks</h2>
        <Show
          when={props.checks.required_contexts.length > 0}
          fallback={<p style={{ color: "var(--text-muted)", margin: "0" }}>No required checks configured for this branch.</p>}
        >
          <ul data-testid="pr-checks" style={{ "list-style": "none", margin: "0", padding: "0", display: "flex", "flex-direction": "column", gap: "var(--space-1)" }}>
            <For each={props.checks.required_contexts}>
              {(ctx) => {
                const fork = forkSet().has(ctx);
                const green = greenSet().has(ctx);
                const cue = fork
                  ? { icon: "gate" as IconName, color: "var(--warning)", label: "untrusted fork — neutral until trusted" }
                  : green
                    ? { icon: "check-pass" as IconName, color: "var(--success)", label: "passed" }
                    : { icon: "check-pending" as IconName, color: "var(--text-muted)", label: "not reported" };
                return (
                  <li style={{ display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
                    <span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: cue.color }}>
                      <Icon name={cue.icon} /> <span>{cue.label}</span>
                    </span>
                    <code style={{ "font-family": "var(--font-mono)" }}>{ctx}</code>
                    <span style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>required</span>
                  </li>
                );
              }}
            </For>
          </ul>
        </Show>
        <Show when={props.checks.fork_unendorsed_contexts.length > 0}>
          <p role="note" data-testid="fork-trust" style={{ margin: "0", color: "var(--warning)", "font-size": "var(--fs-caption)" }}>
            <Icon name="gate" /> A run executed code from an untrusted fork. It does NOT satisfy the gate by itself — a maintainer must trust it (GT-004b).
          </p>
        </Show>
      </section>

      <section aria-labelledby="merge-heading" style={{ ...card, display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
        <h2 id="merge-heading" style={{ "font-size": "var(--fs-h3)", margin: "0" }}>Merge readiness</h2>
        <Show
          when={props.checks.gate_admitted}
          fallback={
            <div data-testid="merge-blocked" style={{ color: "var(--warning)", display: "flex", "flex-direction": "column", gap: "var(--space-1)" }}>
              <span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)" }}><Icon name="gate" /> <strong>Blocked by branch protection</strong></span>
              <ul style={{ margin: "0", "padding-inline-start": "var(--space-4)" }}>
                <For each={blockedReasons()}>{(r) => <li>{r}</li>}</For>
              </ul>
            </div>
          }
        >
          <span data-testid="merge-ready" style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--success)" }}>
            <Icon name="check-pass" /> <strong>Ready to merge</strong> — all required checks satisfied.
          </span>
        </Show>
      </section>
    </>
  );
}

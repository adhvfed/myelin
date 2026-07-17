// Cross-repo PR front door (R3.1) — `/prs`. The "what needs me" surface across repos: two buckets,
// "Needs your review" (the attention job) and "Your PRs", each a saved query on the one PR list. NOT a
// sixth rail item (PRs are part of Code — P1); reached from the Code-landing header, the repo headers,
// the inbox, and ⌘K. Prefiltered leak-free by the `visible_repos` list_objects seam server-side (a
// repo the viewer cannot pull never contributes a PR). Semantic tokens only; StatusPill for status.
import { ErrorBoundary, For, Show, Suspense, createSignal, createEffect } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync } from "@solidjs/router";
import { Icon, Skeleton, SkeletonBlock, StatusPill } from "@myelin/design-system";
import { getMyPrs, type PrListRowVM, type PrListPage } from "~/lib/api";
import { prTitleText, isTitleFallback, updatedLabel, reviewMarker, bucketPageSummary } from "~/lib/pr-view";

export default function CrossRepoPrsScreen() {
  const needsReview = createAsync(() => getMyPrs({ bucket: "needs-review" }));
  const yours = createAsync(() => getMyPrs({ bucket: "yours" }));

  return (
    <section aria-labelledby="myprs-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-4)" }}>
      <Title>Your pull requests · Myelin</Title>
      <nav aria-label="Breadcrumb" style={{ "font-size": "var(--fs-caption)", display: "flex", gap: "var(--space-1)" }}>
        <A href="/git/repos" style={{ color: "var(--text-muted)" }}>Code</A>
        <span aria-hidden="true">/</span>
        <span aria-current="page" style={{ color: "var(--text-muted)" }}>Pull requests</span>
      </nav>
      <h1 id="myprs-heading" style={{ "font-size": "var(--fs-h1)", margin: "0" }}>Your pull requests</h1>

      <Bucket
        testid="bucket-needs-review"
        heading="Needs your review"
        icon="message"
        hint="You are a requested reviewer. Merge stays gated to a human even where an agent has reviewed."
        emptyText="No pull requests are waiting on your review."
        data={needsReview()}
      />
      <Bucket
        testid="bucket-yours"
        heading="Your PRs"
        icon="human"
        emptyText="You haven't opened any pull requests yet."
        data={yours()}
      />
    </section>
  );
}

function Bucket(props: {
  testid: string;
  heading: string;
  icon: "message" | "human";
  hint?: string;
  emptyText: string;
  data: PrListPage | undefined;
}) {
  // Roving-tabindex composite per bucket (one Tab stop; arrows + j/k re-rove; Enter opens).
  const [active, setActive] = createSignal(0);
  const rowEls: (HTMLAnchorElement | undefined)[] = [];
  const items = () => props.data?.items ?? [];
  createEffect(() => {
    items();
    setActive(0);
  });
  const focusRow = (i: number) => {
    const len = items().length;
    if (len === 0) return;
    const n = ((i % len) + len) % len;
    setActive(n);
    rowEls[n]?.focus();
  };
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "j") { e.preventDefault(); focusRow(active() + 1); }
    else if (e.key === "ArrowUp" || e.key === "k") { e.preventDefault(); focusRow(active() - 1); }
  };

  return (
    <section aria-labelledby={`${props.testid}-h`} data-testid={props.testid} style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
      <h2 id={`${props.testid}-h`} style={{ "font-size": "var(--fs-h3)", margin: "0", display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
        <Icon name={props.icon} /> {props.heading}
        <Show when={props.data}>
          {(d) => {
            // #21a: the chip shows the TRUE total (page.total), not the page size — never a page count
            // masquerading as the whole.
            const summary = () => bucketPageSummary(d());
            return (
              <span
                data-testid={`${props.testid}-count`}
                title={summary().truncated ? `Showing ${summary().shown} of ${summary().count}` : undefined}
                style={{ "font-family": "var(--font-mono)", "font-size": "var(--fs-caption)", color: "var(--text-muted)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-pill)", padding: "0 var(--space-2)" }}
              >
                {summary().count}
              </span>
            );
          }}
        </Show>
      </h2>
      <Show when={props.hint}>
        <p style={{ margin: "0", color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>{props.hint}</p>
      </Show>

      <ErrorBoundary
        fallback={() => (
          <div role="alert" data-testid={`${props.testid}-error`} style={{ display: "flex", "align-items": "flex-start", gap: "var(--space-2)", padding: "var(--space-3)", border: "var(--hairline) solid var(--danger)", "border-radius": "var(--radius-1)" }}>
            <Icon name="check-fail" title="Error" />
            <span style={{ color: "var(--text-muted)" }}>We couldn't load these pull requests. This is scoped to the section — the rest of the page is still live.</span>
          </div>
        )}
      >
        <Suspense fallback={<Skeleton label="Loading pull requests…" rows={3} rowHeight="3rem" data-testid={`${props.testid}-loading`}><SkeletonBlock height="3rem" /><SkeletonBlock height="3rem" /></Skeleton>}>
          <Show
            when={items().length > 0}
            fallback={<p data-testid={`${props.testid}-empty`} style={{ margin: "0", color: "var(--text-muted)" }}>{props.emptyText}</p>}
          >
            <ul role="list" aria-label={`${props.heading}, ${items().length} items`} onKeyDown={onKeyDown} style={{ "list-style": "none", margin: "0", padding: "0", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", overflow: "hidden" }}>
              <For each={items()}>
                {(row, i) => (
                  <li role="listitem">
                    <CrossRow row={row} active={active() === i()} setRef={(el) => (rowEls[i()] = el)} onFocus={() => setActive(i())} />
                  </li>
                )}
              </For>
            </ul>
            {/* #21a: DISCLOSE truncation instead of silently dropping the rest (the chip already shows the
                true total). The cross-repo list returns everything today, so this only appears if/when the
                endpoint starts paginating — never a silent shortfall. */}
            <Show when={props.data && bucketPageSummary(props.data)} keyed>
              {(s) => (
                <Show when={s.truncated}>
                  <p data-testid={`${props.testid}-truncated`} style={{ margin: "0", color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>
                    Showing {s.shown} of {s.count}. Open a repository's pull requests to see the rest.
                  </p>
                </Show>
              )}
            </Show>
          </Show>
        </Suspense>
      </ErrorBoundary>
    </section>
  );
}

function CrossRow(props: { row: PrListRowVM; active: boolean; setRef: (el: HTMLAnchorElement) => void; onFocus: () => void }) {
  const row = () => props.row;
  const repo = () => row().repo ?? "";
  const marker = () => reviewMarker(row());
  return (
    <A
      ref={props.setRef}
      href={`/git/repos/${encodeURIComponent(repo())}/prs/${row().number}`}
      tabindex={props.active ? 0 : -1}
      onFocus={props.onFocus}
      data-testid="pr-row"
      style={{ display: "grid", "grid-template-columns": "auto 1fr auto", "align-items": "start", gap: "var(--space-3)", padding: "var(--space-3)", "text-decoration": "none", color: "inherit", "border-block-end": "var(--hairline) solid var(--border)" }}
    >
      <StatusPill kind="pr-state" state={row().pr_state} />
      <span style={{ "min-width": "0", display: "flex", "flex-direction": "column", gap: "var(--space-1)" }}>
        <span style={{ "font-weight": "var(--weight-medium)", color: "var(--text-primary)" }}>
          <span style={isTitleFallback(row()) ? { color: "var(--text-subtle)", "font-family": "var(--font-mono)" } : {}}>{prTitleText(row())}</span>{" "}
          <span style={{ color: "var(--text-subtle)", "font-family": "var(--font-mono)", "font-weight": "var(--weight-regular)", "font-size": "var(--fs-caption)" }}>{repo()}#{row().number}</span>
        </span>
        <span style={{ display: "flex", "flex-wrap": "wrap", "align-items": "center", gap: "var(--space-1) var(--space-3)", color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>
          <span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", "font-family": "var(--font-mono)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", padding: "0 var(--space-1)", background: "var(--surface-raised)" }}>
            <Icon name="repo" /> {repo()}
          </span>
        </span>
      </span>
      <span style={{ display: "flex", "flex-direction": "column", "align-items": "flex-end", gap: "var(--space-1)", "white-space": "nowrap", "font-size": "var(--fs-caption)" }}>
        <StatusPill kind="check-verdict" verdict={row().checks_summary.verdict} passing={row().checks_summary.passing} failing={row().checks_summary.failing} total={row().checks_summary.total} merged={row().pr_state === "merged"} />
        <Show when={marker()} fallback={<span style={{ color: "var(--text-subtle)" }}>{updatedLabel(row())}</span>}>
          {(m) => <span style={{ color: "var(--info)", display: "inline-flex", "align-items": "center", gap: "var(--space-1)" }} title="Your review requested"><Icon name="message" /> {m()}</span>}
        </Show>
      </span>
    </A>
  );
}

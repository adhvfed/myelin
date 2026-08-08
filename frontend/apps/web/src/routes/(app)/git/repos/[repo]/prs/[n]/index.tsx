// PR overview (R3.3 · G-6 wedge flagship + G-8 review verdicts + G-9 checks panel) —
// `/git/repos/{repo}/prs/{n}`. Composes the durable PR record (title/body/state/refs/author) with:
//   • the shell-owned CONTEXT PANE (linked issue / CI / doc / agent slots, via `useContextPane`);
//   • the CHECKS panel in its OWN local ErrorBoundary — a checks projection failure degrades to
//     "Checks unavailable" and the PR stays live (ux-git finding 5: never "PR not available");
//   • the discussion (threads with anchor null) inline, with a composer + reply;
//   • the reviews with glyph+label verdicts + the BATCHED review bar (Start review → pending comments
//     → Submit with a verdict) — ONE event on submit, the human verdict feeds the gate;
//   • the merge card that REFLECTS the server's authoritative `gate_admitted` (never recomputes policy)
//     with a ConfirmDialog (alertdialog, safe-action default focus) that re-verifies on a 409.
// Semantic tokens only; status is TEXT never colour-alone; every unglamorous state is first-class.
import {
  ErrorBoundary,
  For,
  Show,
  Suspense,
  createMemo,
  createSignal,
  createEffect,
  onCleanup,
  untrack,
} from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, revalidate, useAction, useParams } from "@solidjs/router";
import {
  Icon,
  Skeleton,
  SkeletonBlock,
  Chip,
  PaneSection,
  ConfirmDialog,
  Dialog,
  useToast,
  type IconName,
} from "@myelin/design-system";
import {
  getPr,
  getPrChecks,
  getPrThreads,
  getPrCommits,
  PR_COMMITS_PAGE_LIMIT,
  RepoRouteError,
  prMutate,
  type MergeResult,
  type PrVM,
  type PrChecksVM,
  type PrThreadVM,
  type PrReviewVM,
  type PrincipalVM,
  type PrCommitsPage,
} from "~/lib/api";
import { NotAvailable } from "~/components/NotAvailable";
import { PrHeader } from "~/components/PrHeader";
import { RepoErrorState, errKind } from "~/components/RepoErrorState";
import { Markdown } from "~/components/Markdown";
import { useContextPane } from "~/components/AppShell";

const card = {
  border: "var(--hairline) solid var(--border)",
  "border-radius": "var(--radius-1)",
  padding: "var(--space-3)",
  background: "var(--surface-raised)",
  display: "flex",
  "flex-direction": "column",
  gap: "var(--space-2)",
} as const;

/** The checks region degrades locally: a projection failure resolves to this sentinel (never a throw). */
type ChecksUnavailableSentinel = { unavailable: true };
type ChecksState = PrChecksVM | ChecksUnavailableSentinel | undefined;
type PrCommitsState = {
  repo: string;
  n: number;
  page: PrCommitsPage | null;
  unavailable: boolean;
} | undefined;
function isUnavailable(c: ChecksState): c is ChecksUnavailableSentinel {
  return c != null && (c as ChecksUnavailableSentinel).unavailable === true;
}
function checksOrNull(c: ChecksState): PrChecksVM | null {
  return c != null && !isUnavailable(c) ? (c as PrChecksVM) : null;
}

const VERDICT: Record<PrReviewVM["verdict"], { icon: IconName; label: string; color: string }> = {
  approved: { icon: "approve", label: "Approved", color: "var(--success)" },
  changes_requested: { icon: "reject", label: "Changes requested", color: "var(--danger)" },
  commented: { icon: "message", label: "Commented", color: "var(--text-muted)" },
  in_progress: { icon: "edit", label: "In progress", color: "var(--text-subtle)" },
};

export default function PrOverviewScreen() {
  const params = useParams();
  const toast = useToast();
  const ready = () => Boolean(params.repo && params.n && Number.isFinite(Number(params.n)));
  const repo = () => params.repo ?? "";
  const n = () => Number(params.n);

  const pr = createAsync(
    async () => (ready() ? getPr({ repo: repo(), n: n() }) : undefined),
    { deferStream: true },
  );
  // The checks resource degrades LOCALLY (ux-git finding 5): a projection failure resolves to an
  // `{ unavailable }` sentinel — NEVER a throw that would fail the whole PR page. The 401→/login
  // redirect (a thrown Response) still propagates; only a mapped edge failure becomes the sentinel.
  const checks = createAsync(
    async (): Promise<ChecksState> => {
      if (!ready()) return undefined;
      try {
        return await getPrChecks({ repo: repo(), n: n() });
      } catch (e) {
        if (e instanceof RepoRouteError) return { unavailable: true };
        throw e;
      }
    },
    { deferStream: true },
  );
  const threads = createAsync(
    async () => (ready() ? getPrThreads({ repo: repo(), n: n() }) : undefined),
    { deferStream: true },
  );
  const commits = createAsync(
    async (): Promise<PrCommitsState> => {
      if (!ready()) return undefined;
      const requestRepo = repo();
      const requestNumber = n();
      try {
        return {
          repo: requestRepo,
          n: requestNumber,
          page: await getPrCommits({
            repo: requestRepo,
            n: requestNumber,
            limit: PR_COMMITS_PAGE_LIMIT,
          }),
          unavailable: false,
        };
      } catch (error) {
        if (error instanceof Response) throw error;
        return { repo: requestRepo, n: requestNumber, page: null, unavailable: true };
      }
    },
    { deferStream: true },
  );

  const reload = async () => {
    await Promise.all([
      revalidate("git-pr-threads"),
      revalidate("git-pr-checks"),
      revalidate("git-pr"),
    ]);
  };

  // The shell-owned context pane (G-6). Set as a render thunk so the inline column and the narrow
  // drawer never share a DOM node; dropped on unmount so leaving the PR clears the pane.
  const paneApi = useContextPane();
  createEffect(() => {
    paneApi.setContextPaneLabel("Pull request context");
    paneApi.setContextPane(() => <PrContextPane checks={checksOrNull(checks())} reviews={threads()?.reviews} />);
    onCleanup(() => paneApi.setContextPane(null));
  });

  return (
    <section aria-labelledby="pr-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-4)" }}>
      <Title>PR #{params.n} · {params.repo} · Myelin</Title>
      <nav aria-label="Breadcrumb" style={{ "font-size": "var(--fs-caption)", display: "flex", gap: "var(--space-1)" }}>
        <A href="/git/repos" style={{ color: "var(--text-muted)" }}>Repositories</A>
        <span aria-hidden="true">/</span>
        <A href={`/git/repos/${params.repo}`} style={{ color: "var(--text-muted)" }}>{params.repo}</A>
        <span aria-hidden="true">/</span>
        <A href={`/git/repos/${params.repo}/prs`} style={{ color: "var(--text-muted)" }}>Pull requests</A>
      </nav>

      {/* Route-level failure: the dignified not-found / no-access / error trio (anti-oracle — a 404
          PR is indistinguishable from a no-pull-grant one; never a raw err.message). */}
      <ErrorBoundary
        fallback={(err) => <RepoErrorState kind={errKind(err)} repo={params.repo} />}
      >
        <Suspense
          fallback={
            <Skeleton label="Loading pull request…" data-testid="pr-loading">
              <SkeletonBlock height="var(--fs-h1)" width="16rem" />
              <SkeletonBlock height="1.25rem" width="24rem" style={{ "margin-block-start": "var(--space-2)" }} />
              <SkeletonBlock height="8rem" style={{ "margin-block-start": "var(--space-3)" }} />
            </Skeleton>
          }
        >
          <Show when={ready()} fallback={<NotAvailable kind="pull request" status="missing" />}>
            {/* NOT keyed — a mutation action auto-revalidates `pr()`; a keyed Show would re-create the
                children (losing the reviews section's in-progress draft). Non-keyed keeps them mounted
                and reactive, so a submitted review re-renders in place without dropping the batch. */}
            <Show when={pr()}>
              {(p) => (
                <>
                  <PrHeader pr={p()} repo={repo()} active="overview" commitsCount={p().commits_count} />

                  <Show when={p().body_md}>
                    <section aria-label="Description" style={{ ...card }}>
                      <Markdown source={p().body_md ?? ""} />
                    </section>
                  </Show>

                  {/* G-9 CHECKS — degrades LOCALLY: an `unavailable` sentinel renders "Checks
                      unavailable" HERE, the PR stays live (never "PR not available", finding 5). The
                      ErrorBoundary is the belt-and-braces for an unexpected render throw. */}
                  <ErrorBoundary fallback={() => <ChecksUnavailable onRetry={() => void revalidate("git-pr-checks")} />}>
                    <Show
                      when={checks() !== undefined}
                      fallback={<Skeleton label="Loading checks…" rows={3} rowHeight="2rem" data-testid="checks-loading" />}
                    >
                      <Show
                        when={checksOrNull(checks())}
                        keyed
                        fallback={<ChecksUnavailable onRetry={() => void revalidate("git-pr-checks")} />}
                      >
                        {(ck) => <ChecksPanel checks={ck} />}
                      </Show>
                    </Show>
                  </ErrorBoundary>

                  {/* G-8 REVIEWS + the batched review bar. */}
                  <ReviewsSection
                    repo={repo()}
                    n={n()}
                    reviews={threads()?.reviews ?? []}
                    onChange={reload}
                    toast={toast}
                  />

                  {/* Commits IN this PR. */}
                  <CommitsSection repo={repo()} n={n()} initial={commits()} />

                  {/* Discussion (threads with anchor null) inline. */}
                  <DiscussionSection
                    repo={repo()}
                    n={n()}
                    threads={(threads()?.discussion ?? []) as PrThreadVM[]}
                    loading={threads() === undefined}
                    onChange={reload}
                  />

                  {/* MERGE card — reflects the authoritative gate; ConfirmDialog re-verifies on 409.
                      When checks are unavailable it degrades to "Gate state unavailable" (never
                      fabricates a gate). */}
                  <Show
                    when={checksOrNull(checks())}
                    keyed
                    fallback={
                      <Show when={isUnavailable(checks())}>
                        <section aria-labelledby="merge-heading" style={{ ...card, color: "var(--text-muted)" }} data-testid="merge-degraded">
                          <h2 id="merge-heading" style={{ "font-size": "var(--fs-h3)", margin: "0" }}>Merge readiness</h2>
                          <p role="note" style={{ margin: "0" }}>Gate state unavailable — the checks service didn't respond, so merge readiness can't be shown. The gate is never assumed.</p>
                        </section>
                      </Show>
                    }
                  >
                    {(ck) => (
                      <MergeCard repo={repo()} n={n()} pr={p()} checks={ck} onChange={reload} toast={toast} />
                    )}
                  </Show>
                </>
              )}
            </Show>
          </Show>
        </Suspense>
      </ErrorBoundary>
    </section>
  );
}

// ── header ──────────────────────────────────────────────────────────────────────────────────────

// ── checks panel (G-9) + its local failure state ──────────────────────────────────────────────────

function ChecksUnavailable(props: { onRetry: () => void }) {
  // ux-git finding 5: a system-blaming one-liner scoped to the checks region, with a retry — the PR
  // stays live around it (never "PR not available").
  return (
    <section aria-labelledby="checks-heading" data-testid="checks-unavailable" style={{ ...card }}>
      <h2 id="checks-heading" style={{ "font-size": "var(--fs-h3)", margin: "0" }}>Checks</h2>
      <p role="note" style={{ margin: "0", color: "var(--text-muted)" }}>Checks unavailable — the checks service didn't respond. The rest of the pull request is unaffected.</p>
      <button type="button" onClick={() => props.onRetry()} style={{ ...barBtn, "align-self": "flex-start" }}>
        Retry
      </button>
    </section>
  );
}

function ChecksPanel(props: { checks: PrChecksVM }) {
  const greenSet = createMemo(() => new Set(props.checks.green_contexts));
  const forkSet = createMemo(() => new Set(props.checks.fork_unendorsed_contexts));
  return (
    <section aria-labelledby="checks-heading" style={{ ...card }} data-testid="checks-panel">
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
      {/* R-21 #22 — the fork-trust note survives the reskin (a fork's green never reads as gating-green). */}
      <Show when={props.checks.fork_unendorsed_contexts.length > 0}>
        <p role="note" data-testid="fork-trust" style={{ margin: "0", color: "var(--warning)", "font-size": "var(--fs-caption)" }}>
          <Icon name="gate" /> A run executed code from an untrusted fork. It does NOT satisfy the gate by itself — a maintainer must trust it.
        </p>
      </Show>
    </section>
  );
}

// ── the context pane (G-6) ──────────────────────────────────────────────────────────────────────

function PrContextPane(props: { checks: PrChecksVM | null; reviews?: PrReviewVM[] }) {
  // The CI slot is fed from the checks projection (a live artifact); linked issue/doc slots are honest
  // FLOORS — the viewer-scoped linked-refs resolver (N4) is a named follow-on, so the slots render an
  // honest empty state, never a fabricated link. The agent slot is ABSENT unless an agent review
  // exists (Q8 — absent when no agent activity; not a fake "no agent" for a permitted-and-none tenant).
  const agentReviews = createMemo(() => (props.reviews ?? []).filter((r) => r.advisory));
  const ciVerdict = () =>
    props.checks == null
      ? { label: "Checks pending", state: "degraded" as const, status: "loading" }
      : props.checks.gate_admitted
        ? { label: "Gate admitted", state: "live" as const, status: "ready" }
        : { label: "Gate blocked", state: "live" as const, status: "blocked" };

  return (
    <div style={{ display: "flex", "flex-direction": "column", gap: "var(--space-4)" }}>
      <h2 style={{ "font-size": "var(--fs-h3)", margin: "0" }}>Context</h2>

      <PaneSection label="CI run">
        <Show when={props.checks} fallback={<span style={{ color: "var(--text-muted)" }}>No checks reported.</span>}>
          <Chip type="run" label={ciVerdict().label} state={ciVerdict().state} statusLabel={ciVerdict().status} />
        </Show>
      </PaneSection>

      <PaneSection label="Linked issue">
        {/* FLOOR: the viewer-scoped linked-refs resolver (N4) is a named follow-on. */}
        <span style={{ color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>No linked issue yet.</span>
      </PaneSection>

      <PaneSection label="Linked doc">
        <span style={{ color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>No linked doc yet.</span>
      </PaneSection>

      {/* Agent slot — present ONLY when there is agent activity (advisory review). */}
      <Show when={agentReviews().length > 0}>
        <PaneSection label="Agent">
          <For each={agentReviews()}>
            {(r) => <Chip type="agent" label={r.reviewer.display} statusLabel="advisory — never gates" state="live" />}
          </For>
        </PaneSection>
      </Show>
    </div>
  );
}

// ── reviews (G-8) ─────────────────────────────────────────────────────────────────────────────────

function ReviewsSection(props: {
  repo: string;
  n: number;
  reviews: PrReviewVM[];
  onChange: () => Promise<void>;
  toast: ReturnType<typeof useToast>;
}) {
  const doMutate = useAction(prMutate);
  const [draft, setDraft] = createSignal<PrReviewVM | null>(null);
  const [verdictOpen, setVerdictOpen] = createSignal(false);
  const [pendingText, setPendingText] = createSignal("");
  const [summaryText, setSummaryText] = createSignal("");
  const submitted = createMemo(() => props.reviews.filter((r) => r.verdict !== "in_progress"));

  // Rehydrate the in-progress batch from the server (finding #18): `threads().reviews` returns the
  // viewer's OWN un-submitted draft (the projection hides other reviewers' drafts — pr_threads.rs
  // `view_for`), so on load/reload we RESUME it instead of leaving "Start a review" to double-create a
  // second orphan batch. Only seeds while `draft()` is null, so it never clobbers live local edits;
  // submit/discard clear the draft AFTER the reviews refetch, so a resolved batch is not resurrected.
  createEffect(() => {
    if (draft()) return;
    const resumable = props.reviews.find((r) => r.verdict === "in_progress");
    if (resumable) setDraft(resumable);
  });

  const start = async () => {
    try {
      const r = await doMutate({ op: "review-start", repo: props.repo, n: props.n });
      if ("review" in r) setDraft(r.review);
    } catch {
      props.toast.show({ title: "Could not start a review", variant: "danger" });
    }
  };
  const addPending = async () => {
    const d = draft();
    const text = pendingText().trim();
    if (!d || !text) return;
    try {
      await doMutate({ op: "review-comment", repo: props.repo, n: props.n, reviewId: d.id, body_md: text });
      setPendingText("");
      props.toast.show({ title: "Pending comment added (only you can see it)", variant: "info" });
    } catch {
      props.toast.show({ title: "Could not add the comment", variant: "danger" });
    }
  };
  const submit = async (verdict: "approved" | "changes_requested" | "commented") => {
    const d = draft();
    if (!d) return;
    try {
      await doMutate({ op: "review-submit", repo: props.repo, n: props.n, reviewId: d.id, verdict, summary_md: summaryText() });
      // Refetch FIRST so `props.reviews` no longer carries an in_progress batch, THEN drop the local
      // draft — otherwise the rehydrate effect would re-seed the just-submitted batch from stale data.
      await props.onChange();
      setDraft(null);
      setVerdictOpen(false);
      setSummaryText("");
      props.toast.show({ title: "Review submitted", variant: "success" });
    } catch {
      props.toast.show({ title: "Could not submit the review", variant: "danger" });
    }
  };
  const discard = async () => {
    const d = draft();
    if (!d) return;
    try {
      await doMutate({ op: "review-discard", repo: props.repo, n: props.n, reviewId: d.id });
      // Drop the server draft so the rehydrate effect can't resurrect it, THEN clear the local draft.
      await props.onChange();
    } catch {
      /* a discard failure is non-fatal — the draft is the reviewer's private state */
    }
    setDraft(null);
    setVerdictOpen(false);
  };

  return (
    <section aria-labelledby="reviews-heading" style={{ ...card }} data-testid="reviews">
      <h2 id="reviews-heading" style={{ "font-size": "var(--fs-h3)", margin: "0" }}>Reviews</h2>
      <Show
        when={submitted().length > 0}
        fallback={<p style={{ color: "var(--text-muted)", margin: "0" }}>No reviews yet.</p>}
      >
        <ul style={{ "list-style": "none", margin: "0", padding: "0", display: "flex", "flex-direction": "column", gap: "var(--space-1)" }}>
          <For each={submitted()}>
            {(r) => {
              const v = VERDICT[r.verdict];
              return (
                <li style={{ display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
                  <span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: v.color }}>
                    <Icon name={v.icon} /> <span>{v.label}</span>
                  </span>
                  <PrincipalBadge who={r.reviewer} />
                  <Show when={r.advisory}>
                    <span style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>advisory — never gates</span>
                  </Show>
                </li>
              );
            }}
          </For>
        </ul>
      </Show>

      {/* The batched review bar (G-8). */}
      <Show
        when={draft()}
        fallback={
          <button type="button" data-testid="start-review" onClick={() => void start()} class="btn-secondary" style={barBtn}>
            <Icon name="message" /> Start a review
          </button>
        }
      >
        <div data-testid="review-batch" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)", border: "var(--hairline) dashed var(--border)", "border-radius": "var(--radius-1)", padding: "var(--space-2)" }}>
            <span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", "font-size": "var(--fs-caption)", color: "var(--text-muted)" }}>
              <Icon name="edit" /> Review in progress · Pending · only you
            </span>
            <textarea
              aria-label="Pending review comment"
              value={pendingText()}
              onInput={(e) => setPendingText(e.currentTarget.value)}
              rows={2}
              placeholder="Add a comment to this review…"
              style={textareaStyle}
            />
            <div style={{ display: "flex", gap: "var(--space-2)", "flex-wrap": "wrap" }}>
              <button type="button" onClick={() => void addPending()} class="btn-secondary" style={barBtn}>Add comment</button>
              <button type="button" data-testid="open-verdict" onClick={() => setVerdictOpen(true)} class="btn-primary" style={barBtnPrimary}>
                Finish review…
              </button>
              <button type="button" onClick={() => void discard()} class="btn-secondary" style={barBtn}>Discard</button>
            </div>
            {/* The verdict submission (#21d: the sanctioned DS Dialog — focus move/trap, Esc + backdrop
                dismiss, return-focus, scroll-lock, APG role=dialog+aria-modal — replaces the former
                ad-hoc role="dialog" div that had none of those). */}
            <Dialog open={verdictOpen()} onClose={() => setVerdictOpen(false)} title="Submit review" size="sm">
              <div data-testid="verdict-panel" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
                <textarea onInput={(e) => setSummaryText(e.currentTarget.value)} aria-label="Review summary" rows={2} placeholder="Summary (optional)…" style={textareaStyle} />
                <div style={{ display: "flex", gap: "var(--space-2)", "flex-wrap": "wrap" }}>
                  <button type="button" data-testid="verdict-approve" onClick={() => void submit("approved")} class="btn-secondary" style={{ ...barBtn, color: "var(--success)" }}><Icon name="approve" /> Approve</button>
                  <button type="button" data-testid="verdict-changes" onClick={() => void submit("changes_requested")} class="btn-secondary" style={{ ...barBtn, color: "var(--danger)" }}><Icon name="reject" /> Request changes</button>
                  <button type="button" data-testid="verdict-comment" onClick={() => void submit("commented")} class="btn-secondary" style={barBtn}><Icon name="message" /> Comment</button>
                </div>
              </div>
            </Dialog>
          </div>
      </Show>
    </section>
  );
}

// ── commits ─────────────────────────────────────────────────────────────────────────────────────

function CommitsSection(props: { repo: string; n: number; initial: PrCommitsState }) {
  const [firstPage, setFirstPage] = createSignal<PrCommitsPage | null>(null);
  const [extraPages, setExtraPages] = createSignal<PrCommitsPage[]>([]);
  const [initialError, setInitialError] = createSignal(false);
  const [loadingMore, setLoadingMore] = createSignal(false);
  const [loadMoreError, setLoadMoreError] = createSignal(false);
  const [paginationCompleted, setPaginationCompleted] = createSignal(false);
  const [retryingInitial, setRetryingInitial] = createSignal(false);
  let completionStatus: HTMLParagraphElement | undefined;
  let generation = 0;
  let requestSequence = 0;
  let identity = "";
  let observedInitial: PrCommitsState;

  const resetContinuation = () => {
    generation += 1;
    requestSequence += 1;
    setExtraPages([]);
    setLoadingMore(false);
    setLoadMoreError(false);
    setPaginationCompleted(false);
  };

  createEffect(() => {
    const nextIdentity = `${props.repo}:${props.n}`;
    const initial = props.initial;
    if (identity !== nextIdentity) {
      identity = nextIdentity;
      observedInitial = undefined;
      setFirstPage(null);
      setInitialError(false);
      setRetryingInitial(false);
      resetContinuation();
    }
    if (!initial || initial.repo !== props.repo || initial.n !== props.n || initial === observedInitial) {
      return;
    }
    observedInitial = initial;
    setRetryingInitial(false);
    if (initial.page) {
      setFirstPage(initial.page);
      setInitialError(false);
      resetContinuation();
    } else {
      setInitialError(true);
    }
  });

  const items = createMemo(() => [
    ...(firstPage()?.items ?? []),
    ...extraPages().flatMap((page) => page.items),
  ]);
  const nextCursor = () => {
    const pages = extraPages();
    return pages.length > 0
      ? pages[pages.length - 1]!.page.next_cursor
      : firstPage()?.page.next_cursor ?? null;
  };
  const retryInitial = async () => {
    if (retryingInitial()) return;
    setRetryingInitial(true);
    try {
      await revalidate("git-pr-commits");
    } finally {
      setRetryingInitial(false);
    }
  };
  const loadMore = async (retry = false) => {
    const cursor = nextCursor();
    if (!cursor || loadingMore()) return;
    const requestGeneration = generation;
    const request = ++requestSequence;
    const requestRepo = props.repo;
    const requestNumber = props.n;
    const continuationInput = {
      repo: requestRepo,
      n: requestNumber,
      limit: PR_COMMITS_PAGE_LIMIT,
      cursor,
    };
    setLoadingMore(true);
    setLoadMoreError(false);
    try {
      if (retry) await revalidate(getPrCommits.keyFor(continuationInput));
      const page = await getPrCommits(continuationInput);
      if (requestGeneration !== generation || request !== requestSequence ||
          requestRepo !== props.repo || requestNumber !== props.n) return;

      const existingOids = new Set(items().map((item) => item.oid));
      const knownCursors = new Set([
        firstPage()?.page.next_cursor,
        ...extraPages().map((candidate) => candidate.page.next_cursor),
      ].filter((candidate): candidate is string => candidate !== null && candidate !== undefined));
      const duplicates = page.items.some((item) => existingOids.has(item.oid));
      const cursorCycle = page.page.next_cursor !== null && knownCursors.has(page.page.next_cursor);
      const emptyContinuation = page.items.length === 0 && page.page.next_cursor !== null;
      if (duplicates || cursorCycle || emptyContinuation ||
          page.page.limit !== PR_COMMITS_PAGE_LIMIT) {
        setLoadMoreError(true);
        return;
      }
      setExtraPages((pages) => [...pages, page]);
      if (page.page.next_cursor === null) {
        setPaginationCompleted(true);
        queueMicrotask(() => {
          if (requestGeneration === generation && request === requestSequence) {
            completionStatus?.focus();
          }
        });
      }
    } catch {
      if (requestGeneration === generation && request === requestSequence) {
        setLoadMoreError(true);
      }
    } finally {
      if (requestGeneration === generation && request === requestSequence) {
        setLoadingMore(false);
      }
    }
  };

  const loadingInitial = () => !firstPage() && !initialError() &&
    (!props.initial || props.initial.repo !== props.repo || props.initial.n !== props.n);
  return (
    <section aria-labelledby="commits-heading" aria-busy={loadingMore()} style={{ ...card }} data-testid="pr-commits-card">
      <h2 id="commits-heading" style={{ "font-size": "var(--fs-h3)", margin: "0" }}>Commits</h2>
      <Show when={!loadingInitial()} fallback={<Skeleton label="Loading commits…" rows={2} rowHeight="1.5rem" />}>
        <Show when={!initialError() || firstPage()} fallback={
          <div style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)", "align-items": "flex-start" }}>
            <p role="alert" style={{ color: "var(--danger)", margin: "0" }}>Commits could not be loaded. The pull request is still available.</p>
            <button type="button" class="btn-secondary" style={barBtn} disabled={retryingInitial()} onClick={() => void retryInitial()}>
              {retryingInitial() ? "Retrying commits…" : "Retry commits"}
            </button>
          </div>
        }>
          <Show when={items().length > 0} fallback={<p style={{ color: "var(--text-muted)", margin: "0" }}>No commits in this pull request.</p>}>
          <ul data-testid="pr-commits-list" style={{ "list-style": "none", margin: "0", padding: "0", display: "flex", "flex-direction": "column", gap: "var(--space-1)" }}>
            <For each={items()}>
              {(c) => (
                <li data-commit-oid={c.oid} style={{ display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
                  <Icon name="commit" />
                  <A href={`/git/repos/${props.repo}/commit/${c.oid}`} style={{ "font-family": "var(--font-mono)", "font-size": "var(--fs-caption)" }}>{c.short_oid}</A>
                  <span>{c.summary}</span>
                  <span style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>{c.author}</span>
                </li>
              )}
            </For>
          </ul>
          <Show when={initialError() && firstPage()}>
            <div style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)", "align-items": "flex-start" }}>
              <p role="alert" style={{ color: "var(--danger)", margin: "0" }}>Commits could not be refreshed. Already loaded commits are unchanged.</p>
              <button type="button" class="btn-secondary" style={barBtn} disabled={retryingInitial()} onClick={() => void retryInitial()}>
                {retryingInitial() ? "Retrying commits…" : "Retry commits"}
              </button>
            </div>
          </Show>
          <Show when={loadMoreError()}>
            <div style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)", "align-items": "flex-start" }}>
              <p role="alert" style={{ color: "var(--danger)", margin: "0" }}>Older commits could not be loaded. Already loaded commits are unchanged.</p>
              <button type="button" class="btn-secondary" style={barBtn} disabled={loadingMore()} onClick={() => void loadMore(true)}>
                Retry loading older commits
              </button>
            </div>
          </Show>
          <Show when={!loadMoreError() && nextCursor()}>
            <button type="button" data-testid="load-older-commits" class="btn-secondary" style={{ ...barBtn, "align-self": "flex-start" }} disabled={loadingMore()} onClick={() => void loadMore()}>
              {loadingMore() ? "Loading older commits…" : "Load older commits"}
            </button>
          </Show>
          <Show when={paginationCompleted()}>
            <p ref={completionStatus} tabindex={-1} role="status" aria-live="polite" data-testid="commits-pagination-complete" style={{ color: "var(--text-muted)", margin: "0" }}>
              All commits loaded.
            </p>
          </Show>
        </Show>
        </Show>
      </Show>
    </section>
  );
}

// ── discussion (threads with anchor null) ─────────────────────────────────────────────────────────

function DiscussionSection(props: {
  repo: string;
  n: number;
  threads: PrThreadVM[];
  loading: boolean;
  onChange: () => Promise<void>;
}) {
  const doMutate = useAction(prMutate);
  // CONTROLLED via a signal (finding #19): `value` is bound so a successful post that clears the signal
  // also clears the DOM — an uncontrolled field kept the stale text and a second submit duplicate-posted.
  // The text is preserved on failure (the signal keeps it, never lost).
  const [composer, setComposer] = createSignal("");
  const post = async () => {
    const text = composer().trim();
    if (!text) return;
    try {
      await doMutate({ op: "thread", repo: props.repo, n: props.n, body_md: text });
      setComposer("");
      await props.onChange();
    } catch {
      /* the composer text is preserved on failure (the ref keeps it, never lost) */
    }
  };
  return (
    <section id="discussion" aria-labelledby="discussion-heading" style={{ ...card }} data-testid="discussion">
      <h2 id="discussion-heading" style={{ "font-size": "var(--fs-h3)", margin: "0" }}>Discussion</h2>
      <Show when={!props.loading} fallback={<Skeleton label="Loading discussion…" rows={2} rowHeight="2rem" />}>
        <Show when={props.threads.length > 0} fallback={<p style={{ color: "var(--text-muted)", margin: "0" }}>No discussion yet.</p>}>
          <ul style={{ "list-style": "none", margin: "0", padding: "0", display: "flex", "flex-direction": "column", gap: "var(--space-3)" }}>
            <For each={props.threads}>
              {(t) => <ThreadView repo={props.repo} n={props.n} thread={t} onChange={props.onChange} />}
            </For>
          </ul>
        </Show>
      </Show>
      {/* The composer (a read-only viewer's write is server-rejected → the toast; the field never lies). */}
      <div style={{ display: "flex", "flex-direction": "column", gap: "var(--space-1)" }}>
        <textarea aria-label="New comment" value={composer()} onInput={(e) => setComposer(e.currentTarget.value)} rows={2} placeholder="Start a discussion…" style={textareaStyle} />
        <button type="button" data-testid="post-thread" disabled={!composer().trim()} onClick={() => void post()} class="btn-primary" style={{ ...barBtnPrimary, "align-self": "flex-start" }}>Comment</button>
      </div>
    </section>
  );
}

function ThreadView(props: { repo: string; n: number; thread: PrThreadVM; onChange: () => Promise<void> }) {
  const doMutate = useAction(prMutate);
  const [reply, setReply] = createSignal("");
  const send = async () => {
    const text = reply().trim();
    if (!text) return;
    try {
      await doMutate({ op: "comment", repo: props.repo, n: props.n, threadId: props.thread.id, body_md: text });
      setReply("");
      await props.onChange();
    } catch {
      /* keep the reply text */
    }
  };
  return (
    <li style={{ border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", padding: "var(--space-2)", display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
      <For each={props.thread.comments}>
        {(c) => (
          <div style={{ display: "flex", "flex-direction": "column", gap: "var(--space-1)" }}>
            <span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", "font-size": "var(--fs-caption)" }}>
              <PrincipalBadge who={c.author} />
              <Show when={c.pending}>
                <span style={{ color: "var(--text-subtle)" }}>· Pending · only you</span>
              </Show>
            </span>
            <Show when={c.state === "visible"} fallback={<span style={{ color: "var(--text-muted)", "font-style": "italic" }}>Comment removed</span>}>
              <Markdown source={c.body_md ?? ""} />
            </Show>
          </div>
        )}
      </For>
      <div style={{ display: "flex", gap: "var(--space-1)" }}>
        <input aria-label="Reply" value={reply()} onInput={(e) => setReply(e.currentTarget.value)} placeholder="Reply…" style={{ ...textareaStyle, flex: "1" }} />
        <button type="button" onClick={() => void send()} class="btn-secondary" style={barBtn}>Reply</button>
      </div>
    </li>
  );
}

// ── merge card (N6) ───────────────────────────────────────────────────────────────────────────────

function MergeCard(props: {
  repo: string;
  n: number;
  pr: PrVM;
  checks: PrChecksVM;
  onChange: () => Promise<void>;
  toast: ReturnType<typeof useToast>;
}) {
  const doMutate = useAction(prMutate);
  const [confirm, setConfirm] = createSignal(false);
  const [live, setLive] = createSignal<PrChecksVM>(untrack(() => props.checks));
  createEffect(() => setLive(props.checks));

  const blockedReasons = createMemo(() => {
    const c = live();
    const reasons: string[] = [];
    const greenSet = new Set(c.green_contexts);
    const forkSet = new Set(c.fork_unendorsed_contexts);
    for (const ctx of c.required_contexts) {
      if (forkSet.has(ctx)) reasons.push(`${ctx} awaiting fork trust`);
      else if (!greenSet.has(ctx)) reasons.push(`${ctx} not green`);
    }
    if (c.changes_requested) reasons.push("changes were requested");
    const have = c.current_approvals ?? 0;
    if (c.required_approvals > have) reasons.push(`${c.required_approvals - have} more approval(s) required`);
    return reasons;
  });

  const doMerge = async () => {
    try {
      const res = await doMutate({ op: "merge", repo: props.repo, n: props.n });
      const r = res as MergeResult;
      if (r.blocked) {
        // N6 — the gate flipped mid-dialog: re-render the blocked card from the FRESH checks, never merge.
        if (r.checks) setLive(r.checks);
        setConfirm(false);
        props.toast.show({ title: "Merge blocked — the gate changed. Showing the current state.", variant: "warning" });
        await props.onChange();
      } else {
        setConfirm(false);
        props.toast.show({ title: "Merged", variant: "success" });
        await props.onChange();
      }
    } catch {
      setConfirm(false);
      props.toast.show({ title: "Could not merge", variant: "danger" });
    }
  };

  const isTerminal = () => props.pr.pr_state === "merged" || props.pr.pr_state === "closed";

  return (
    <section aria-labelledby="merge-heading" style={{ ...card }} data-testid="merge-card">
      <h2 id="merge-heading" style={{ "font-size": "var(--fs-h3)", margin: "0" }}>Merge readiness</h2>
      <Show
        when={!isTerminal()}
        fallback={
          <span data-testid="merge-terminal" style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--text-muted)" }}>
            <Icon name={props.pr.pr_state === "merged" ? "merge" : "close"} /> This pull request is {props.pr.pr_state}.
          </span>
        }
      >
        <Show
          when={live().gate_admitted}
          fallback={
            <div data-testid="merge-blocked" style={{ color: "var(--warning)", display: "flex", "flex-direction": "column", gap: "var(--space-1)" }}>
              <span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)" }}><Icon name="gate" /> <strong>Blocked by branch protection</strong></span>
              <ul style={{ margin: "0", "padding-inline-start": "var(--space-4)" }}>
                <For each={blockedReasons()}>{(r) => <li>{r}</li>}</For>
              </ul>
            </div>
          }
        >
          <div style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
            <span data-testid="merge-ready" style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--success)" }}>
              <Icon name="check-pass" /> <strong>Ready to merge</strong> — all required checks satisfied.
            </span>
            <button type="button" data-testid="merge-button" onClick={() => setConfirm(true)} class="btn-primary" style={{ ...barBtnPrimary, "align-self": "flex-start" }}>
              <Icon name="merge" /> Merge pull request
            </button>
          </div>
        </Show>
      </Show>

      <ConfirmDialog
        open={confirm()}
        onCancel={() => setConfirm(false)}
        onConfirm={() => void doMerge()}
        title="Merge this pull request?"
        description={`This merges ${props.pr.head_ref} into ${props.pr.base_ref} with a merge commit. The branch protection gate is re-checked at merge time.`}
        confirmLabel="Merge"
        cancelLabel="Cancel"
      />
    </section>
  );
}

// ── shared bits ───────────────────────────────────────────────────────────────────────────────────

function PrincipalBadge(props: { who: PrincipalVM }) {
  const glyph = (): IconName => (props.who.kind === "agent" ? "agent" : props.who.kind === "service" ? "settings" : "human");
  return (
    <span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)" }}>
      <Icon name={glyph()} title={props.who.kind} />
      <span>{props.who.display}</span>
      <Show when={props.who.kind === "agent"}>
        <span style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>agent</span>
      </Show>
    </span>
  );
}

const textareaStyle = {
  width: "100%",
  padding: "var(--space-2)",
  border: "var(--hairline) solid var(--border)",
  "border-radius": "var(--radius-1)",
  background: "var(--surface)",
  color: "var(--text-primary)",
  "font-family": "inherit",
  "font-size": "var(--fs-body)",
  "box-sizing": "border-box",
} as const;

const barBtn = {
  display: "inline-flex",
  "align-items": "center",
  gap: "var(--space-1)",
  padding: "var(--space-1) var(--space-3)",
  border: "var(--hairline) solid var(--border)",
  "border-radius": "var(--radius-1)",
  background: "var(--surface-raised)",
  color: "var(--text-primary)",
  cursor: "pointer",
} as const;

const barBtnPrimary = {
  ...barBtn,
  // The primary CTA: accent as a LABELLED-button background (the §3.1-allowed accent use) with
  // on-accent text — AA-contrast by construction, unlike text-primary on --surface-hover.
  background: "var(--accent)",
  color: "var(--on-accent)",
  "border-color": "var(--accent)",
} as const;

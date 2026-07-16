// Commit log (R3.4 / G-3) — `/git/repos/{repo}/commits/{ref}`. The ref-carrying breadcrumb (no
// hardcoded 'main'), the paginated revwalk, and a bidirectional pager: Newer / Older with an honest
// position readout ("Commits 31–60 · page 2" — range + page, NO fabricated total). The position is an
// aria-live polite region so paging is announced. First-page Newer is present-disabled (the boundary
// is legible). Semantic tokens only.
import { ErrorBoundary, For, Show, Suspense } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, useParams, useSearchParams } from "@solidjs/router";
import { Icon, Skeleton } from "@myelin/design-system";
import { getCommits } from "~/lib/api";
import { fmtDate } from "~/lib/format";
import { RepoErrorState, errKind } from "~/components/RepoErrorState";
import { RepoBreadcrumb } from "~/components/RepoBreadcrumb";

export default function CommitLogScreen() {
  const params = useParams();
  const [search] = useSearchParams();
  const cursor = () => (typeof search.cursor === "string" ? search.cursor : undefined);
  const ready = () => Boolean(params.repo && params.ref);
  const commits = createAsync(async () =>
    ready() ? getCommits({ repo: params.repo!, ref: params.ref!, cursor: cursor() }) : undefined,
  );

  const linkTo = (c?: string | null) => {
    const base = `/git/repos/${params.repo}/commits/${encodeURIComponent(params.ref!)}`;
    return c ? `${base}?cursor=${encodeURIComponent(c)}` : base;
  };

  return (
    <section aria-labelledby="log-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-3)" }}>
      <Title>Commits · {params.repo} · Myelin</Title>
      <RepoBreadcrumb repo={params.repo!} refName={params.ref!} kind="tree" />
      <h1 id="log-heading" style={{ "font-size": "var(--fs-h1)", margin: "0", display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
        <Icon name="commit" /> Commits <span style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>on {params.ref}</span>
      </h1>

      <ErrorBoundary fallback={(err, reset) => <RepoErrorState kind={errKind(err)} repo={params.repo} onRetry={reset} />}>
        <Suspense fallback={<Skeleton label="Loading commits…" rows={6} rowHeight="3rem" data-testid="commits-loading" />}>
          <Show when={ready()} fallback={<RepoErrorState kind="not-found" repo={params.repo} />}>
            <Show when={commits()} keyed>
              {(page) => (
                <Show
                  when={page.items.length > 0}
                  fallback={<p data-testid="log-empty" style={{ color: "var(--text-muted)" }}>No commits on this ref yet.</p>}
                >
                  <ol data-testid="commit-log" style={{ "list-style": "none", margin: "0", padding: "0", display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
                    <For each={page.items}>
                      {(c) => (
                        <li style={{ border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", padding: "var(--space-3)", background: "var(--surface-raised)", display: "flex", "flex-direction": "column", gap: "var(--space-1)" }}>
                          <span style={{ display: "flex", "align-items": "center", gap: "var(--space-2)", "flex-wrap": "wrap" }}>
                            <A href={`/git/repos/${params.repo}/commit/${c.oid}?ref=${encodeURIComponent(params.ref!)}`} style={{ "font-family": "var(--font-mono)", color: "var(--text-primary)", "text-decoration": "underline" }}>{c.short_oid}</A>
                            <strong>{c.summary}</strong>
                            <Show when={c.parents.length > 1}>
                              <span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>
                                <Icon name="merge" /> merge
                              </span>
                            </Show>
                          </span>
                          <span style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>
                            {c.author} · {fmtDate(c.committed_at)}
                          </span>
                        </li>
                      )}
                    </For>
                  </ol>

                  {/* Bidirectional pager + honest position (range + page, no fabricated total). */}
                  <nav aria-label="Commit log pages" style={{ display: "flex", "align-items": "center", gap: "var(--space-3)", "flex-wrap": "wrap" }}>
                    <Show
                      when={page.page.prev_cursor != null}
                      fallback={
                        <span aria-disabled="true" style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--text-subtle)", padding: "var(--space-2) var(--space-3)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)" }}>
                          <Icon name="chevron" /> Newer
                        </span>
                      }
                    >
                      <A data-testid="pager-newer" href={linkTo(page.page.prev_cursor === "0" ? undefined : page.page.prev_cursor)} style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--text-primary)", padding: "var(--space-2) var(--space-3)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)" }}>
                        <Icon name="chevron" /> Newer
                      </A>
                    </Show>

                    <span data-testid="pager-position" aria-live="polite" style={{ color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>
                      <Show when={page.page.range}>
                        {(rg) => <>Commits {rg().from}&ndash;{rg().to}</>}
                      </Show>
                      {" · "}page {Math.floor((page.page.offset ?? 0) / (page.page.limit || 1)) + 1}
                    </span>

                    <Show when={page.page.next_cursor}>
                      {(next) => (
                        <A data-testid="pager-older" href={linkTo(next())} style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--text-primary)", padding: "var(--space-2) var(--space-3)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)" }}>
                        Older <Icon name="chevron" />
                        </A>
                      )}
                    </Show>
                  </nav>
                </Show>
              )}
            </Show>
          </Show>
        </Suspense>
      </ErrorBoundary>
    </section>
  );
}

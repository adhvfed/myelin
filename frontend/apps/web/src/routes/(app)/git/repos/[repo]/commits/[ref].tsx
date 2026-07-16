// Commit log (GT-004) — `/git/repos/{repo}/commits/{ref}`. Renders the edge's paginated commit log
// (libgit2 revwalk over the durable repo): one row per commit (short oid → diff link, summary, the
// PII-free pseudonymous author, the UTC time, a merge badge for >1 parent). Cursor pagination via the
// edge's `{items,page}` envelope. Semantic tokens only.
import { ErrorBoundary, For, Show, Suspense } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, useParams, useSearchParams } from "@solidjs/router";
import { Icon, Skeleton } from "@myelin/design-system";
import { getCommits } from "~/lib/api";
import { fmtDate } from "~/lib/format";
import { NotAvailable } from "~/components/NotAvailable";

export default function CommitLogScreen() {
  const params = useParams();
  const [search] = useSearchParams();
  const cursor = () => (typeof search.cursor === "string" ? search.cursor : undefined);
  // Guard the route segments: a deep-link missing {repo,ref} renders a dignified not-found.
  const ready = () => Boolean(params.repo && params.ref);
  const commits = createAsync(async () => {
    const repo = params.repo;
    const ref = params.ref;
    return repo && ref ? getCommits({ repo, ref, cursor: cursor() }) : undefined;
  });

  return (
    <section aria-labelledby="log-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-3)" }}>
      <Title>Commits · {params.repo} · Myelin</Title>
      <nav aria-label="Breadcrumb" style={{ "font-size": "var(--fs-caption)", display: "flex", gap: "var(--space-1)" }}>
        <A href="/git/repos" style={{ color: "var(--text-muted)" }}>Repositories</A>
        <span aria-hidden="true">/</span>
        <A href={`/git/repos/${params.repo}`} style={{ color: "var(--text-muted)" }}>{params.repo}</A>
      </nav>
      <h1 id="log-heading" style={{ "font-size": "var(--fs-h1)", margin: "0", display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
        <Icon name="commit" /> Commits <span style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>on {params.ref}</span>
      </h1>

      <ErrorBoundary
        fallback={(err) => (
          <p role="alert" style={{ color: "var(--danger)", border: "var(--hairline) solid var(--danger)", padding: "var(--space-3)", "border-radius": "var(--radius-1)" }}>
            <Icon name="check-fail" /> Could not load the commit log: {String(err.message ?? err)}
          </p>
        )}
      >
        <Suspense fallback={<Skeleton label="Loading commits…" rows={6} rowHeight="3rem" data-testid="commits-loading" />}>
          <Show when={ready()} fallback={<NotAvailable kind="commit log" />}>
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
                          {/* Short-oid link: --text-primary (AA-passing), never --accent-as-text.
                              accent lands at the AA floor in light and fails 4.5:1 as small mono text
                              on --surface-raised (DESIGN-MANUAL §3.1 carve-out). */}
                          <A href={`/git/repos/${params.repo}/commit/${c.oid}`} style={{ "font-family": "var(--font-mono)", color: "var(--text-primary)", "text-decoration": "underline" }}>{c.short_oid}</A>
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
                <Show when={page.page.next_cursor}>
                  {(next) => (
                    <A
                      href={`/git/repos/${params.repo}/commits/${params.ref}?cursor=${encodeURIComponent(next())}`}
                      style={{ "align-self": "flex-start", display: "inline-flex", "align-items": "center", gap: "var(--space-1)", padding: "var(--space-2) var(--space-3)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", color: "var(--text-primary)" }}
                    >
                      Older commits <Icon name="chevron" />
                    </A>
                  )}
                </Show>
              </Show>
            )}
          </Show>
          </Show>
        </Suspense>
      </ErrorBoundary>
    </section>
  );
}

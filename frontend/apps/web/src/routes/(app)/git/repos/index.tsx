// The Git repos list (GT-004), rendered from the edge's RepoHome ViewModel JSON. Proves the full path:
// shell → server-side gateway client → edge `/v1/git/repos?view=summary` → the bounded catalogue
// envelope → this Solid render. Each repo links to its separately fetched home screen. Unglamorous states are
// first-class (loading / error / empty-list / per-repo empty). Semantic tokens only.
import { ErrorBoundary, For, Match, Show, Suspense, Switch } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, useSearchParams } from "@solidjs/router";
import { Icon, Skeleton } from "@myelin/design-system";
import { getRepos, RepoRouteError, type RepoListRowVM } from "~/lib/api";
import { getViewer } from "~/lib/auth";
import { bareRepo } from "~/lib/format";
import { RepoErrorState, errKind } from "~/components/RepoErrorState";
import { ReposEmptyState } from "~/components/ReposEmptyState";
import { repoListHref, repoListInputFromSearch } from "~/lib/repo-list-state";

export default function ReposScreen() {
  const [search] = useSearchParams();
  const repos = createAsync(async () => {
    const input = repoListInputFromSearch(search.limit, search.cursor);
    if (!input) throw new RepoRouteError("error");
    return getRepos(input);
  }, { deferStream: true });
  const viewer = createAsync(() => getViewer());

  return (
    <section aria-labelledby="repos-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-4)" }}>
      <Title>Code · Myelin</Title>
      <div style={{ display: "flex", "align-items": "center", gap: "var(--space-3)", "flex-wrap": "wrap" }}>
        <h1 id="repos-heading" style={{ "font-size": "var(--fs-h1)", margin: "0" }}>
          Repositories
        </h1>
        <div style={{ flex: "1" }} />
        {/* R3.1 — the cross-repo "what needs me" front door (the front door for the review job). */}
        <A href="/prs" style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--text-primary)", "text-decoration": "none" }}>
          <Icon name="pull-request" /> Your pull requests
        </A>
      </div>

      <ErrorBoundary fallback={(err, reset) => <RepoErrorState kind={errKind(err)} onRetry={reset} />}>
        <Suspense fallback={<Skeleton label="Loading repositories…" rows={4} rowHeight="4rem" data-testid="repos-loading" />}>
          <Show when={repos()} keyed>
            {(page) => (
              <Show
                when={page.items.length > 0}
                fallback={<ReposEmptyState tenant={viewer()?.tenant ?? "your-org"} />}
              >
                <ul
                  data-testid="repos-list"
                  style={{ "list-style": "none", margin: "0", padding: "0", display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}
                >
                  <For each={page.items}>{(repo) => <RepoRow repo={repo} />}</For>
                </ul>
                <Show when={page.page.next_cursor}>
                  {(next) => (
                    <nav aria-label="Repository pages">
                      <A
                        data-testid="repos-next"
                        href={repoListHref({ limit: page.page.limit, cursor: next() })}
                        style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--text-primary)", padding: "var(--space-2) var(--space-3)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)" }}
                      >
                        Next <Icon name="chevron" />
                      </A>
                    </nav>
                  )}
                </Show>
              </Show>
            )}
          </Show>
        </Suspense>
      </ErrorBoundary>
    </section>
  );
}

function RepoRow(props: { repo: RepoListRowVM }) {
  const populated = () => props.repo.state === "populated" ? props.repo : undefined;
  const empty = () => props.repo.state === "empty" ? props.repo : undefined;
  return (
    <li
      style={{
        border: "var(--hairline) solid var(--border)",
        "border-radius": "var(--radius-1)",
        padding: "var(--space-3)",
        background: "var(--surface-raised)",
        display: "flex",
        "flex-direction": "column",
        gap: "var(--space-1)",
      }}
    >
      <Switch fallback={<span style={{ color: "var(--text-subtle)" }}>Restricted repository</span>}>
        <Match when={empty()} keyed>
          {(repo) => (
            <A
              href={`/git/repos/${bareRepo(repo.slug)}`}
              style={{ display: "flex", "align-items": "center", gap: "var(--space-2)", color: "var(--text-primary)" }}
            >
              <Icon name="repo" />
              <strong>{repo.slug}</strong>
              <span style={{ color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>empty · push to get started</span>
            </A>
          )}
        </Match>
        <Match when={populated()} keyed>
          {(repo) => (
            <>
              <A
                href={`/git/repos/${bareRepo(repo.slug)}`}
                style={{ display: "flex", "align-items": "center", gap: "var(--space-2)", color: "var(--text-primary)" }}
              >
                <Icon name="repo" />
                <strong>{repo.slug}</strong>
              </A>
              <code style={{ "font-family": "var(--font-mono)", color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>
                {repo.clone_url}
              </code>
            </>
          )}
        </Match>
      </Switch>
    </li>
  );
}

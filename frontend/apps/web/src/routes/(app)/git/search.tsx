import { ErrorBoundary, For, Show, Suspense } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, useSearchParams } from "@solidjs/router";
import { Icon, Skeleton } from "@myelin/design-system";
import { getCodeSearch, RepoRouteError } from "~/lib/api";
import {
  codeSearchHitHref,
  parseCodeSearchInput,
  type CodeSearchInput,
} from "~/lib/code-search";
import { RepoErrorState, errKind } from "~/components/RepoErrorState";

export default function CodeSearchScreen() {
  const [search] = useSearchParams();
  const input = (): CodeSearchInput | null => parseCodeSearchInput({
    q: typeof search.q === "string" ? search.q : "",
    ...(typeof search.repo === "string" && search.repo ? { repo: search.repo } : {}),
  });
  const results = createAsync(async () => {
    const request = input();
    return request ? getCodeSearch(request) : undefined;
  }, { deferStream: true });

  return (
    <section aria-labelledby="code-search-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-4)" }}>
      <Title>Search code · Myelin</Title>
      <nav aria-label="Breadcrumb" style={{ "font-size": "var(--fs-caption)", display: "flex", gap: "var(--space-1)" }}>
        <A href="/git/repos" style={{ color: "var(--text-muted)" }}>Code</A>
        <span aria-hidden="true">/</span>
        <span aria-current="page" style={{ color: "var(--text-muted)" }}>Search</span>
      </nav>
      <h1 id="code-search-heading" style={{ "font-size": "var(--fs-h1)", margin: "0", display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
        <Icon name="search" /> Search code
      </h1>

      <form method="get" action="/git/search" role="search" style={{ display: "flex", "flex-wrap": "wrap", gap: "var(--space-2)", "align-items": "end", "max-width": "56rem" }}>
        <label style={{ display: "flex", "flex-direction": "column", gap: "var(--space-1)", flex: "1 1 18rem" }}>
          <span>Search text</span>
          <input name="q" type="search" required maxLength={4096} value={typeof search.q === "string" ? search.q : ""} placeholder="Function, symbol, or exact text" />
        </label>
        <label style={{ display: "flex", "flex-direction": "column", gap: "var(--space-1)", flex: "1 1 14rem" }}>
          <span>Repository <span style={{ color: "var(--text-subtle)" }}>(optional)</span></span>
          <input name="repo" maxLength={255} value={typeof search.repo === "string" ? search.repo : ""} placeholder="All visible repositories" />
        </label>
        <button type="submit">Search</button>
      </form>

      <Show
        when={input()}
        fallback={<p style={{ color: "var(--text-muted)", margin: "0" }}>Search the default branch of repositories you can access.</p>}
      >
        <ErrorBoundary fallback={(error, reset) => <RepoErrorState kind={error instanceof RepoRouteError ? errKind(error) : "error"} onRetry={reset} />}>
          <Suspense fallback={<Skeleton label="Searching code…" rows={5} rowHeight="3.5rem" data-testid="code-search-loading" />}>
            <Show when={results()} keyed>
              {(page) => (
                <>
                  <Show
                    when={page.items.length > 0}
                    fallback={<p data-testid="code-search-empty" style={{ color: "var(--text-muted)", margin: "0" }}>No matches found.</p>}
                  >
                    <ul data-testid="code-search-results" style={{ "list-style": "none", margin: "0", padding: "0", display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
                      <For each={page.items}>
                        {(hit) => (
                          <li style={{ border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", background: "var(--surface-raised)" }}>
                            <A href={codeSearchHitHref(hit)} style={{ display: "flex", "flex-direction": "column", gap: "var(--space-1)", padding: "var(--space-3)", color: "inherit", "text-decoration": "none" }}>
                              <span style={{ display: "flex", "align-items": "center", gap: "var(--space-2)", "font-size": "var(--fs-body-sm)" }}>
                                <Icon name="file" />
                                <strong>{hit.repo}</strong>
                                <code style={{ color: "var(--text-muted)" }}>{hit.path}:{hit.line}</code>
                              </span>
                              <code style={{ "font-family": "var(--font-mono)", "white-space": "pre-wrap", "overflow-wrap": "anywhere", color: "var(--text-primary)" }}>{hit.excerpt}</code>
                              <span style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>{hit.ref}</span>
                            </A>
                          </li>
                        )}
                      </For>
                    </ul>
                  </Show>
                  <Show when={!page.complete}>
                    <p role="note" style={{ color: "var(--text-muted)", margin: "0" }}>
                      Results reached the current search limits. Narrow the text or repository to search more precisely.
                    </p>
                  </Show>
                </>
              )}
            </Show>
          </Suspense>
        </ErrorBoundary>
      </Show>
    </section>
  );
}

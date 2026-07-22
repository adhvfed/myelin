// Tree-at-path (R3.4 / G-2) — `/git/repos/{repo}/tree/{ref}/{...path}`. The `[...path]` splat matches
// the tree ROOT (empty) AND any nested dir. Renders the ref+every-segment breadcrumb, a parent-dir
// row, clickable dir/file rows (shared TreeList), and a subtree README. A file requested here (the
// edge's `redirect_to_blob` hint — kind mismatch) client-redirects to the blob route. The dignified
// error trio replaces raw err.message. Semantic tokens only; a11y per the manual.
import { ErrorBoundary, Show, Suspense, createEffect, createSignal, onCleanup } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, Navigate, createAsync, useNavigate, useParams, useSearchParams } from "@solidjs/router";
import { Skeleton } from "@myelin/design-system";
import { getTree } from "~/lib/api";
import { RepoErrorState, errKind } from "~/components/RepoErrorState";
import { RepoBreadcrumb } from "~/components/RepoBreadcrumb";
import { RefSwitcher } from "~/components/RefSwitcher";
import { Markdown } from "~/components/Markdown";
import { isFullGitRef } from "~/lib/git-read-input";
import {
  InitialTreeReader,
  TREE_SEARCH_DEBOUNCE_MS,
  treeCursorValue,
  treeHref,
  treeLimitValue,
  treeReloadHref,
  treeSearchValue,
} from "~/lib/tree-browse-state";
import { TreeList } from "../../index";

export default function TreeScreen() {
  const params = useParams();
  const [search] = useSearchParams();
  const navigate = useNavigate();
  const path = () => params.path ?? "";
  const query = () => treeSearchValue(search.q);
  const cursor = () => treeCursorValue(search.cursor);
  const limit = () => treeLimitValue(search.limit);
  const [searchDraft, setSearchDraft] = createSignal(query());
  const initialTreeReader = new InitialTreeReader(getTree);
  const ready = () => Boolean(params.repo && params.ref);
  const location = (overrides: { q?: string; cursor?: string } = {}) => treeHref({
    repo: params.repo!,
    ref: params.ref!,
    path: path(),
    limit: limit(),
    q: Object.hasOwn(overrides, "q") ? overrides.q : query(),
    cursor: overrides.cursor,
  });
  createEffect(() => setSearchDraft(query()));
  createEffect(() => {
    const draft = searchDraft();
    if (draft === query()) return;
    const timer = setTimeout(() => {
      navigate(location({ q: draft, cursor: undefined }));
    }, TREE_SEARCH_DEBOUNCE_MS);
    onCleanup(() => clearTimeout(timer));
  });
  const initialTree = createAsync(
    async () => ready()
      ? initialTreeReader.read({ repo: params.repo!, ref: params.ref!, path: path() })
      : undefined,
    { deferStream: true },
  );
  const tree = createAsync(
    async () =>
      ready()
        ? getTree({
            repo: params.repo!,
            ref: params.ref!,
            path: path(),
            limit: limit(),
            ...(cursor() ? { cursor: cursor() } : {}),
            ...(query() ? { q: query() } : {}),
          })
        : undefined,
    { deferStream: true },
  );

  const parentHref = () => {
    const segs = path().split("/").filter(Boolean);
    if (segs.length === 0) return undefined; // the root has no parent row
    const parent = segs.slice(0, -1).map(encodeURIComponent).join("/");
    return treeHref({ repo: params.repo!, ref: params.ref!, path: parent });
  };

  const blobHrefForFile = () =>
    `/git/repos/${params.repo}/blob/${encodeURIComponent(params.ref!)}/${path()
      .split("/")
      .map(encodeURIComponent)
      .join("/")}`;

  return (
    <section aria-labelledby="tree-title" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-4)" }}>
      <Title>{path() || params.ref} · {params.repo} · Myelin</Title>
      <div style={{ display: "flex", "align-items": "center", gap: "var(--space-3)", "flex-wrap": "wrap" }}>
        <RepoBreadcrumb repo={params.repo!} refName={params.ref!} path={path()} kind="tree" />
        <Show when={params.repo && params.ref}>
          <RefSwitcher
            repo={params.repo!}
            currentRef={params.ref!}
            currentFullRef={isFullGitRef(params.ref) ? params.ref : undefined}
            hrefFor={(ref) => treeHref({ repo: params.repo!, ref, path: path() })}
          />
        </Show>
      </div>
      <h1 id="tree-title" class="sr-only" style={{ position: "absolute", width: "1px", height: "1px", overflow: "hidden", clip: "rect(0 0 0 0)" }}>
        {params.repo} tree {path() ? `/ ${path()}` : ""} at {params.ref}
      </h1>

      <div class="ref-filter" style={{ "max-width": "24rem" }}>
        <span aria-hidden="true">⌕</span>
        <input
          type="search"
          class="ref-filter-input"
          aria-label="Search this directory by basename"
          placeholder="Search this directory…"
          maxLength={256}
          value={searchDraft()}
          onInput={(event) => setSearchDraft(event.currentTarget.value)}
        />
      </div>

      <ErrorBoundary fallback={(err, reset) => {
        const kind = errKind(err);
        return (
          <RepoErrorState
            kind={kind}
            repo={params.repo}
            onRetry={kind === "stale-tree"
              ? () => {
                  navigate(treeReloadHref({
                    repo: params.repo!,
                    ref: params.ref!,
                    path: path(),
                    limit: limit(),
                    q: query(),
                    cursor: cursor(),
                  }));
                  reset();
                }
              : reset}
          />
        );
      }}>
        <Suspense fallback={<Skeleton label="Loading directory…" rows={6} rowHeight="2rem" data-testid="tree-loading" />}>
          <Show when={ready()} fallback={<RepoErrorState kind="not-found" repo={params.repo} />}>
            <Show when={tree()} keyed>
              {(vm) => (
                <Show
                  when={!vm.redirect_to_blob}
                  fallback={<Navigate href={blobHrefForFile()} />}
                >
                  <Show
                    when={(vm.entries ?? []).length > 0}
                    fallback={<p data-testid="tree-empty" style={{ color: "var(--text-muted)" }}>This directory is empty.</p>}
                  >
                    <TreeList
                      repo={params.repo!}
                      refName={vm.ref ?? params.ref!}
                      path={path()}
                      entries={vm.entries ?? []}
                      heading={path() || `Files on ${params.ref}`}
                      parentHref={parentHref()}
                    />
                  </Show>
                  <p aria-live="polite" style={{ margin: "0", color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>
                    {query()
                      ? `${(vm.entries ?? []).length} matching ${(vm.entries ?? []).length === 1 ? "entry" : "entries"}`
                      : `${(vm.entries ?? []).length} ${(vm.entries ?? []).length === 1 ? "entry" : "entries"} on this page`}
                  </p>
                  <Show when={vm.page?.next_cursor}>
                    {(next) => (
                      <A
                        data-testid="tree-next-page"
                        href={location({ q: query(), cursor: next() })}
                        style={{ color: "var(--text-primary)", "align-self": "flex-start" }}
                      >
                        Next {vm.page?.limit ?? 100}
                      </A>
                    )}
                  </Show>
                  <Show when={initialTree()?.readme ?? vm.readme}>
                    {(readme) => (
                      <section aria-labelledby="tree-readme-heading">
                        <h2 id="tree-readme-heading" style={{ "font-size": "var(--fs-h3)", margin: "0 0 var(--space-2)" }}>README</h2>
                        <div style={{ border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", padding: "var(--space-3)", background: "var(--surface-raised)" }}>
                          <Markdown source={readme()} />
                        </div>
                      </section>
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

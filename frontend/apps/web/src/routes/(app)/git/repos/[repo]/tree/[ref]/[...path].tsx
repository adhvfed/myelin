// Tree-at-path (R3.4 / G-2) — `/git/repos/{repo}/tree/{ref}/{...path}`. The `[...path]` splat matches
// the tree ROOT (empty) AND any nested dir. Renders the ref+every-segment breadcrumb, a parent-dir
// row, clickable dir/file rows (shared TreeList), and a subtree README. A file requested here (the
// edge's `redirect_to_blob` hint — kind mismatch) client-redirects to the blob route. The dignified
// error trio replaces raw err.message. Semantic tokens only; a11y per the manual.
import { ErrorBoundary, Show, Suspense } from "solid-js";
import { Title } from "@solidjs/meta";
import { Navigate, createAsync, useParams } from "@solidjs/router";
import { Skeleton } from "@myelin/design-system";
import { getTree } from "~/lib/api";
import { RepoErrorState, errKind } from "~/components/RepoErrorState";
import { RepoBreadcrumb } from "~/components/RepoBreadcrumb";
import { RefSwitcher } from "~/components/RefSwitcher";
import { Markdown } from "~/components/Markdown";
import { TreeList } from "../../index";

export default function TreeScreen() {
  const params = useParams();
  const path = () => params.path ?? "";
  const ready = () => Boolean(params.repo && params.ref);
  const tree = createAsync(async () =>
    ready() ? getTree({ repo: params.repo!, ref: params.ref!, path: path() }) : undefined,
  );

  const parentHref = () => {
    const segs = path().split("/").filter(Boolean);
    if (segs.length === 0) return undefined; // the root has no parent row
    const parent = segs.slice(0, -1).map(encodeURIComponent).join("/");
    const base = `/git/repos/${params.repo}/tree/${encodeURIComponent(params.ref!)}`;
    return parent ? `${base}/${parent}` : base;
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
          <RefSwitcher repo={params.repo!} currentRef={params.ref!} hrefFor={(ref) => `/git/repos/${params.repo}/tree/${encodeURIComponent(ref)}/${path().split("/").map(encodeURIComponent).join("/")}`} />
        </Show>
      </div>
      <h1 id="tree-title" class="sr-only" style={{ position: "absolute", width: "1px", height: "1px", overflow: "hidden", clip: "rect(0 0 0 0)" }}>
        {params.repo} tree {path() ? `/ ${path()}` : ""} at {params.ref}
      </h1>

      <ErrorBoundary fallback={(err, reset) => <RepoErrorState kind={errKind(err)} repo={params.repo} onRetry={reset} />}>
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
                      refName={params.ref!}
                      path={path()}
                      entries={vm.entries ?? []}
                      heading={path() || `Files on ${params.ref}`}
                      parentHref={parentHref()}
                    />
                  </Show>
                  <Show when={vm.readme}>
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

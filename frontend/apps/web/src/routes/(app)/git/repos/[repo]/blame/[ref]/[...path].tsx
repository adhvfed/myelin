import { ErrorBoundary, Show, Suspense } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, useParams } from "@solidjs/router";
import { Icon, Skeleton, SkeletonBlock } from "@myelin/design-system";

import { BlameViewer } from "~/components/BlameViewer";
import { RepoBreadcrumb } from "~/components/RepoBreadcrumb";
import { RepoErrorState, errKind } from "~/components/RepoErrorState";
import { getBlame } from "~/lib/api";

export default function BlameScreen() {
  const params = useParams();
  const ref = () => {
    try {
      return decodeURIComponent(params.ref ?? "");
    } catch {
      return "";
    }
  };
  const path = () => params.path ?? "";
  const ready = () => Boolean(params.repo && ref() && path());
  const blame = createAsync(
    async () => ready() ? getBlame({ repo: params.repo!, ref: ref(), path: path() }) : undefined,
    { deferStream: true },
  );
  const blobHref = () =>
    `/git/repos/${params.repo}/blob/${encodeURIComponent(ref())}/${path().split("/").map(encodeURIComponent).join("/")}`;

  return (
    <section aria-labelledby="blame-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-3)" }}>
      <Title>Blame {path()} · {params.repo} · Myelin</Title>
      <RepoBreadcrumb repo={params.repo!} refName={ref()} path={path()} kind="blob" />

      <ErrorBoundary fallback={(error, reset) => <RepoErrorState kind={errKind(error)} repo={params.repo} onRetry={reset} />}>
        <Suspense
          fallback={
            <Skeleton label="Tracing line history…" data-testid="blame-loading">
              <SkeletonBlock height="2rem" width="24rem" />
              <SkeletonBlock height="18rem" style={{ "margin-block-start": "var(--space-3)" }} />
            </Skeleton>
          }
        >
          <Show when={ready()} fallback={<RepoErrorState kind="not-found" repo={params.repo} />}>
            <Show when={blame()} keyed>
              {(view) => (
                <>
                  <header style={{ display: "flex", "align-items": "center", gap: "var(--space-2)", "flex-wrap": "wrap" }}>
                    <h1 id="blame-heading" style={{ margin: "0", "font-size": "var(--fs-h3)", display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
                      <Icon name="human" /> Line history
                      <code style={{ "font-family": "var(--font-mono)" }}>{view.path}</code>
                    </h1>
                    <div style={{ flex: "1" }} />
                    <A href={blobHref()} style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", padding: "var(--space-1) var(--space-2)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", color: "var(--text-primary)", background: "var(--surface)" }}>
                      <Icon name="file" /> View file
                    </A>
                  </header>
                  <p style={{ margin: "0", color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>
                    Attribution is pinned to snapshot <code style={{ "font-family": "var(--font-mono)" }} title={view.snapshot_oid}>{view.snapshot_oid.slice(0, 12)}</code> on {view.ref}.
                  </p>
                  <BlameViewer repo={params.repo!} blame={view} />
                </>
              )}
            </Show>
          </Show>
        </Suspense>
      </ErrorBoundary>
    </section>
  );
}

// Blob view (R3.4 / G-2) — `/git/repos/{repo}/blob/{ref}/{...path}` (nested path). Full-path
// breadcrumb; a Raw + Download toolbar (gateway-proxied, in-region — Download forces an attachment via
// the /git-raw proxy) + a present-disabled Blame slot ("soon"). Body: a BINARY file renders the
// download fallback (NEVER split('\n') a binary into a garbled dump); a large file whose object was
// not inflated shows an explicit metadata-only fallback; otherwise the line-numbered code view. A directory requested
// here (the edge's redirect_to_tree hint) client-redirects to the tree route. Semantic tokens only.
import { ErrorBoundary, For, Show, Suspense } from "solid-js";
import { Title } from "@solidjs/meta";
import { Navigate, createAsync, useParams } from "@solidjs/router";
import { Icon, Skeleton, SkeletonBlock } from "@myelin/design-system";
import { getBlob } from "~/lib/api";
import { RepoErrorState, errKind } from "~/components/RepoErrorState";
import { RepoBreadcrumb } from "~/components/RepoBreadcrumb";

function fmtBytes(n?: number): string {
  if (!n && n !== 0) return "";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MiB`;
}

export default function BlobScreen() {
  const params = useParams();
  const path = () => params.path ?? "";
  const ready = () => Boolean(params.repo && params.ref && path());
  const blob = createAsync(
    async () =>
      ready() ? getBlob({ repo: params.repo!, ref: params.ref!, path: path() }) : undefined,
    { deferStream: true },
  );

  const encPath = () => path().split("/").map(encodeURIComponent).join("/");
  const rawHref = () => `/git-raw/${params.repo}/${encodeURIComponent(params.ref!)}/${encPath()}?d=inline`;
  const downloadHref = () => `/git-raw/${params.repo}/${encodeURIComponent(params.ref!)}/${encPath()}?d=attachment`;
  const treeHref = () => `/git/repos/${params.repo}/tree/${encodeURIComponent(params.ref!)}/${encPath()}`;

  const toolbarBtn = {
    display: "inline-flex", "align-items": "center", gap: "var(--space-1)",
    padding: "var(--space-1) var(--space-2)", border: "var(--hairline) solid var(--border)",
    "border-radius": "var(--radius-1)", color: "var(--text-primary)", background: "var(--surface)",
  } as const;

  return (
    <section aria-labelledby="blob-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-3)" }}>
      <Title>{path()} · {params.repo} · Myelin</Title>
      <RepoBreadcrumb repo={params.repo!} refName={params.ref!} path={path()} kind="blob" />

      <ErrorBoundary fallback={(err, reset) => <RepoErrorState kind={errKind(err)} repo={params.repo} onRetry={reset} />}>
        <Suspense
          fallback={
            <Skeleton label="Loading file…" data-testid="blob-loading">
              <SkeletonBlock height="1.5rem" width="18rem" />
              <SkeletonBlock height="14rem" style={{ "margin-block-start": "var(--space-3)" }} />
            </Skeleton>
          }
        >
          <Show when={ready()} fallback={<RepoErrorState kind="not-found" repo={params.repo} />}>
            <Show when={blob()} keyed>
              {(file) => (
                <Show when={!file.redirect_to_tree} fallback={<Navigate href={treeHref()} />}>
                  <div style={{ display: "flex", "align-items": "center", gap: "var(--space-2)", "flex-wrap": "wrap" }}>
                    <h1 id="blob-heading" style={{ "font-size": "var(--fs-h3)", margin: "0", display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
                      <Icon name="file" />
                      <code style={{ "font-family": "var(--font-mono)" }}>{file.path}</code>
                    </h1>
                    <span style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>{fmtBytes(file.size_bytes)}</span>
                    <div style={{ flex: "1" }} />
                    {/* Raw (open) · Download (attachment, gateway-proxied) · Blame (present-disabled "soon"). */}
                    <Show when={file.download_available !== false}>
                      <a href={rawHref()} target="_blank" rel="noreferrer" style={toolbarBtn}>
                        <Icon name="external-link" /> Raw
                      </a>
                      <a href={downloadHref()} style={toolbarBtn} data-testid="blob-download">
                        <Icon name="download" /> Download
                      </a>
                    </Show>
                    <button type="button" aria-disabled="true" disabled data-testid="blame-soon" title="Blame is coming soon" style={{ ...toolbarBtn, color: "var(--text-subtle)", cursor: "not-allowed" }}>
                      <Icon name="human" /> Blame
                      <span style={{ "font-size": "var(--fs-caption)" }}>soon</span>
                    </button>
                  </div>
                  <p style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)", margin: "0" }}>
                    blob <code style={{ "font-family": "var(--font-mono)" }}>{file.base_oid}</code>
                    {" · "}editing in the browser is GT-004b
                  </p>

                  <Show
                    when={!file.preview_unavailable}
                    fallback={
                      <div role="note" data-testid="blob-preview-unavailable" style={{ border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", padding: "var(--space-4)", background: "var(--surface-raised)", display: "flex", "flex-direction": "column", "align-items": "center", gap: "var(--space-2)" }}>
                        <Icon name="file" size={24} />
                        <p style={{ margin: "0", color: "var(--text-muted)" }}>
                          Preview not available for this large file ({fmtBytes(file.size_bytes)}).
                        </p>
                        <Show
                          when={file.download_available !== false}
                          fallback={<p style={{ margin: "0", color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>This file also exceeds the browser transfer limit. Fetch it through Git instead.</p>}
                        >
                          <a href={downloadHref()} style={{ ...toolbarBtn }}>
                            <Icon name="download" /> Download file
                          </a>
                        </Show>
                      </div>
                    }
                  >
                    <Show
                      when={!file.is_binary}
                      fallback={
                        <div role="note" data-testid="blob-binary" style={{ border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", padding: "var(--space-4)", background: "var(--surface-raised)", display: "flex", "flex-direction": "column", "align-items": "center", gap: "var(--space-2)" }}>
                          <Icon name="file" size={24} />
                          <p style={{ margin: "0", color: "var(--text-muted)" }}>Preview not available &mdash; binary file ({fmtBytes(file.size_bytes)}).</p>
                          <a href={downloadHref()} style={{ ...toolbarBtn }}>
                            <Icon name="download" /> Download file
                          </a>
                        </div>
                      }
                    >
                      <pre
                        data-testid="blob-contents"
                        aria-label="File contents"
                        style={{
                          border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)",
                          padding: "var(--space-3)", background: "var(--surface-raised)", margin: "0",
                          "font-family": "var(--font-mono)", "white-space": "pre-wrap", overflow: "auto",
                        }}
                      >
                        <For each={file.contents.split("\n")}>
                          {(line, i) => (
                            <div style={{ display: "flex", gap: "var(--space-2)" }}>
                              <span aria-hidden="true" style={{ color: "var(--text-subtle)", "min-width": "2.5rem", "text-align": "end", "user-select": "none" }}>{i() + 1}</span>
                              <span>{line}</span>
                            </div>
                          )}
                        </For>
                      </pre>
                    </Show>
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

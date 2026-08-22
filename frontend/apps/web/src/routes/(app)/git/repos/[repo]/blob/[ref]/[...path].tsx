// Blob preview with raw/download links and line attribution. Binary and oversized files use a
// download or metadata fallback; directory responses redirect to the tree route.
import { createSignal, ErrorBoundary, For, Show, Suspense } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, Navigate, createAsync, revalidate, useParams } from "@solidjs/router";
import { Icon, Skeleton, SkeletonBlock, useToast } from "@myelin/design-system";
import { getBlob } from "~/lib/api";
import { RepoErrorState, errKind } from "~/components/RepoErrorState";
import { RepoBreadcrumb } from "~/components/RepoBreadcrumb";
import { GitFileEditorDialog } from "~/components/repos/GitFileEditorDialog";
import { isEditableBranch } from "~/lib/git-file-edit-contract";
import { gitRepositoryPath, parseGitRepositoryRouteParam } from "~/lib/git-route";

function fmtBytes(n?: number): string {
  if (!n && n !== 0) return "";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MiB`;
}

export default function BlobScreen() {
  const params = useParams();
  const toast = useToast();
  const [editorOpen, setEditorOpen] = createSignal(false);
  const ref = () => {
    try {
      return decodeURIComponent(params.ref ?? "");
    } catch {
      return "";
    }
  };
  const path = () => params.path ?? "";
  const repo = () => parseGitRepositoryRouteParam(params.repo) ?? "";
  const repoPath = () => gitRepositoryPath(repo());
  const ready = () => Boolean(repo() && ref() && path());
  const blob = createAsync(
    async () =>
      ready() ? getBlob({ repo: repo(), ref: ref(), path: path() }) : undefined,
    { deferStream: true },
  );

  const encPath = () => path().split("/").map(encodeURIComponent).join("/");
  const rawHref = () => `/git-raw/${encodeURIComponent(repo())}/${encodeURIComponent(ref())}/${encPath()}?d=inline`;
  const downloadHref = () => `/git-raw/${encodeURIComponent(repo())}/${encodeURIComponent(ref())}/${encPath()}?d=attachment`;
  const treeHref = () => `${repoPath()}/tree/${encodeURIComponent(ref())}/${encPath()}`;
  const blameHref = () => `${repoPath()}/blame/${encodeURIComponent(ref())}/${encPath()}`;

  const toolbarBtn = {
    display: "inline-flex", "align-items": "center", gap: "var(--space-1)",
    padding: "var(--space-1) var(--space-2)", border: "var(--hairline) solid var(--border)",
    "border-radius": "var(--radius-1)", color: "var(--text-primary)", background: "var(--surface)",
  } as const;

  return (
    <section aria-labelledby="blob-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-3)" }}>
      <Title>{path()} · {repo()} · Myelin</Title>
      <RepoBreadcrumb repo={repo()} refName={ref()} path={path()} kind="blob" />

      <ErrorBoundary fallback={(err, reset) => <RepoErrorState kind={errKind(err)} repo={repo()} onRetry={reset} />}>
        <Suspense
          fallback={
            <Skeleton label="Loading file…" data-testid="blob-loading">
              <SkeletonBlock height="1.5rem" width="18rem" />
              <SkeletonBlock height="14rem" style={{ "margin-block-start": "var(--space-3)" }} />
            </Skeleton>
          }
        >
          <Show when={ready()} fallback={<RepoErrorState kind="not-found" repo={repo()} />}>
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
                    {/* Raw (open) · Download (attachment, gateway-proxied) · snapshot-pinned blame. */}
                    <Show when={file.download_available !== false}>
                      <a href={rawHref()} target="_blank" rel="noreferrer" style={toolbarBtn}>
                        <Icon name="external-link" /> Raw
                      </a>
                      <a href={downloadHref()} style={toolbarBtn} data-testid="blob-download">
                        <Icon name="download" /> Download
                      </a>
                    </Show>
                    <Show when={!file.preview_unavailable && !file.is_binary}>
                      <A href={blameHref()} data-testid="blame-link" style={toolbarBtn}>
                        <Icon name="human" /> Blame
                      </A>
                    </Show>
                    <Show when={file.viewer_may_edit && isEditableBranch(ref()) &&
                      !file.preview_unavailable && !file.is_binary && !file.is_truncated}>
                      <button type="button" class="repo-file-action" onClick={() => setEditorOpen(true)}>
                        <Icon name="file" /> Edit file
                      </button>
                    </Show>
                  </div>
                  <p style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)", margin: "0" }}>
                    blob <code style={{ "font-family": "var(--font-mono)" }}>{file.base_oid}</code>
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
                            <div id={`L${i() + 1}`} style={{ display: "flex", gap: "var(--space-2)", "scroll-margin-block-start": "var(--space-5)" }}>
                              <span aria-hidden="true" style={{ color: "var(--text-subtle)", "min-width": "2.5rem", "text-align": "end", "user-select": "none" }}>{i() + 1}</span>
                              <span>{line}</span>
                            </div>
                          )}
                        </For>
                      </pre>
                    </Show>
                  </Show>
                  <GitFileEditorDialog
                    open={editorOpen()}
                    mode="edit"
                    repo={repo()}
                    refName={ref()}
                    initialPath={file.path}
                    initialContents={file.contents}
                    initialBaseOid={file.base_oid}
                    onClose={() => setEditorOpen(false)}
                    onCommitted={(committed) => {
                      toast.show({ title: `${committed.path} committed`, variant: "success" });
                      void revalidate(getBlob.keyFor({ repo: repo(), ref: ref(), path: path() }));
                    }}
                  />
                </Show>
              )}
            </Show>
          </Show>
        </Suspense>
      </ErrorBoundary>
    </section>
  );
}

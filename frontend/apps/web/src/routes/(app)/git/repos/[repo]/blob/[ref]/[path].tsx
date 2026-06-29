// Blob view (GT-004) — `/git/repos/{repo}/blob/{ref}/{path}`. Renders the edge's WebEditForm ViewModel
// read-only: the file path + contents + the content-address (base_oid). The in-browser edit/commit
// composer (GF-6 single-file edit) is GT-004b. Single path segment (the gateway matches one segment
// per `{param}`); nested paths are a follow-on. Semantic tokens only.
import { ErrorBoundary, Show, Suspense, For } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, useParams } from "@solidjs/router";
import { Icon } from "@myelin/design-system";
import { getBlob } from "~/lib/api";
import { NotAvailable } from "~/components/NotAvailable";

export default function BlobScreen() {
  const params = useParams();
  // Guard the route segments: a deep-link missing any of {repo,ref,path} renders a dignified not-found.
  const ready = () => Boolean(params.repo && params.ref && params.path);
  const blob = createAsync(async () => {
    const repo = params.repo;
    const ref = params.ref;
    const path = params.path;
    return repo && ref && path ? getBlob({ repo, ref, path }) : undefined;
  });

  return (
    <section aria-labelledby="blob-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-3)" }}>
      <Title>{params.path} · {params.repo} · Myelin</Title>
      <nav aria-label="Breadcrumb" style={{ "font-size": "var(--fs-caption)", display: "flex", gap: "var(--space-1)" }}>
        <A href="/git/repos" style={{ color: "var(--text-muted)" }}>Repositories</A>
        <span aria-hidden="true">/</span>
        <A href={`/git/repos/${params.repo}`} style={{ color: "var(--text-muted)" }}>{params.repo}</A>
      </nav>

      <ErrorBoundary
        fallback={(err) => (
          <p role="alert" style={{ color: "var(--danger)", border: "var(--hairline) solid var(--danger)", padding: "var(--space-3)", "border-radius": "var(--radius-1)" }}>
            <Icon name="check-fail" /> Could not load this file: {String(err.message ?? err)}
          </p>
        )}
      >
        <Suspense fallback={<p style={{ color: "var(--text-muted)" }}>Loading file…</p>}>
          <Show when={ready()} fallback={<NotAvailable kind="file" />}>
          <Show when={blob()} keyed>
            {(file) => (
              <>
                <h1 id="blob-heading" style={{ "font-size": "var(--fs-h3)", margin: "0", display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
                  <Icon name="file" />
                  <code style={{ "font-family": "var(--font-mono)" }}>{file.path}</code>
                  <span style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>@ {params.ref}</span>
                </h1>
                <p style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)", margin: "0" }}>
                  blob <code style={{ "font-family": "var(--font-mono)" }}>{file.base_oid}</code>
                  {" · "}editing in the browser is GT-004b
                </p>
                <pre
                  data-testid="blob-contents"
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
              </>
            )}
          </Show>
          </Show>
        </Suspense>
      </ErrorBoundary>
    </section>
  );
}

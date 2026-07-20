// Commit diff (GT-004 · G-3) — `/git/repos/{repo}/commit/{oid}`. Renders the edge's CommitDiff
// ViewModel (libgit2 tree-to-tree diff over the durable repo) on the SHARED <DiffViewer> (R3.2 · G-7)
// — the bespoke FileDiff/DiffRow are retired (G-3 "commit detail = diff, reuses G-7"). The diff is
// a11y-accessible by construction in the viewer: change kind + line numbers are announced as TEXT,
// never colour alone (WCAG 1.4.1); the line grid is one tab stop. Semantic tokens only.
import { ErrorBoundary, Show, Suspense, createSignal } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, useParams, useSearchParams } from "@solidjs/router";
import { Skeleton, SkeletonBlock, DiffViewer, type DiffViewerFile } from "@myelin/design-system";
import { getCommit, type DiffFileVM } from "~/lib/api";
import { fmtDate } from "~/lib/format";
import { RepoErrorState, errKind } from "~/components/RepoErrorState";

/** The commit-diff VM carries flat `lines[]` (no hunks); wrap them in ONE synthetic hunk so the shared
 *  DiffViewer (hunk-structured) renders them. Line numbers are absent on this legacy shape — the viewer
 *  tolerates their absence (the gutters render empty, the SR prefix says "line ?"). */
function toViewerFile(f: DiffFileVM): DiffViewerFile {
  return {
    path: f.path,
    old_path: f.old_path,
    status: f.status,
    kind: "text",
    additions: f.lines.filter((l) => l.origin === "+").length,
    deletions: f.lines.filter((l) => l.origin === "-").length,
    hunks: [
      {
        header: "",
        old_start: 0,
        old_lines: 0,
        new_start: 0,
        new_lines: 0,
        lines: f.lines.map((l) => ({ origin: l.origin, content: l.content, old_no: l.old_no ?? null, new_no: l.new_no ?? null })),
      },
    ],
  };
}

export default function CommitDiffScreen() {
  const params = useParams();
  const [search] = useSearchParams();
  const [view, setView] = createSignal<"split" | "unified">("unified");
  // The commit diff KEEPS the arrival ref (finding 6: never reset the breadcrumb to a hardcoded 'main').
  const arrivalRef = () => (typeof search.ref === "string" && search.ref ? search.ref : "main");
  const ready = () => Boolean(params.repo && params.oid);
  const commit = createAsync(
    async () => {
      const repo = params.repo;
      const oid = params.oid;
      return repo && oid ? getCommit({ repo, oid }) : undefined;
    },
    { deferStream: true },
  );

  return (
    <section aria-labelledby="diff-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-3)" }}>
      <Title>{(params.oid ?? "commit").slice(0, 12)} · {params.repo} · Myelin</Title>
      <nav aria-label="Breadcrumb" style={{ "font-size": "var(--fs-caption)", display: "flex", gap: "var(--space-1)", "align-items": "center", "flex-wrap": "wrap" }}>
        <A href="/git/repos" style={{ color: "var(--text-muted)" }}>Repositories</A>
        <span aria-hidden="true">/</span>
        <A href={`/git/repos/${params.repo}`} style={{ color: "var(--text-muted)" }}>{params.repo}</A>
        <span aria-hidden="true">/</span>
        <A href={`/git/repos/${params.repo}/commits/${encodeURIComponent(arrivalRef())}`} style={{ color: "var(--text-muted)" }}>commits on {arrivalRef()}</A>
      </nav>

      <ErrorBoundary fallback={(err, reset) => <RepoErrorState kind={errKind(err)} repo={params.repo} onRetry={reset} />}>
        <Suspense
          fallback={
            <Skeleton label="Loading commit…" data-testid="commit-loading">
              <SkeletonBlock height="5rem" />
              <SkeletonBlock height="12rem" style={{ "margin-block-start": "var(--space-3)" }} />
            </Skeleton>
          }
        >
          <Show when={ready()} fallback={<RepoErrorState kind="not-found" repo={params.repo} />}>
            <Show when={commit()} keyed>
              {(c) => (
                <>
                  <header style={{ border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", padding: "var(--space-3)", background: "var(--surface-raised)", display: "flex", "flex-direction": "column", gap: "var(--space-1)" }}>
                    <h1 id="diff-heading" style={{ "font-size": "var(--fs-h3)", margin: "0" }}>{c.summary}</h1>
                    <span style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>
                      <code style={{ "font-family": "var(--font-mono)" }}>{c.short_oid}</code> · {c.author} · {fmtDate(c.committed_at)}
                    </span>
                    <Show when={c.message.trim() && c.message.trim() !== c.summary}>
                      <pre style={{ margin: "var(--space-2) 0 0", "white-space": "pre-wrap", "font-family": "var(--font-mono)", color: "var(--text-muted)" }}>{c.message}</pre>
                    </Show>
                  </header>

                  <Show
                    when={c.files.length > 0}
                    fallback={<p style={{ color: "var(--text-muted)" }} data-testid="diff-empty">This commit changed no files.</p>}
                  >
                    <div data-testid="diff-files">
                      <DiffViewer files={c.files.map(toViewerFile)} view={view()} onToggleView={setView} />
                    </div>
                  </Show>
                </>
              )}
            </Show>
          </Show>
        </Suspense>
      </ErrorBoundary>
    </section>
  );
}

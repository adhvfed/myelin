// Commit diff (GT-004) — `/git/repos/{repo}/commit/{oid}`. Renders the edge's CommitDiff ViewModel
// (libgit2 tree-to-tree diff over the durable repo): the commit header (short oid / summary / full
// message / pseudonymous author / UTC time) + per-file unified diffs. The diff is a11y-accessible: the
// change status and each line carry a GLYPH + TEXT (the `+`/`-`/` ` prefix), never colour alone (WCAG
// 1.4.1). Semantic tokens only.
import { ErrorBoundary, For, Show, Suspense } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, useParams } from "@solidjs/router";
import { Icon } from "@myelin/design-system";
import { getCommit, type DiffFileVM, type DiffLineVM } from "~/lib/api";
import { fmtDate } from "~/lib/format";
import { NotAvailable } from "~/components/NotAvailable";

const STATUS_LABEL: Record<string, string> = { A: "added", M: "modified", D: "deleted", R: "renamed", C: "copied" };

export default function CommitDiffScreen() {
  const params = useParams();
  // Guard the route segments: a deep-link missing {repo,oid} renders a dignified not-found.
  const ready = () => Boolean(params.repo && params.oid);
  const commit = createAsync(async () => {
    const repo = params.repo;
    const oid = params.oid;
    return repo && oid ? getCommit({ repo, oid }) : undefined;
  });

  return (
    <section aria-labelledby="diff-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-3)" }}>
      <Title>{(params.oid ?? "commit").slice(0, 12)} · {params.repo} · Myelin</Title>
      <nav aria-label="Breadcrumb" style={{ "font-size": "var(--fs-caption)", display: "flex", gap: "var(--space-1)" }}>
        <A href="/git/repos" style={{ color: "var(--text-muted)" }}>Repositories</A>
        <span aria-hidden="true">/</span>
        <A href={`/git/repos/${params.repo}`} style={{ color: "var(--text-muted)" }}>{params.repo}</A>
        <span aria-hidden="true">/</span>
        <A href={`/git/repos/${params.repo}/commits/main`} style={{ color: "var(--text-muted)" }}>commits</A>
      </nav>

      <ErrorBoundary
        fallback={(err) => (
          <p role="alert" style={{ color: "var(--danger)", border: "var(--hairline) solid var(--danger)", padding: "var(--space-3)", "border-radius": "var(--radius-1)" }}>
            <Icon name="check-fail" /> Could not load this commit: {String(err.message ?? err)}
          </p>
        )}
      >
        <Suspense fallback={<p style={{ color: "var(--text-muted)" }}>Loading commit…</p>}>
          <Show when={ready()} fallback={<NotAvailable kind="commit" />}>
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
                  <ul data-testid="diff-files" style={{ "list-style": "none", margin: "0", padding: "0", display: "flex", "flex-direction": "column", gap: "var(--space-3)" }}>
                    <For each={c.files}>{(f) => <FileDiff file={f} />}</For>
                  </ul>
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

function FileDiff(props: { file: DiffFileVM }) {
  return (
    <li style={{ border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", overflow: "hidden" }}>
      <h2 style={{ "font-size": "var(--fs-body)", margin: "0", padding: "var(--space-2) var(--space-3)", background: "var(--surface-overlay)", display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
        <span style={{ "font-size": "var(--fs-caption)", padding: "0 var(--space-1)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", color: "var(--text-muted)" }}>
          {STATUS_LABEL[props.file.status] ?? props.file.status}
        </span>
        <Show when={props.file.old_path}>
          {(old) => <code style={{ "font-family": "var(--font-mono)", color: "var(--text-subtle)" }}>{old()} →</code>}
        </Show>
        <code style={{ "font-family": "var(--font-mono)" }}>{props.file.path}</code>
      </h2>
      <table style={{ width: "100%", "border-collapse": "collapse", "font-family": "var(--font-mono)", "font-size": "var(--fs-body-sm)" }}>
        <tbody>
          <For each={props.file.lines}>{(line) => <DiffRow line={line} />}</For>
        </tbody>
      </table>
    </li>
  );
}

function DiffRow(props: { line: DiffLineVM }) {
  const color = () =>
    props.line.origin === "+" ? "var(--success)" : props.line.origin === "-" ? "var(--danger)" : "var(--text-primary)";
  const label = () => (props.line.origin === "+" ? "added line" : props.line.origin === "-" ? "removed line" : "context line");
  return (
    <tr style={{ color: color() }}>
      <td aria-hidden="true" style={{ width: "1.5rem", "text-align": "center", "user-select": "none", color: "var(--text-subtle)", "border-inline-end": "var(--hairline) solid var(--border)" }}>
        {props.line.origin === " " ? "" : props.line.origin}
      </td>
      <td style={{ padding: "0 var(--space-2)", "white-space": "pre-wrap" }}>
        <span class="sr-only">{label()}: </span>
        {props.line.content || " "}
      </td>
    </tr>
  );
}

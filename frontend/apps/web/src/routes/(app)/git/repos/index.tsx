// The Git repos list (GT-004), rendered from the edge's RepoHome ViewModel JSON. Proves the full path:
// shell → server-side gateway client → edge `/v1/git/repos` → the `{items,page}` envelope of RepoHome
// ViewModels → this Solid render. Each repo links to its home screen. Unglamorous states are
// first-class (loading / error / empty-list / per-repo empty). Semantic tokens only.
import { ErrorBoundary, For, Show, Suspense } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync } from "@solidjs/router";
import { Icon, Skeleton } from "@myelin/design-system";
import { getRepos, type RepoHomeVM } from "~/lib/api";
import { getViewer } from "~/lib/auth";
import { bareRepo } from "~/lib/format";
import { RepoErrorState, errKind } from "~/components/RepoErrorState";
import { ReposEmptyState } from "~/components/ReposEmptyState";

export default function ReposScreen() {
  const repos = createAsync(() => getRepos(), { deferStream: true });
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
              </Show>
            )}
          </Show>
        </Suspense>
      </ErrorBoundary>
    </section>
  );
}

function RepoRow(props: { repo: RepoHomeVM }) {
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
      <Show
        when={props.repo.state === "populated"}
        fallback={
          <Show
            when={props.repo.state === "empty"}
            fallback={<span style={{ color: "var(--text-subtle)" }}>Restricted repository</span>}
          >
            <A
              href={`/git/repos/${bareRepo(props.repo.slug)}`}
              style={{ display: "flex", "align-items": "center", gap: "var(--space-2)", color: "var(--text-primary)" }}
            >
              <Icon name="repo" />
              <strong>{props.repo.slug}</strong>
              <span style={{ color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>empty · push to get started</span>
            </A>
          </Show>
        }
      >
        <A
          href={`/git/repos/${bareRepo(props.repo.slug)}`}
          style={{ display: "flex", "align-items": "center", gap: "var(--space-2)", color: "var(--text-primary)" }}
        >
          <Icon name="repo" />
          <strong>{props.repo.slug}</strong>
        </A>
        <Show when={props.repo.readme_excerpt}>
          {(excerpt) => (
            <span style={{ color: "var(--text-muted)", "font-size": "var(--fs-body-sm)" }}>{excerpt()}</span>
          )}
        </Show>
        <span style={{ display: "flex", gap: "var(--space-3)", color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>
          <span>
            <Icon name="file" /> {props.repo.entries?.length ?? 0} entries
          </span>
          <code style={{ "font-family": "var(--font-mono)" }}>{props.repo.clone_url}</code>
        </span>
      </Show>
    </li>
  );
}

// THE ONE REAL EDGE-BACKED SCREEN (MR-019): the Git repos list, rendered from the edge's RepoHome
// ViewModel JSON. This proves the full path: shell → server-side gateway client (Bearer from the
// httpOnly-cookie session) → edge `/v1/git/repos` → the `{items,page}` envelope of RepoHome ViewModels
// → this Solid render. Unglamorous states are first-class (loading / error / empty-list / per-repo
// empty). Semantic tokens only.
import { ErrorBoundary, For, Show, Suspense } from "solid-js";
import { Title } from "@solidjs/meta";
import { createAsync } from "@solidjs/router";
import { Icon } from "@myelin/design-system";
import { getRepos, type RepoHomeVM } from "../../../lib/api";

export default function ReposScreen() {
  const repos = createAsync(() => getRepos());

  return (
    <section aria-labelledby="repos-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-4)" }}>
      <Title>Code · Myelin</Title>
      <h1 id="repos-heading" style={{ "font-size": "var(--fs-h1)", margin: "0" }}>
        Repositories
      </h1>

      <ErrorBoundary
        fallback={(err) => (
          <p role="alert" style={{ color: "var(--danger)", border: "var(--hairline) solid var(--danger)", padding: "var(--space-3)", "border-radius": "var(--radius-1)" }}>
            <Icon name="check-fail" /> Could not load repositories: {String(err.message ?? err)}
          </p>
        )}
      >
        <Suspense fallback={<p style={{ color: "var(--text-muted)" }}>Loading repositories…</p>}>
          <Show
            when={repos()}
            keyed
          >
            {(page) => (
              <Show
                when={page.items.length > 0}
                fallback={
                  <p style={{ color: "var(--text-muted)" }} data-testid="repos-empty">
                    No repositories in this tenant yet.
                  </p>
                }
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
            <span style={{ display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
              <Icon name="repo" />
              <strong>{props.repo.slug}</strong>
              <span style={{ color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>empty · push to get started</span>
            </span>
          </Show>
        }
      >
        <span style={{ display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
          <Icon name="repo" />
          <strong>{props.repo.slug}</strong>
        </span>
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

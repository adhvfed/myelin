// Repo home (GT-004) — `/git/repos/{repo}`. Renders the edge's RepoHome ViewModel: populated (clone
// URL + top-level tree + README excerpt) / empty (onboarding) / restricted (dignified tombstone), plus
// the unglamorous loading + error + not-found states. Files in the tree link to the blob view; a link
// to the commit log. Semantic tokens only; a11y per the design manual.
import { ErrorBoundary, For, Show, Suspense, Switch, Match } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, useParams } from "@solidjs/router";
import { Icon, useToast, Skeleton, SkeletonBlock } from "@myelin/design-system";
import { getRepo } from "~/lib/api";
import { NotAvailable } from "~/components/NotAvailable";

const card = {
  border: "var(--hairline) solid var(--border)",
  "border-radius": "var(--radius-1)",
  padding: "var(--space-3)",
  background: "var(--surface-raised)",
} as const;

export default function RepoHomeScreen() {
  const params = useParams();
  // Guard the route segment: a deep-link missing `{repo}` renders a dignified not-found, never a crash.
  const repo = createAsync(async () => {
    const slug = params.repo;
    return slug ? getRepo(slug) : undefined;
  });
  const toast = useToast();

  return (
    <section aria-labelledby="repo-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-4)" }}>
      <Title>{params.repo} · Code · Myelin</Title>
      <nav aria-label="Breadcrumb" style={{ "font-size": "var(--fs-caption)" }}>
        <A href="/git/repos" style={{ color: "var(--text-muted)" }}>Repositories</A>
      </nav>

      <ErrorBoundary
        fallback={(err) => (
          <p role="alert" style={{ color: "var(--danger)", border: "var(--hairline) solid var(--danger)", padding: "var(--space-3)", "border-radius": "var(--radius-1)" }}>
            <Icon name="check-fail" /> This repository is not available: {String(err.message ?? err)}
          </p>
        )}
      >
        <Suspense
          fallback={
            <Skeleton label="Loading repository…" data-testid="repo-loading">
              <SkeletonBlock height="var(--fs-h1)" width="14rem" />
              <SkeletonBlock height="2.5rem" width="20rem" style={{ "margin-block-start": "var(--space-3)" }} />
              <SkeletonBlock height="10rem" style={{ "margin-block-start": "var(--space-3)" }} />
            </Skeleton>
          }
        >
          <Show when={params.repo} fallback={<NotAvailable kind="repository" />}>
          <Show when={repo()} keyed>
            {(home) => (
              <Switch>
                <Match when={home.state === "restricted"}>
                  <div role="note" data-testid="repo-restricted" style={{ ...card, color: "var(--text-muted)", display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
                    <Icon name="gate" /> <span>This repository is not available to you.</span>
                  </div>
                </Match>

                <Match when={home.state === "empty"}>
                  <h1 id="repo-heading" style={{ "font-size": "var(--fs-h1)", margin: "0" }}>{home.slug}</h1>
                  <div data-testid="repo-empty" style={{ ...card, display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
                    <p style={{ margin: "0", color: "var(--text-muted)" }}>This repository has no commits yet.</p>
                    <CloneUrl url={home.clone_url} onCopy={() => toast.show({ title: "Clone URL copied", variant: "info" })} />
                    <pre style={{ ...card, "font-family": "var(--font-mono)", margin: "0", "white-space": "pre-wrap" }}>{`git clone ${home.clone_url}\ngit push -u origin main`}</pre>
                  </div>
                </Match>

                <Match when={home.state === "populated"}>
                  <h1 id="repo-heading" style={{ "font-size": "var(--fs-h1)", margin: "0" }}>{home.slug}</h1>
                  <div style={{ display: "flex", gap: "var(--space-3)", "align-items": "center", "flex-wrap": "wrap" }}>
                    <CloneUrl url={home.clone_url} onCopy={() => toast.show({ title: "Clone URL copied", variant: "info" })} />
                    <A href={`/git/repos/${params.repo}/commits/main`} style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--text-primary)" }}>
                      <Icon name="commit" /> Commits
                    </A>
                  </div>

                  <section aria-labelledby="tree-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
                    <h2 id="tree-heading" style={{ "font-size": "var(--fs-h3)", margin: "0" }}>Files <span style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>(on main)</span></h2>
                    <ul data-testid="repo-tree" style={{ ...card, "list-style": "none", margin: "0", padding: "var(--space-2)", display: "flex", "flex-direction": "column", gap: "var(--space-1)" }}>
                      <For each={home.entries ?? []}>
                        {(entry) => (
                          <li style={{ display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
                            <Show
                              when={!entry.is_dir}
                              fallback={
                                <span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-2)", color: "var(--text-muted)" }}>
                                  <Icon name="folder" title="Directory" />
                                  <code style={{ "font-family": "var(--font-mono)" }}>{entry.path}/</code>
                                </span>
                              }
                            >
                              <A href={`/git/repos/${params.repo}/blob/main/${encodeURIComponent(entry.path)}`} style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-2)", color: "var(--text-primary)" }}>
                                <Icon name="file" title="File" />
                                <code style={{ "font-family": "var(--font-mono)" }}>{entry.path}</code>
                              </A>
                            </Show>
                          </li>
                        )}
                      </For>
                    </ul>
                    <p style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)", margin: "0" }}>
                      Top-level tree. Browsing into directories is a follow-on (GT-004b).
                    </p>
                  </section>

                  <Show when={home.readme_excerpt}>
                    {(readme) => (
                      <section aria-labelledby="readme-heading">
                        <h2 id="readme-heading" style={{ "font-size": "var(--fs-h3)", margin: "0 0 var(--space-2)" }}>README</h2>
                        <pre style={{ ...card, margin: "0", "white-space": "pre-wrap", "font-family": "var(--font-mono)" }}>{readme()}</pre>
                      </section>
                    )}
                  </Show>
                </Match>
              </Switch>
            )}
          </Show>
          </Show>
        </Suspense>
      </ErrorBoundary>
    </section>
  );
}

function CloneUrl(props: { url?: string; onCopy: () => void }) {
  return (
    <span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-2)" }}>
      <code data-testid="clone-url" style={{ "font-family": "var(--font-mono)", color: "var(--text-muted)" }}>{props.url}</code>
      <button
        type="button"
        onClick={() => {
          if (props.url) void navigator.clipboard?.writeText(props.url).catch(() => {});
          props.onCopy();
        }}
        style={{
          display: "inline-flex", "align-items": "center", gap: "var(--space-1)",
          padding: "var(--space-1) var(--space-2)", border: "var(--hairline) solid var(--border)",
          "border-radius": "var(--radius-1)", background: "var(--surface)", color: "var(--text-primary)", cursor: "pointer",
        }}
      >
        <Icon name="link" /> Copy
      </button>
      <span style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>(clone over the wire is GT-006)</span>
    </span>
  );
}

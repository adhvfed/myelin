// Repository overview with refs, recent commit, root tree, and rendered README.
import { ErrorBoundary, For, Show, Suspense, Switch, Match } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, useParams } from "@solidjs/router";
import { Icon, useToast, Skeleton, SkeletonBlock } from "@myelin/design-system";
import { getRepo, type RepoEntry } from "~/lib/api";
import { fmtDate } from "~/lib/format";
import { RepoErrorState, errKind } from "~/components/RepoErrorState";
import { RefSwitcher } from "~/components/RefSwitcher";
import { Markdown } from "~/components/Markdown";
import { CloneUrl, GitSetupGuide } from "~/components/GitCloneSetup";
import { repoHomeContinuationHref } from "~/lib/tree-browse-state";
import { useAppViewer } from "~/components/AppShell";
import { CopyArtifactRef } from "~/components/CopyArtifactRef";
import { gitRepositoryPath, parseGitRepositoryRouteParam } from "~/lib/git-route";

const card = {
  border: "var(--hairline) solid var(--border)",
  "border-radius": "var(--radius-1)",
  padding: "var(--space-3)",
  background: "var(--surface-raised)",
} as const;

export default function RepoHomeScreen() {
  const params = useParams();
  const routeRepo = () => parseGitRepositoryRouteParam(params.repo) ?? "";
  const repoPath = () => gitRepositoryPath(routeRepo());
  const repo = createAsync(
    async () => (routeRepo() ? getRepo(routeRepo()) : undefined),
    { deferStream: true },
  );
  const viewer = useAppViewer();
  const toast = useToast();
  const defaultBranch = () => repo()?.default_branch ?? "main";

  return (
    <section aria-labelledby="repo-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-4)" }}>
      <Title>{routeRepo()} · Code · Myelin</Title>
      <nav aria-label="Breadcrumb" style={{ "font-size": "var(--fs-caption)" }}>
        <A href="/git/repos" style={{ color: "var(--text-primary)", "text-decoration": "underline" }}>Repositories</A>
        <span aria-hidden="true" style={{ color: "var(--text-subtle)", margin: "0 var(--space-1)" }}>/</span>
        <span style={{ color: "var(--text-primary)", "font-family": "var(--font-mono)" }}>{routeRepo()}</span>
      </nav>

      <ErrorBoundary
        fallback={(err, reset) => (
          <RepoErrorState kind={errKind(err)} repo={routeRepo()} onRetry={reset} />
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
          <Show when={routeRepo()} fallback={<RepoErrorState kind="not-found" />}>
            <Show when={repo()} keyed>
              {(home) => (
                <Switch>
                  <Match when={home.state === "empty" ? home : undefined} keyed>
                    {(empty) => (
                      <>
                        <div style={{ display: "flex", "align-items": "center", gap: "var(--space-3)", "flex-wrap": "wrap" }}>
                          <h1 id="repo-heading" style={{ "font-size": "var(--fs-h1)", margin: "0" }}>{empty.slug}</h1>
                          <CopyArtifactRef reference={empty.ref} />
                        </div>
                        <div data-testid="repo-empty" style={{ ...card, display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
                          <p style={{ margin: "0", color: "var(--text-muted)" }}>This repository has no commits yet.</p>
                          <CloneUrl url={empty.clone_url} onCopy={() => toast.show({ title: "Clone URL copied", variant: "info" })} />
                          <GitSetupGuide
                            url={empty.clone_url}
                            principalId={viewer.principalId}
                            tenant={viewer.tenant}
                            defaultBranch={defaultBranch()}
                          />
                        </div>
                      </>
                    )}
                  </Match>

                  <Match when={home.state === "populated" ? home : undefined} keyed>
                    {(populated) => (
                      <>
                    <header style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
                      <div style={{ display: "flex", "align-items": "center", gap: "var(--space-3)", "flex-wrap": "wrap" }}>
                        <h1 id="repo-heading" style={{ "font-size": "var(--fs-h1)", margin: "0" }}>{populated.slug}</h1>
                        <CopyArtifactRef reference={populated.ref} />
                        <span data-testid="repo-counts" style={{ color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>
                          {populated.counts.branches} {populated.counts.branches === 1 ? "branch" : "branches"} <span aria-hidden="true">·</span> {populated.counts.tags} {populated.counts.tags === 1 ? "tag" : "tags"}
                        </span>
                      </div>
                      <div style={{ display: "flex", gap: "var(--space-3)", "align-items": "center", "flex-wrap": "wrap" }}>
                        <RefSwitcher
                          repo={routeRepo()}
                          currentRef={defaultBranch()}
                          currentFullRef={`refs/heads/${defaultBranch()}`}
                          hrefFor={(ref) => `${repoPath()}/tree/${encodeURIComponent(ref)}`}
                        />
                        <CloneUrl url={populated.clone_url} onCopy={() => toast.show({ title: "Clone URL copied", variant: "info" })} />
                        <GitSetupGuide
                          url={populated.clone_url}
                          principalId={viewer.principalId}
                          tenant={viewer.tenant}
                          defaultBranch={defaultBranch()}
                        />
                        <A href={`${repoPath()}/commits/${encodeURIComponent(defaultBranch())}`} style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--text-primary)" }}>
                          <Icon name="commit" /> Commits
                        </A>
                        {/* R3.1 — the missing front door: mirror the Commits link (ux-git critical #1). */}
                        <A href={`${repoPath()}/prs`} style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--text-primary)" }}>
                          <Icon name="pull-request" /> Pull requests
                        </A>
                      </div>
                    </header>

                    {/* The latest-commit bar. */}
                    <Show when={populated.latest_commit}>
                      {(lc) => (
                        <div data-testid="latest-commit" style={{ ...card, display: "flex", "align-items": "center", gap: "var(--space-2)", "flex-wrap": "wrap" }}>
                          <Icon name="commit" />
                          <A href={`${repoPath()}/commit/${lc().oid ?? lc().short_oid}`} style={{ "font-family": "var(--font-mono)", color: "var(--text-primary)", "text-decoration": "underline" }}>{lc().short_oid}</A>
                          <strong style={{ flex: "1", "min-width": "10rem" }}>{lc().summary}</strong>
                          <span style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>{lc().author} · {fmtDate(lc().committed_at)}</span>
                        </div>
                      )}
                    </Show>

                    <TreeList
                      repo={routeRepo()}
                      refName={populated.entries_page.ref}
                      entries={populated.entries}
                      heading={`Files on ${defaultBranch()}`}
                    />
                    <Show when={repoHomeContinuationHref(routeRepo(), populated.entries_page)}>
                      {(href) => (
                        <A
                          data-testid="repo-tree-next-page"
                          href={href()}
                          style={{ color: "var(--text-primary)", "align-self": "flex-start" }}
                        >
                          Next {populated.entries_page.limit}
                        </A>
                      )}
                    </Show>

                    <Show when={populated.readme}>
                      {(readme) => (
                        <section aria-labelledby="readme-heading">
                          <h2 id="readme-heading" style={{ "font-size": "var(--fs-h3)", margin: "0 0 var(--space-2)" }}>README</h2>
                          <div style={{ ...card }}>
                            <Markdown source={readme()} />
                          </div>
                        </section>
                      )}
                    </Show>
                      </>
                    )}
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

// The clickable file tree (shared shape with the tree route). Dirs → tree route, files → blob route;
// each row shows the entry name + its latest-commit activity (when the bounded walk resolved it).
export function TreeList(props: {
  repo: string;
  refName: string;
  path?: string;
  entries: RepoEntry[];
  heading: string;
  parentHref?: string;
}) {
  const r = () => encodeURIComponent(props.refName);
  const repoPath = () => gitRepositoryPath(props.repo);
  const encPath = (p: string) => p.split("/").map(encodeURIComponent).join("/");
  return (
    <section aria-labelledby="tree-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
      <h2 id="tree-heading" style={{ "font-size": "var(--fs-h3)", margin: "0" }}>{props.heading}</h2>
      <ul data-testid="repo-tree" style={{ border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", background: "var(--surface-raised)", "list-style": "none", margin: "0", padding: "var(--space-2)", display: "flex", "flex-direction": "column", gap: "var(--space-1)" }}>
        <Show when={props.parentHref}>
          <li>
            <A href={props.parentHref!} aria-label="Up to parent directory" style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-2)", color: "var(--text-muted)" }}>
              <Icon name="chevron" title="Parent directory" />
              <code style={{ "font-family": "var(--font-mono)" }}>..</code>
            </A>
          </li>
        </Show>
        <For each={props.entries}>
          {(entry) => {
            return (
              <li style={{ display: "flex", "align-items": "center", gap: "var(--space-3)" }}>
                <Show
                  when={!entry.is_dir}
                  fallback={
                    <A href={`${repoPath()}/tree/${r()}/${encPath(entry.path)}`} style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-2)", color: "var(--text-primary)", flex: "1" }}>
                      <Icon name="folder" title="Folder" />
                      <code style={{ "font-family": "var(--font-mono)" }}>{entry.name}/</code>
                    </A>
                  }
                >
                  <A href={`${repoPath()}/blob/${r()}/${encPath(entry.path)}`} style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-2)", color: "var(--text-primary)", flex: "1" }}>
                    <Icon name="file" title="File" />
                    <code style={{ "font-family": "var(--font-mono)" }}>{entry.name}</code>
                  </A>
                </Show>
                <Show when={entry.latest_commit}>
                  {(lc) => (
                    <span style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)", "text-align": "end", "max-width": "16rem", overflow: "hidden", "text-overflow": "ellipsis", "white-space": "nowrap" }} title={lc().summary}>
                      {lc().summary} · {fmtDate(lc().committed_at)}
                    </span>
                  )}
                </Show>
              </li>
            );
          }}
        </For>
      </ul>
    </section>
  );
}

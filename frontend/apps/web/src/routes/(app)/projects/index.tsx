import { Title } from "@solidjs/meta";
import { createAsync, revalidate, useAction } from "@solidjs/router";
import { Icon, Skeleton, SkeletonBlock, useToast } from "@myelin/design-system";
import { For, Show, Suspense, createEffect, createMemo, createSignal } from "solid-js";
import { createProjectCatalogue } from "~/components/projects/project-catalogue";
import { getProjectAccess, setProjectAccess } from "~/lib/project-api";
import type { ProjectAccessPage, ProjectAccessVM } from "~/lib/project-contract";

function mergeAccess(first?: ProjectAccessPage, extra: ProjectAccessPage[] = []): ProjectAccessVM[] {
  return [...new Map(
    [first?.items ?? [], ...extra.map((page) => page.items)]
      .flat()
      .map((member) => [member.principal_id, member]),
  ).values()];
}

export default function ProjectsIndex() {
  const projects = createProjectCatalogue();
  const mutate = useAction(setProjectAccess);
  const toast = useToast();
  const [extraPages, setExtraPages] = createSignal<ProjectAccessPage[]>([]);
  const [loadingMore, setLoadingMore] = createSignal(false);
  const [loadMoreError, setLoadMoreError] = createSignal(false);
  const [changing, setChanging] = createSignal<string | null>(null);
  let generation = 0;

  const first = createAsync(async () => {
    const projectId = projects.selectedId();
    if (!projectId) return null;
    return getProjectAccess({ projectId, limit: 50 });
  }, { deferStream: true });
  const firstPage = () => {
    const result = first();
    return result?.ok ? result.page : undefined;
  };
  const members = createMemo(() => mergeAccess(firstPage(), extraPages()));
  const nextCursor = () => {
    const pages = extraPages();
    return pages.length
      ? pages[pages.length - 1]?.page.next_cursor ?? null
      : firstPage()?.page.next_cursor ?? null;
  };

  createEffect(() => {
    projects.selectedId();
    generation += 1;
    setExtraPages([]);
    setLoadingMore(false);
    setLoadMoreError(false);
    setChanging(null);
  });

  const loadMore = async () => {
    const projectId = projects.selectedId();
    const cursor = nextCursor();
    if (!projectId || !cursor || loadingMore()) return;
    const started = generation;
    setLoadingMore(true);
    setLoadMoreError(false);
    try {
      const result = await getProjectAccess({ projectId, cursor, limit: 50 });
      if (generation !== started) return;
      if (result.ok) setExtraPages((pages) => [...pages, result.page]);
      else setLoadMoreError(true);
    } catch {
      if (generation === started) setLoadMoreError(true);
    } finally {
      if (generation === started) setLoadingMore(false);
    }
  };

  const setAccess = async (member: ProjectAccessVM, access: boolean) => {
    const projectId = projects.selectedId();
    if (!projectId || changing()) return;
    setChanging(member.principal_id);
    try {
      const result = await mutate({
        projectId,
        principalId: member.principal_id,
        access,
        clientNonce: crypto.randomUUID(),
      });
      if (!result.ok) {
        toast.show({ title: "Project access could not be changed", variant: "warning" });
        return;
      }
      generation += 1;
      setExtraPages([]);
      await revalidate("project-access-list");
      if (!access && result.receipt.has_access) {
        toast.show({
          title: `${member.display_name} still has access through another project relationship`,
          variant: "info",
        });
      } else {
        toast.show({
          title: access
            ? `${member.display_name} can now collaborate in this project`
            : `${member.display_name}'s direct access was removed`,
          variant: "success",
        });
      }
    } catch {
      toast.show({ title: "Project access could not be confirmed", variant: "warning" });
    } finally {
      setChanging(null);
    }
  };

  return (
    <section aria-labelledby="projects-heading" class="issues-screen projects-screen">
      <Title>Projects · Myelin</Title>
      <header class="issues-heading-row">
        <div>
          <p class="issues-eyebrow">Organize shared work</p>
          <h1 id="projects-heading"><Icon name="team" /> Projects</h1>
        </div>
        <a class="issues-button issues-button-primary" href="/issues?new=1">New project or issue</a>
      </header>

      <Show when={projects.loading()}>
        <p role="status"><Icon name="cycle" /> Loading your projects…</p>
      </Show>
      <Show when={projects.unavailable()}>
        <div role="alert" class="issues-error">
          <Icon name="check-fail" />
          <div><h2>Projects are temporarily unavailable</h2><button class="issues-button issues-button-secondary" onClick={() => void projects.retry()}>Try again</button></div>
        </div>
      </Show>
      <Show when={projects.empty()}>
        <div class="issues-empty"><Icon name="team" /><h2>No projects yet</h2><p>Create one while capturing your first issue.</p><a class="issues-button issues-button-primary" href="/issues?new=1">Create a project</a></div>
      </Show>

      <Show when={projects.projects().length > 0}>
        <label class="issues-field-label projects-picker">
          Project
          <select
            class="issues-text-input issues-project-select"
            value={projects.selectedId()}
            onChange={(event) => projects.select(event.currentTarget.value)}
          >
            <For each={projects.projects()}>{(project) => (
              <option value={project.id}>{project.name} — {project.issue_prefix}</option>
            )}</For>
          </select>
        </label>
        <Show when={projects.nextCursor()}>
          <button class="issues-button issues-button-secondary projects-more" disabled={projects.loadingMore()} onClick={() => void projects.loadMore()}>
            {projects.loadingMore() ? "Loading projects…" : "Load more projects"}
          </button>
        </Show>

        <section aria-labelledby="project-access-heading" class="projects-access-card">
          <div>
            <p class="issues-eyebrow">Live authorization</p>
            <h2 id="project-access-heading">Who can collaborate</h2>
            <p>Direct readers can use this project across issues, chat, and agent review. Team and owner access stays governed by its original relationship.</p>
          </div>
          <Suspense fallback={<AccessSkeleton />}>
            <Show when={first()?.ok} fallback={
              <div role="alert" class="projects-access-state">
                <Icon name="check-fail" />
                <p>You need to be a project writer to manage access, or authorization is temporarily unavailable.</p>
              </div>
            }>
              <Show when={members().length > 0} fallback={
                <p class="projects-access-state">
                  {nextCursor()
                    ? "No active humans were found in this directory page. Continue to the next page."
                    : "No other active humans are in this organization yet."}
                </p>
              }>
                <ul class="projects-access-list">
                  <For each={members()}>{(member) => (
                    <li data-testid="project-access-member">
                      <div>
                        <strong>{member.display_name}</strong>
                        <code>{member.principal_id}</code>
                      </div>
                      <Show
                        when={!member.has_access || member.removable}
                        fallback={<span class="projects-access-inherited">Access via owner or team</span>}
                      >
                        <button
                          type="button"
                          class={`issues-button ${member.removable ? "issues-button-danger" : "issues-button-secondary"}`}
                          disabled={changing() !== null}
                          onClick={() => void setAccess(member, !member.has_access)}
                        >
                          {changing() === member.principal_id
                            ? "Saving…"
                            : member.removable ? "Remove direct access" : "Give access"}
                        </button>
                      </Show>
                    </li>
                  )}</For>
                </ul>
              </Show>
              <Show when={nextCursor()}>
                <div class="issues-load-more">
                  <button class="issues-button issues-button-secondary" disabled={loadingMore()} onClick={() => void loadMore()}>{loadingMore() ? "Loading…" : "Load more people"}</button>
                  <Show when={loadMoreError()}><span role="alert">More people could not be loaded. Try again.</span></Show>
                </div>
              </Show>
            </Show>
          </Suspense>
        </section>
      </Show>
    </section>
  );
}

function AccessSkeleton() {
  return (
    <Skeleton label="Loading project access…" rows={3} rowHeight="3.5rem">
      <For each={[0, 1, 2]}>{() => <div class="projects-access-row"><SkeletonBlock width="12rem" /><SkeletonBlock width="8rem" /></div>}</For>
    </Skeleton>
  );
}

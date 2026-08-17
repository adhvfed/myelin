import { createAsync, revalidate } from "@solidjs/router";
import { createEffect, createMemo, createSignal } from "solid-js";
import { getProjects } from "~/lib/project-api";
import type { ProjectPage, ProjectVM } from "~/lib/project-contract";

function mergeProjects(created: ProjectVM[], first?: ProjectPage, extra: ProjectPage[] = []): ProjectVM[] {
  const rows = [created, first?.items ?? [], ...extra.map((page) => page.items)].flat();
  return [...new Map(rows.map((project) => [project.id, project])).values()];
}

/** Reactive, authorized project choices shared by project-scoped mutation dialogs. */
export function createProjectCatalogue() {
  const initial = createAsync(() => getProjects({ limit: 50 }), { deferStream: true });
  const [extraPages, setExtraPages] = createSignal<ProjectPage[]>([]);
  const [created, setCreated] = createSignal<ProjectVM[]>([]);
  const [selectedId, setSelectedId] = createSignal("");
  const [loadingMore, setLoadingMore] = createSignal(false);
  const [loadMoreError, setLoadMoreError] = createSignal(false);
  const [retrying, setRetrying] = createSignal(false);
  let generation = 0;
  let continuationRequest = 0;

  const firstPage = () => {
    const result = initial();
    return result?.ok ? result.page : undefined;
  };
  const projects = createMemo(() => mergeProjects(created(), firstPage(), extraPages()));
  const loading = () => initial() === undefined || retrying();
  const unavailable = () => !retrying() && initial()?.ok === false;
  const empty = () => !loading() && !unavailable() && projects().length === 0;
  const selected = () => projects().find((project) => project.id === selectedId());
  const nextCursor = () => {
    const extra = extraPages();
    return extra.length
      ? extra[extra.length - 1]?.page.next_cursor ?? null
      : firstPage()?.page.next_cursor ?? null;
  };

  createEffect(() => {
    const rows = projects();
    if (rows.length > 0 && !rows.some((project) => project.id === selectedId())) {
      setSelectedId(rows[0]!.id);
    }
  });

  const restartContinuations = () => {
    generation += 1;
    continuationRequest += 1;
    setExtraPages([]);
    setLoadingMore(false);
    setLoadMoreError(false);
  };

  const add = (project: ProjectVM) => {
    restartContinuations();
    setCreated((rows) => [project, ...rows]);
    setSelectedId(project.id);
    void revalidate("projects-list");
  };

  const loadMore = async () => {
    const cursor = nextCursor();
    if (!cursor || loadingMore()) return;
    const startedInGeneration = generation;
    const request = ++continuationRequest;
    setLoadingMore(true);
    setLoadMoreError(false);
    try {
      const result = await getProjects({ cursor, limit: 50 });
      if (startedInGeneration !== generation || request !== continuationRequest) return;
      if (result.ok) setExtraPages((pages) => [...pages, result.page]);
      else setLoadMoreError(true);
    } catch {
      if (startedInGeneration === generation && request === continuationRequest) {
        setLoadMoreError(true);
      }
    } finally {
      if (startedInGeneration === generation && request === continuationRequest) {
        setLoadingMore(false);
      }
    }
  };

  const retry = async () => {
    restartContinuations();
    setRetrying(true);
    try {
      await revalidate("projects-list");
    } finally {
      setRetrying(false);
    }
  };

  return {
    projects,
    loading,
    unavailable,
    empty,
    selected,
    selectedId,
    select: setSelectedId,
    nextCursor,
    loadingMore,
    loadMoreError,
    add,
    loadMore,
    retry,
  };
}

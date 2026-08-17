import { Icon, Skeleton, SkeletonBlock, useToast } from "@myelin/design-system";
import { Title } from "@solidjs/meta";
import { createAsync, revalidate, useNavigate, useSearchParams } from "@solidjs/router";
import { createMemo, createSignal, onMount, Show } from "solid-js";
import { KnowledgeCreateDialog } from "~/components/knowledge/KnowledgeCreateDialog";
import { KnowledgeSidebar, knowledgeHref } from "~/components/knowledge/KnowledgeSidebar";
import { KnowledgeWorkspace } from "~/components/knowledge/KnowledgeWorkspace";
import { getKnowledgePage, getKnowledgePages, isKnowledgeUlid, KnowledgeRouteError, type KnowledgeErrorKind, type KnowledgePageList } from "~/lib/knowledge-api";

interface PageResult<T> { value: T | null; error: KnowledgeErrorKind | null }
function kind(error: unknown): KnowledgeErrorKind { return error instanceof KnowledgeRouteError ? error.kind : "error"; }
function mergePages(first: KnowledgePageList | undefined, continuations: KnowledgePageList[]) {
  const pages = [first?.items ?? [], ...continuations.map((page) => page.items)].flat();
  return [...new Map(pages.map((page) => [page.id, page])).values()];
}

export default function KnowledgeIndex() {
  const [search] = useSearchParams();
  const navigate = useNavigate();
  const toast = useToast();
  const requestedPage = () => typeof search.page === "string" ? search.page : undefined;
  const selected = () => {
    const page = requestedPage();
    return page && isKnowledgeUlid(page) ? page : undefined;
  };
  const createOpen = () => search.new === "1";
  const [interactive, setInteractive] = createSignal(false);
  const [more, setMore] = createSignal<KnowledgePageList[]>([]);
  const [loadingMore, setLoadingMore] = createSignal(false);
  let pageListGeneration = 0;

  const first = createAsync(async (): Promise<PageResult<KnowledgePageList>> => {
    try { return { value: await getKnowledgePages({ limit: 100 }), error: null }; }
    catch (error) { if (error instanceof Response) throw error; return { value: null, error: kind(error) }; }
  }, { deferStream: true });
  const document = createAsync(async () => {
    const id = requestedPage();
    if (!id) return null;
    if (!isKnowledgeUlid(id)) {
      return { value: null, error: "bad-input" as KnowledgeErrorKind };
    }
    try { return { value: await getKnowledgePage(id), error: null as KnowledgeErrorKind | null }; }
    catch (error) { if (error instanceof Response) throw error; return { value: null, error: kind(error) }; }
  }, { deferStream: true });

  const pages = createMemo(() => mergePages(first()?.value ?? undefined, more()));
  const nextCursor = () => more().at(-1)?.page.next_cursor ?? first()?.value?.page.next_cursor ?? null;
  onMount(() => setInteractive(true));

  const loadMore = async () => {
    const cursor = nextCursor(); if (!cursor || loadingMore()) return;
    const generation = pageListGeneration;
    setLoadingMore(true);
    try {
      const page = await getKnowledgePages({ cursor, limit: 100 });
      if (generation === pageListGeneration) setMore((current) => [...current, page]);
    } catch {
      if (generation === pageListGeneration) {
        toast.show({ title: "More pages couldn’t be loaded", variant: "danger" });
      }
    } finally {
      if (generation === pageListGeneration) setLoadingMore(false);
    }
  };
  const restartPageList = () => {
    pageListGeneration += 1;
    setMore([]);
    setLoadingMore(false);
    void revalidate("knowledge-pages");
  };
  const openCreate = () => navigate(selected() ? `${knowledgeHref(selected())}&new=1` : "/knowledge?new=1");
  const closeCreate = () => navigate(knowledgeHref(selected()), { replace: true });
  const reloadPage = async (pageId: string) => {
    try {
      await revalidate(getKnowledgePage.keyFor(pageId));
      return await getKnowledgePage(pageId);
    } catch {
      return undefined;
    }
  };
  const pageSaved = async (page: { id: string }) => {
    try {
      await Promise.all([
        revalidate("knowledge-pages"),
        revalidate(getKnowledgePage.keyFor(page.id)),
      ]);
      if (selected() === page.id) toast.show({ title: "Page saved", variant: "success" });
    } catch {
      if (selected() === page.id) {
        toast.show({ title: "Page saved — reload to refresh its details", variant: "warning" });
      }
    }
  };

  return <>
    <Title>Knowledge · Myelin</Title>
    <div class="knowledge-screen" classList={{ "knowledge-has-selection": Boolean(requestedPage()) }} data-testid="knowledge-screen">
      <KnowledgeSidebar pages={pages()} selectedId={selected()} loading={first() === undefined} error={first()?.error} interactive={interactive()} hasMore={Boolean(nextCursor())} loadingMore={loadingMore()} onLoadMore={() => void loadMore()} onNew={openCreate} />
      <section class="knowledge-workspace-shell" aria-label="Knowledge page">
        <Show when={requestedPage()} fallback={<KnowledgeWelcome hasPages={pages().length > 0} interactive={interactive()} onNew={openCreate} />}>
          <Show when={document() !== undefined} fallback={<KnowledgeLoading />}>
            <Show when={document()?.value} fallback={<KnowledgeError kind={document()?.error ?? "error"} />}>
              {(page) => <KnowledgeWorkspace page={page()} onReload={reloadPage} onSaved={pageSaved} />}
            </Show>
          </Show>
        </Show>
      </section>
    </div>
    <KnowledgeCreateDialog open={createOpen()} onClose={closeCreate} onCreated={(receipt) => { restartPageList(); navigate(knowledgeHref(receipt.page.id)); toast.show({ title: receipt.created ? "Page created" : "Page already existed", variant: "success" }); }} />
  </>;
}

function KnowledgeWelcome(props: { hasPages: boolean; interactive: boolean; onNew: () => void }) {
  return <div class="knowledge-welcome"><Icon name="nav-knowledge" size={34} /><p class="knowledge-eyebrow">A shared memory for engineering</p><h2>{props.hasPages ? "Choose a page" : "Turn decisions into organisational memory"}</h2><p>Write specs, runbooks, architecture decisions, and operating context in the same place your code, issues, CI, and conversations live.</p><button type="button" class="knowledge-button primary" disabled={!props.interactive} onClick={() => props.onNew()}><Icon name="doc" /> New page</button></div>;
}

function KnowledgeLoading() { return <div class="knowledge-loading"><Skeleton label="Loading page…" rows={4}><SkeletonBlock height="3rem" /><SkeletonBlock height="2rem" /><SkeletonBlock height="5rem" /><SkeletonBlock height="5rem" /></Skeleton></div>; }
function KnowledgeError(props: { kind: KnowledgeErrorKind }) {
  const heading = () => props.kind === "bad-input"
    ? "Page address invalid"
    : props.kind === "not-found" ? "Page not found" : "Page unavailable";
  const copy = () => props.kind === "bad-input"
    ? "This link doesn’t contain a valid Myelin page address."
    : props.kind === "not-found"
      ? "It may have been removed, made private, or the link may be wrong."
      : "Knowledge couldn’t load this page. Refresh to try again.";
  return <div class="knowledge-welcome" role="alert"><Icon name="gate" size={28} /><h2>{heading()}</h2><p>{copy()}</p><a class="knowledge-button secondary" href="/knowledge">Back to pages</a></div>;
}

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

export default function KnowledgeIndex() {
  const [search] = useSearchParams();
  const navigate = useNavigate();
  const toast = useToast();
  const selected = () => typeof search.page === "string" && isKnowledgeUlid(search.page) ? search.page : undefined;
  const createOpen = () => search.new === "1";
  const [interactive, setInteractive] = createSignal(false);
  const [more, setMore] = createSignal<KnowledgePageList[]>([]);
  const [loadingMore, setLoadingMore] = createSignal(false);

  const first = createAsync(async (): Promise<PageResult<KnowledgePageList>> => {
    try { return { value: await getKnowledgePages({ limit: 100 }), error: null }; }
    catch (error) { if (error instanceof Response) throw error; return { value: null, error: kind(error) }; }
  }, { deferStream: true });
  const document = createAsync(async () => {
    const id = selected();
    if (!id) return null;
    try { return { value: await getKnowledgePage(id), error: null as KnowledgeErrorKind | null }; }
    catch (error) { if (error instanceof Response) throw error; return { value: null, error: kind(error) }; }
  }, { deferStream: true });

  const pages = createMemo(() => [...(first()?.value?.items ?? []), ...more().flatMap((page) => page.items)]);
  const nextCursor = () => more().at(-1)?.page.next_cursor ?? first()?.value?.page.next_cursor ?? null;
  onMount(() => setInteractive(true));

  const loadMore = async () => {
    const cursor = nextCursor(); if (!cursor || loadingMore()) return;
    setLoadingMore(true);
    try { const page = await getKnowledgePages({ cursor, limit: 100 }); setMore((current) => [...current, page]); }
    catch { toast.show({ title: "More pages couldn’t be loaded", variant: "danger" }); }
    finally { setLoadingMore(false); }
  };
  const openCreate = () => navigate(selected() ? `${knowledgeHref(selected())}&new=1` : "/knowledge?new=1");
  const closeCreate = () => navigate(knowledgeHref(selected()), { replace: true });
  const reloadPage = async () => {
    const id = selected();
    if (!id) return undefined;
    await revalidate(getKnowledgePage.keyFor(id));
    return getKnowledgePage(id);
  };
  const pageSaved = async () => {
    await Promise.all([revalidate("knowledge-pages"), reloadPage()]);
    toast.show({ title: "Page saved", variant: "success" });
  };

  return <>
    <Title>Knowledge · Myelin</Title>
    <div class="knowledge-screen" classList={{ "knowledge-has-selection": Boolean(selected()) }} data-testid="knowledge-screen">
      <KnowledgeSidebar pages={pages()} selectedId={selected()} loading={first() === undefined} error={first()?.error} interactive={interactive()} hasMore={Boolean(nextCursor())} loadingMore={loadingMore()} onLoadMore={() => void loadMore()} onNew={openCreate} />
      <section class="knowledge-workspace-shell" aria-label="Knowledge page">
        <Show when={selected()} fallback={<KnowledgeWelcome hasPages={pages().length > 0} interactive={interactive()} onNew={openCreate} />}>
          <Show when={document() !== undefined} fallback={<KnowledgeLoading />}>
            <Show when={document()?.value} fallback={<KnowledgeError kind={document()?.error ?? "error"} />}>
              {(page) => <KnowledgeWorkspace page={page()} onReload={reloadPage} onSaved={pageSaved} />}
            </Show>
          </Show>
        </Show>
      </section>
    </div>
    <KnowledgeCreateDialog open={createOpen()} onClose={closeCreate} onCreated={(receipt) => { void revalidate("knowledge-pages"); setMore([]); navigate(knowledgeHref(receipt.page.id)); toast.show({ title: receipt.created ? "Page created" : "Page already existed", variant: "success" }); }} />
  </>;
}

function KnowledgeWelcome(props: { hasPages: boolean; interactive: boolean; onNew: () => void }) {
  return <div class="knowledge-welcome"><Icon name="nav-knowledge" size={34} /><p class="knowledge-eyebrow">A shared memory for engineering</p><h2>{props.hasPages ? "Choose a page" : "Turn decisions into organisational memory"}</h2><p>Write specs, runbooks, architecture decisions, and operating context in the same place your code, issues, CI, and conversations live.</p><button type="button" class="knowledge-button primary" disabled={!props.interactive} onClick={() => props.onNew()}><Icon name="doc" /> New page</button></div>;
}

function KnowledgeLoading() { return <div class="knowledge-loading"><Skeleton label="Loading page…" rows={4}><SkeletonBlock height="3rem" /><SkeletonBlock height="2rem" /><SkeletonBlock height="5rem" /><SkeletonBlock height="5rem" /></Skeleton></div>; }
function KnowledgeError(props: { kind: KnowledgeErrorKind }) { return <div class="knowledge-welcome" role="alert"><Icon name="gate" size={28} /><h2>{props.kind === "not-found" ? "Page not found" : "Page unavailable"}</h2><p>{props.kind === "not-found" ? "It may have been removed, made private, or the link may be wrong." : "Knowledge couldn’t load this page. Refresh to try again."}</p><a class="knowledge-button secondary" href="/knowledge">Back to pages</a></div>; }

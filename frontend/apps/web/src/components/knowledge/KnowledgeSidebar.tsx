import { Icon, Skeleton, SkeletonBlock } from "@myelin/design-system";
import { A } from "@solidjs/router";
import { For, Show } from "solid-js";
import type { KnowledgeErrorKind, KnowledgePageSummary } from "~/lib/knowledge-api";

export function knowledgeHref(pageId?: string): string { return pageId ? `/knowledge?page=${encodeURIComponent(pageId)}` : "/knowledge"; }

export interface KnowledgeSidebarProps {
  pages: KnowledgePageSummary[];
  selectedId?: string;
  loading: boolean;
  error?: KnowledgeErrorKind | null;
  interactive: boolean;
  hasMore: boolean;
  loadingMore: boolean;
  onLoadMore: () => void;
  onNew: () => void;
}

export function KnowledgeSidebar(props: KnowledgeSidebarProps) {
  return <aside class="knowledge-sidebar" classList={{ "knowledge-sidebar-mobile-hidden": Boolean(props.selectedId) }} aria-label="Knowledge page tree">
    <header class="knowledge-sidebar-header"><div><p>Workspace</p><h1><Icon name="nav-knowledge" /> Knowledge</h1></div><button type="button" class="knowledge-icon-button" aria-label="Create a page" disabled={!props.interactive} onClick={() => props.onNew()}><Icon name="doc" /></button></header>
    <Show when={!props.loading} fallback={<Skeleton label="Loading pages…" rows={4}><SkeletonBlock height="2.25rem" /><SkeletonBlock height="2.25rem" /><SkeletonBlock height="2.25rem" /><SkeletonBlock height="2.25rem" /></Skeleton>}>
      <Show when={!props.error} fallback={<div class="knowledge-sidebar-state" role="alert"><Icon name="gate" /><strong>Pages couldn’t be loaded</strong><span>{props.error === "unavailable" ? "Knowledge is temporarily unavailable." : "Refresh to try again."}</span></div>}>
        <Show when={props.pages.length} fallback={<div class="knowledge-sidebar-state"><Icon name="doc" /><strong>Your knowledge base is ready</strong><span>Start with a decision, product spec, or operational runbook.</span><button type="button" class="knowledge-button primary" onClick={() => props.onNew()} disabled={!props.interactive}>Create the first page</button></div>}>
          <nav class="knowledge-tree" aria-label="Engineering pages"><h2><Icon name="folder" /> Engineering</h2><ul><For each={props.pages}>{(page) => <li><A href={knowledgeHref(page.id)} aria-current={page.id === props.selectedId ? "page" : undefined}><Icon name="doc" /><span>{page.title}</span><Show when={page.visibility === "private"}><small>Private</small></Show></A></li>}</For></ul></nav>
          <Show when={props.hasMore}><button type="button" class="knowledge-load-more" disabled={props.loadingMore} onClick={() => props.onLoadMore()}><Icon name={props.loadingMore ? "cycle" : "chevron"} />{props.loadingMore ? "Loading…" : "More pages"}</button></Show>
        </Show>
      </Show>
    </Show>
  </aside>;
}

import { BlockEditor, Icon, type EditorBlock } from "@myelin/design-system";
import { A, useAction } from "@solidjs/router";
import { createEffect, createSignal, onCleanup, Show, untrack } from "solid-js";
import { knowledgeMutate, type KnowledgeErrorKind, type KnowledgePage, type KnowledgeVisibility } from "~/lib/knowledge-api";

export interface KnowledgeWorkspaceProps {
  page: KnowledgePage;
  onSaved: (page: KnowledgePage) => Promise<void> | void;
  onReload: () => Promise<KnowledgePage | undefined>;
}

function errorCopy(kind: KnowledgeErrorKind): string {
  if (kind === "conflict") return "A newer version exists. Your draft is still here—choose how to continue.";
  if (kind === "bad-input") return "This draft contains content that can’t be saved yet.";
  if (kind === "not-found") return "This page is no longer available to edit.";
  if (kind === "unavailable") return "Knowledge is temporarily unavailable. Your draft is still in this browser.";
  return "We couldn’t confirm the save. Your draft is still here.";
}

export function KnowledgeWorkspace(props: KnowledgeWorkspaceProps) {
  const mutate = useAction(knowledgeMutate);
  const [pageId, setPageId] = createSignal("");
  const [title, setTitle] = createSignal("");
  const [visibility, setVisibility] = createSignal<KnowledgeVisibility>("private");
  const [blocks, setBlocks] = createSignal<EditorBlock[]>([]);
  const [version, setVersion] = createSignal(1);
  const [dirty, setDirty] = createSignal(false);
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<KnowledgeErrorKind | null>(null);
  const [savedAt, setSavedAt] = createSignal<number | null>(null);
  let saveTimer: number | undefined;

  createEffect(() => {
    const incoming = props.page;
    if (incoming.id !== pageId() || (!dirty() && incoming.version >= version())) {
      setPageId(incoming.id); setTitle(incoming.title); setVisibility(incoming.visibility);
      setBlocks(incoming.blocks.map(({ id, type, markdown, state }) => ({ id, type, markdown, state })));
      setVersion(incoming.version); setDirty(false); setError(null);
    }
  });

  const changed = () => {
    if (!props.page.can_edit) return;
    setDirty(true); setError(null);
  };

  const save = async () => {
    if (!dirty() || saving() || !props.page.can_edit || !title().trim()) return;
    if (saveTimer !== undefined) window.clearTimeout(saveTimer);
    setSaving(true); setError(null);
    try {
      const result = await mutate({ op: "save", pageId: pageId(), expectedVersion: version(), title: title(), visibility: visibility(), blocks: blocks() });
      if (!result.ok || result.op !== "save") { setError(result.ok ? "error" : result.error); return; }
      setVersion(result.receipt.version); setDirty(false); setSavedAt(Date.now());
      await props.onSaved(result.receipt.page);
    } catch { setError("error"); }
    finally { setSaving(false); }
  };

  createEffect(() => {
    title(); visibility(); blocks();
    if (!dirty() || saving() || error() === "conflict") return;
    if (saveTimer !== undefined) window.clearTimeout(saveTimer);
    saveTimer = window.setTimeout(() => void untrack(save), 1_200);
  });
  onCleanup(() => { if (saveTimer !== undefined) window.clearTimeout(saveTimer); });

  const reloadLatest = async () => {
    const latest = await props.onReload();
    if (!latest) return;
    setPageId(latest.id); setTitle(latest.title); setVisibility(latest.visibility);
    setBlocks(latest.blocks.map(({ id, type, markdown, state }) => ({ id, type, markdown, state })));
    setVersion(latest.version); setDirty(false); setError(null);
  };
  const keepMine = async () => {
    const latest = await props.onReload();
    if (!latest) return;
    setVersion(latest.version); setError(null); setDirty(true);
  };

  return <article class="knowledge-workspace">
    <header class="knowledge-page-toolbar"><A href="/knowledge" class="knowledge-mobile-back"><Icon name="chevron" /> Pages</A><div class="knowledge-page-meta"><span><Icon name={visibility() === "private" ? "human" : "team"} />{visibility() === "private" ? "Private" : "Team"}</span><span>v{version()}</span></div><div class="knowledge-save-state" role="status">{saving() ? "Saving…" : dirty() ? "Unsaved changes" : savedAt() ? "Saved" : "Up to date"}</div><Show when={props.page.can_edit}><button type="button" class="knowledge-button secondary" onClick={() => void save()} disabled={!dirty() || saving()}>Save now</button></Show></header>
    <Show when={error()}>{(kind) => <section class="knowledge-conflict" role="alert"><div><strong>{kind() === "conflict" ? "This page changed elsewhere" : "Save not confirmed"}</strong><p>{errorCopy(kind())}</p></div><Show when={kind() === "conflict"} fallback={<button type="button" class="knowledge-button secondary" onClick={() => void save()}>Retry</button>}><button type="button" class="knowledge-button secondary" onClick={() => void reloadLatest()}>Reload latest</button><button type="button" class="knowledge-button primary" onClick={() => void keepMine()}>Keep my draft</button></Show></section>}</Show>
    <main class="knowledge-document"><input class="knowledge-title" aria-label="Page title" value={title()} maxlength={512} readonly={!props.page.can_edit || props.page.title_state === "tombstoned"} onInput={(event) => { setTitle(event.currentTarget.value); changed(); }} /><div class="knowledge-document-controls"><label>Visibility<select value={visibility()} disabled={!props.page.can_edit} onChange={(event) => { setVisibility(event.currentTarget.value as KnowledgeVisibility); changed(); }}><option value="private">Private</option><option value="team">Team</option></select></label><span>{blocks().length} block{blocks().length === 1 ? "" : "s"}</span></div><BlockEditor value={blocks()} readOnly={!props.page.can_edit} label={`Edit ${title()}`} onChange={(next) => { setBlocks(next); changed(); }} /></main>
  </article>;
}

import { BlockEditor, Icon, type EditorBlock } from "@myelin/design-system";
import { A, useAction } from "@solidjs/router";
import { createEffect, createSignal, onCleanup, Show } from "solid-js";

import { CopyArtifactRef } from "~/components/CopyArtifactRef";
import { KnowledgeReferenceComposer } from "~/components/knowledge/KnowledgeReferenceComposer";
import { artifactRefHref, artifactRefLabel } from "~/lib/artifact-ref";
import {
  knowledgeMutate,
  type KnowledgeErrorKind,
  type KnowledgePage,
  type KnowledgeVisibility,
} from "~/lib/knowledge-api";

type KnowledgeEditorBlock = EditorBlock & { references?: string[] };

interface KnowledgeDraft {
  title: string;
  visibility: KnowledgeVisibility;
  blocks: KnowledgeEditorBlock[];
  version: number;
  revision: number;
  editable: boolean;
  dirty: boolean;
  saving: boolean;
  error: KnowledgeErrorKind | null;
  savedAt: number | null;
}

export interface KnowledgeWorkspaceProps {
  page: KnowledgePage;
  onSaved: (page: KnowledgePage) => Promise<void> | void;
  onReload: (pageId: string) => Promise<KnowledgePage | undefined>;
}

function blocksFrom(page: KnowledgePage): KnowledgeEditorBlock[] {
  return page.blocks.map(({ id, type, markdown, references, state }) => ({
    id,
    type,
    markdown,
    references,
    state,
  }));
}

function draftFrom(page: KnowledgePage, savedAt: number | null = null): KnowledgeDraft {
  return {
    title: page.title,
    visibility: page.visibility,
    blocks: blocksFrom(page),
    version: page.version,
    revision: 0,
    editable: page.can_edit,
    dirty: false,
    saving: false,
    error: null,
    savedAt,
  };
}

function errorCopy(kind: KnowledgeErrorKind): string {
  if (kind === "conflict") {
    return "A newer version exists. Your draft is still here—choose how to continue.";
  }
  if (kind === "bad-input") return "This draft contains content that can’t be saved yet.";
  if (kind === "not-found") return "This page is no longer available to edit.";
  if (kind === "unavailable") {
    return "Knowledge is temporarily unavailable. Your draft is still in this browser.";
  }
  return "We couldn’t confirm the save. Your draft is still here.";
}

export function KnowledgeWorkspace(props: KnowledgeWorkspaceProps) {
  const mutate = useAction(knowledgeMutate);
  const [drafts, setDrafts] = createSignal(new Map<string, KnowledgeDraft>());
  const saveTimers = new Map<string, number>();

  createEffect(() => {
    const incoming = props.page;
    setDrafts((current) => {
      const existing = current.get(incoming.id);
      if (existing && (existing.dirty || existing.saving || incoming.version <= existing.version)) {
        return current;
      }
      return new Map(current).set(incoming.id, draftFrom(incoming, existing?.savedAt));
    });
  });

  const draft = () => drafts().get(props.page.id) ?? draftFrom(props.page);

  const updateDraft = (
    pageId: string,
    update: (current: KnowledgeDraft) => KnowledgeDraft,
  ) => {
    setDrafts((current) => {
      const existing = current.get(pageId);
      if (!existing) return current;
      return new Map(current).set(pageId, update(existing));
    });
  };

  const cancelSave = (pageId: string) => {
    const timer = saveTimers.get(pageId);
    if (timer !== undefined) window.clearTimeout(timer);
    saveTimers.delete(pageId);
  };

  const scheduleSave = (pageId: string) => {
    cancelSave(pageId);
    saveTimers.set(pageId, window.setTimeout(() => {
      saveTimers.delete(pageId);
      void save(pageId);
    }, 1_200));
  };

  const edit = (update: (current: KnowledgeDraft) => KnowledgeDraft) => {
    const pageId = props.page.id;
    if (!props.page.can_edit) return;
    let conflicted = false;
    updateDraft(pageId, (current) => {
      conflicted = current.error === "conflict";
      return {
        ...update(current),
        revision: current.revision + 1,
        dirty: true,
        error: conflicted ? "conflict" : null,
      };
    });
    if (!conflicted) scheduleSave(pageId);
  };

  const rejectSave = (pageId: string, error: KnowledgeErrorKind) => {
    updateDraft(pageId, (current) => ({ ...current, saving: false, error }));
  };

  const save = async (pageId = props.page.id) => {
    const pending = drafts().get(pageId);
    if (!pending?.dirty || pending.saving || !pending.editable || !pending.title.trim()) return;
    cancelSave(pageId);
    const savedRevision = pending.revision;
    updateDraft(pageId, (current) => ({ ...current, saving: true, error: null }));

    let result;
    try {
      result = await mutate({
        op: "save",
        pageId,
        expectedVersion: pending.version,
        title: pending.title,
        visibility: pending.visibility,
        blocks: pending.blocks,
      });
    } catch {
      rejectSave(pageId, "error");
      return;
    }

    if (!result.ok || result.op !== "save") {
      rejectSave(pageId, result.ok ? "error" : result.error);
      return;
    }

    let hasNewerEdits = false;
    const savedAt = Date.now();
    updateDraft(pageId, (current) => {
      hasNewerEdits = current.revision !== savedRevision;
      if (!hasNewerEdits) return draftFrom(result.receipt.page, savedAt);
      return {
        ...current,
        version: result.receipt.version,
        editable: result.receipt.page.can_edit,
        dirty: true,
        saving: false,
        error: null,
        savedAt,
      };
    });
    if (hasNewerEdits) scheduleSave(pageId);
    await props.onSaved(result.receipt.page);
  };

  onCleanup(() => {
    for (const timer of saveTimers.values()) window.clearTimeout(timer);
    saveTimers.clear();
  });

  const reloadLatest = async () => {
    const pageId = props.page.id;
    const latest = await props.onReload(pageId);
    if (!latest) {
      rejectSave(pageId, "unavailable");
      return;
    }
    cancelSave(pageId);
    updateDraft(pageId, (current) => draftFrom(latest, current.savedAt));
  };

  const keepMine = async () => {
    const pageId = props.page.id;
    const latest = await props.onReload(pageId);
    if (!latest) {
      rejectSave(pageId, "unavailable");
      return;
    }
    updateDraft(pageId, (current) => ({
      ...current,
      version: latest.version,
      editable: latest.can_edit,
      revision: current.revision + 1,
      dirty: true,
      error: null,
    }));
    scheduleSave(pageId);
  };

  const references = () => draft().blocks.flatMap((block) => block.references ?? []);
  const addReference = (reference: string) => {
    edit((current) => ({
      ...current,
      blocks: [...current.blocks, {
        type: "paragraph",
        markdown: "Related work: \uFFFC",
        references: [reference],
      }],
    }));
  };

  return (
    <article class="knowledge-workspace">
      <header class="knowledge-page-toolbar">
        <A href="/knowledge" class="knowledge-mobile-back">
          <Icon name="chevron" /> Pages
        </A>
        <div class="knowledge-page-meta">
          <span>
            <Icon name={draft().visibility === "private" ? "human" : "team"} />
            {draft().visibility === "private" ? "Private" : "Team"}
          </span>
          <span>v{draft().version}</span>
        </div>
        <CopyArtifactRef reference={props.page.ref} />
        <div class="knowledge-save-state" role="status">
          {draft().saving
            ? "Saving…"
            : draft().dirty
              ? "Unsaved changes"
              : draft().savedAt ? "Saved" : "Up to date"}
        </div>
        <Show when={props.page.can_edit}>
          <button
            type="button"
            class="knowledge-button secondary"
            onClick={() => void save()}
            disabled={!draft().dirty || draft().saving}
          >
            Save now
          </button>
        </Show>
      </header>

      <Show when={draft().error}>
        {(kind) => (
          <section class="knowledge-conflict" role="alert">
            <div>
              <strong>{kind() === "conflict" ? "This page changed elsewhere" : "Save not confirmed"}</strong>
              <p>{errorCopy(kind())}</p>
            </div>
            <Show
              when={kind() === "conflict"}
              fallback={
                <button type="button" class="knowledge-button secondary" onClick={() => void save()}>
                  Retry
                </button>
              }
            >
              <button type="button" class="knowledge-button secondary" onClick={() => void reloadLatest()}>
                Reload latest
              </button>
              <button type="button" class="knowledge-button primary" onClick={() => void keepMine()}>
                Keep my draft
              </button>
            </Show>
          </section>
        )}
      </Show>

      <main class="knowledge-document">
        <input
          class="knowledge-title"
          aria-label="Page title"
          value={draft().title}
          maxlength={512}
          readonly={!props.page.can_edit || props.page.title_state === "tombstoned"}
          onInput={(event) => edit((current) => ({
            ...current,
            title: event.currentTarget.value,
          }))}
        />
        <div class="knowledge-document-controls">
          <label>
            Visibility
            <select
              value={draft().visibility}
              disabled={!props.page.can_edit}
              onChange={(event) => edit((current) => ({
                ...current,
                visibility: event.currentTarget.value as KnowledgeVisibility,
              }))}
            >
              <option value="private">Private</option>
              <option value="team">Team</option>
            </select>
          </label>
          <span>
            {draft().blocks.length} block{draft().blocks.length === 1 ? "" : "s"}
          </span>
          <KnowledgeReferenceComposer
            pageRef={props.page.ref}
            references={references()}
            disabled={!props.page.can_edit}
            atCapacity={draft().blocks.length >= 500 || references().length >= 100}
            onAdd={addReference}
          />
        </div>
        <BlockEditor
          value={draft().blocks}
          readOnly={!props.page.can_edit}
          label={`Edit ${draft().title}`}
          referenceLabel={artifactRefLabel}
          referenceHref={artifactRefHref}
          onChange={(blocks) => edit((current) => ({ ...current, blocks }))}
        />
      </main>
    </article>
  );
}

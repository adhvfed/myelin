import { Icon } from "@myelin/design-system";
import { Show, createSignal, createUniqueId } from "solid-js";

import { isStorableArtifactRef, parseArtifactRef } from "~/lib/artifact-ref";

export interface KnowledgeReferenceComposerProps {
  pageRef: string;
  references: readonly string[];
  disabled: boolean;
  atCapacity: boolean;
  onAdd: (reference: string) => void;
}

export function KnowledgeReferenceComposer(props: KnowledgeReferenceComposerProps) {
  const inputId = createUniqueId();
  const [open, setOpen] = createSignal(false);
  const [draft, setDraft] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);

  const close = () => {
    setOpen(false);
    setDraft("");
    setError(null);
  };

  const add = () => {
    const reference = draft().trim();
    const target = parseArtifactRef(reference);
    const source = parseArtifactRef(props.pageRef);
    if (!isStorableArtifactRef(reference) || !target || !source) {
      setError("Paste a complete canonical myelin:// reference.");
      return;
    }
    if (target.tenant !== source.tenant) {
      setError("Related work must belong to this workspace.");
      return;
    }
    if (target.root === source.root) {
      setError("Choose work outside this page.");
      return;
    }
    if (props.references.includes(reference)) {
      setError("This work is already linked from the page.");
      return;
    }
    props.onAdd(reference);
    close();
  };

  return <section class="knowledge-reference-composer" aria-label="Related work">
    <Show when={!open()} fallback={
      <form onSubmit={(event) => { event.preventDefault(); add(); }}>
        <div>
          <label>Canonical Myelin reference
            <input
              id={inputId}
              value={draft()}
              maxlength={1024}
              placeholder="myelin://workspace/issue/issue/ENG-41"
              aria-invalid={Boolean(error())}
              aria-describedby={error() ? `${inputId}-error` : `${inputId}-hint`}
              autofocus
              onInput={(event) => { setDraft(event.currentTarget.value); setError(null); }}
            />
          </label>
          <button type="submit" class="knowledge-button primary" disabled={!draft().trim()}>Add</button>
          <button type="button" class="knowledge-button secondary" onClick={close}>Cancel</button>
        </div>
        <Show when={error()} fallback={<p id={`${inputId}-hint`}>Paste a reference copied from an issue, pull request, CI run, or Knowledge page.</p>}>
          {(message) => <p id={`${inputId}-error`} class="knowledge-error" role="alert">{message()}</p>}
        </Show>
      </form>
    }>
      <button
        type="button"
        class="knowledge-link-work"
        disabled={props.disabled || props.atCapacity}
        aria-expanded="false"
        title={props.atCapacity ? "This page has reached its structured-reference limit" : undefined}
        onClick={() => setOpen(true)}
      >
        <Icon name="link" /> Link related work
      </button>
    </Show>
  </section>;
}

import { Dialog, Icon } from "@myelin/design-system";
import { useAction } from "@solidjs/router";
import { createEffect, createSignal, onCleanup, Show, untrack } from "solid-js";

import {
  commitGitFile,
  parseGitFileEditDraft,
  type GitFileEditError,
  type GitFileEditReceipt,
} from "~/lib/git-file-edit";

interface CommittedFile extends GitFileEditReceipt {
  ref: string;
  path: string;
}

function errorCopy(kind: GitFileEditError): string {
  if (kind === "bad-input") return "Check the file path, contents, and commit message.";
  if (kind === "not-found") return "This repository or branch is no longer available.";
  if (kind === "conflict") return "Another edit landed first. Reload the file before editing it again.";
  if (kind === "forbidden") return "You cannot commit directly to this branch. Use an unprotected branch and open a pull request.";
  if (kind === "too-large") return "This file is too large for the browser editor. Commit it through Git instead.";
  if (kind === "unavailable") return "Git is temporarily unavailable. Your unchanged draft is safe to retry.";
  return "We couldn’t confirm the commit. Your unchanged draft keeps its retry identity.";
}

export function GitFileEditorDialog(props: {
  open: boolean;
  mode: "create" | "edit";
  repo: string;
  refName: string;
  initialPath?: string;
  initialContents?: string;
  initialBaseOid?: string;
  onClose: () => void;
  onCommitted: (file: CommittedFile) => void;
}) {
  const commit = useAction(commitGitFile);
  const [path, setPath] = createSignal("");
  const [contents, setContents] = createSignal("");
  const [message, setMessage] = createSignal("");
  const [clientNonce, setClientNonce] = createSignal(crypto.randomUUID());
  const [submitting, setSubmitting] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  let pathInput: HTMLInputElement | undefined;
  let contentsInput: HTMLTextAreaElement | undefined;
  let openingGeneration = 0;

  createEffect(() => {
    openingGeneration += 1;
    if (!props.open) return;
    const initialPath = untrack(() => props.initialPath ?? "");
    setPath(initialPath);
    setContents(untrack(() => props.initialContents ?? ""));
    setMessage(props.mode === "edit" ? `Update ${initialPath}` : "Create file");
    setClientNonce(crypto.randomUUID());
    setSubmitting(false);
    setError(null);
  });
  onCleanup(() => { openingGeneration += 1; });

  const edit = (apply: () => void) => {
    apply();
    setClientNonce(crypto.randomUUID());
    setError(null);
  };
  const close = () => { if (!submitting()) props.onClose(); };
  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    const draft = parseGitFileEditDraft({
      repo: props.repo,
      ref: props.refName,
      path: path(),
      baseOid: props.initialBaseOid ?? "",
      contents: contents(),
      message: message(),
      clientNonce: clientNonce(),
    });
    if (!draft) {
      setError(errorCopy("bad-input"));
      (path() ? contentsInput : pathInput)?.focus();
      return;
    }

    const generation = openingGeneration;
    setSubmitting(true);
    setError(null);
    try {
      const result = await commit(draft);
      if (generation !== openingGeneration) return;
      if (!result.ok) {
        setError(errorCopy(result.error));
        return;
      }
      props.onCommitted({ ...result.receipt, ref: draft.ref, path: draft.path });
      props.onClose();
    } catch {
      if (generation === openingGeneration) setError(errorCopy("error"));
    } finally {
      if (generation === openingGeneration) setSubmitting(false);
    }
  };

  const editing = () => props.mode === "edit";
  return (
    <Dialog
      open={props.open}
      onClose={close}
      title={editing() ? `Edit ${props.initialPath ?? "file"}` : "Create file"}
      description={`Commit one text file to ${props.refName}. Branch protection remains authoritative.`}
      size="lg"
      dismissable={!submitting()}
      initialFocus={() => editing() ? contentsInput : pathInput}
      footer={<>
        <button type="button" class="repo-file-button" onClick={close} disabled={submitting()}>
          Cancel
        </button>
        <button
          type="submit"
          form="git-file-editor"
          class="repo-file-button primary"
          disabled={submitting() || !path() || !message().trim()}
        >
          <Icon name={submitting() ? "cycle" : "commit"} />
          {submitting() ? "Committing…" : editing() ? "Commit changes" : "Commit file"}
        </button>
      </>}
    >
      <form id="git-file-editor" class="repo-file-editor" onSubmit={submit}>
        <label>
          File path
          <input
            ref={pathInput}
            value={path()}
            readOnly={editing()}
            maxlength={4_096}
            autocomplete="off"
            spellcheck={false}
            disabled={submitting()}
            onInput={(event) => edit(() => setPath(event.currentTarget.value))}
          />
        </label>
        <label>
          File contents
          <textarea
            ref={contentsInput}
            value={contents()}
            rows={18}
            autocomplete="off"
            spellcheck={false}
            disabled={submitting()}
            onInput={(event) => edit(() => setContents(event.currentTarget.value))}
          />
        </label>
        <label>
          Commit message
          <input
            value={message()}
            maxlength={8_192}
            autocomplete="off"
            disabled={submitting()}
            onInput={(event) => edit(() => setMessage(event.currentTarget.value))}
          />
        </label>
        <p class="repo-file-note">Edits are compare-and-swap commits. Myelin refuses to overwrite a file that changed after you opened it.</p>
        <Show when={error()}>{(copy) => <p role="alert" class="repo-file-error">{copy()}</p>}</Show>
      </form>
    </Dialog>
  );
}

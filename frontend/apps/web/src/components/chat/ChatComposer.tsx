import { BlockEditor, Icon, type EditorBlock } from "@myelin/design-system";
import { useAction } from "@solidjs/router";
import { createEffect, createSignal, onMount, Show } from "solid-js";

import { chatMutate, type ChatErrorKind } from "~/lib/api";
import {
  artifactRefHref,
  artifactRefLabel,
  relatedArtifactRefError,
  type RelatedArtifactRefError,
} from "~/lib/artifact-ref";

export interface ChatComposerProps {
  conversationId: string;
  conversationRef: string;
  topic: string;
  threadRootId?: string;
  onPosted: (conversationId: string) => Promise<void> | void;
}

interface ConversationDraft {
  content: string;
  references: string[];
  clientNonce: string;
  error: string | null;
  sending: boolean;
  linking: boolean;
  referenceInput: string;
  referenceError: string | null;
}

const EMPTY_DRAFT: ConversationDraft = {
  content: "",
  references: [],
  clientNonce: "",
  error: null,
  sending: false,
  linking: false,
  referenceInput: "",
  referenceError: null,
};

function newDraft(): ConversationDraft {
  return { ...EMPTY_DRAFT, references: [], clientNonce: crypto.randomUUID() };
}

function errorCopy(kind: ChatErrorKind): string {
  switch (kind) {
    case "bad-input":
      return "Write a message before sending.";
    case "not-found":
      return "This topic is no longer available.";
    case "unavailable":
      return "Chat is temporarily unavailable. Your message was not confirmed; retrying this draft is safe.";
    case "conflict":
      return "That send was already handled. Refreshing the conversation may reveal it.";
    default:
      return "We couldn’t confirm the send. This draft keeps its retry identity until you edit it.";
  }
}

function referenceErrorCopy(kind: RelatedArtifactRefError): string {
  if (kind === "cross-tenant") return "Linked work must belong to this workspace.";
  if (kind === "self") return "Choose work outside this conversation.";
  if (kind === "duplicate") return "This work is already linked in the draft.";
  return "Paste a complete canonical myelin:// reference.";
}

export function ChatComposer(props: ChatComposerProps) {
  const mutate = useAction(chatMutate);
  const [drafts, setDrafts] = createSignal(new Map<string, ConversationDraft>());
  const [interactive, setInteractive] = createSignal(false);

  onMount(() => setInteractive(true));

  const draftId = () => props.threadRootId
    ? `${props.conversationId}:thread:${props.threadRootId}`
    : props.conversationId;

  createEffect(() => {
    const conversationId = draftId();
    setDrafts((current) => {
      if (current.has(conversationId)) return current;
      return new Map(current).set(conversationId, newDraft());
    });
  });

  const draft = () => drafts().get(draftId()) ?? EMPTY_DRAFT;
  const updateDraft = (
    conversationId: string,
    update: (current: ConversationDraft) => ConversationDraft,
  ) => {
    setDrafts((current) => {
      const next = new Map(current);
      next.set(conversationId, update(current.get(conversationId) ?? newDraft()));
      return next;
    });
  };

  const rejectSend = (conversationId: string, clientNonce: string, message: string) => {
    updateDraft(conversationId, (current) => current.clientNonce === clientNonce
      ? { ...current, error: message, sending: false }
      : current);
  };

  const send = async () => {
    const conversationId = draftId();
    const outgoing = draft();
    if (outgoing.sending) return;
    if (!outgoing.content.trim()) {
      updateDraft(conversationId, (current) => ({
        ...current,
        error: "Write a message before sending.",
      }));
      return;
    }
    updateDraft(conversationId, (current) => ({ ...current, error: null, sending: true }));

    let result;
    try {
      result = props.threadRootId
        ? await mutate({
            op: "post-reply",
            rootMessageId: props.threadRootId,
            content: outgoing.content,
            references: outgoing.references,
            clientNonce: outgoing.clientNonce,
          })
        : await mutate({
            op: "post-message",
            conversationId: props.conversationId,
            content: outgoing.content,
            references: outgoing.references,
            clientNonce: outgoing.clientNonce,
          });
    } catch {
      rejectSend(conversationId, outgoing.clientNonce, errorCopy("error"));
      return;
    }

    if (!result.ok) {
      rejectSend(conversationId, outgoing.clientNonce, errorCopy(result.error));
      return;
    }
    const expectedOperation = props.threadRootId ? "post-reply" : "post-message";
    if (result.op !== expectedOperation) {
      rejectSend(conversationId, outgoing.clientNonce, errorCopy("error"));
      return;
    }

    const completed = { ...newDraft(), sending: true };
    updateDraft(conversationId, (current) => current.clientNonce === outgoing.clientNonce
      ? completed
      : current);
    try {
      await props.onPosted(props.conversationId);
    } catch {
      updateDraft(conversationId, (current) => current.clientNonce === completed.clientNonce
        ? { ...current, error: "Message sent, but the timeline couldn’t refresh. Reload to see it." }
        : current);
    } finally {
      updateDraft(conversationId, (current) => current.clientNonce === completed.clientNonce
        ? { ...current, sending: false }
        : current);
    }
  };

  const addReference = () => {
    const conversationId = draftId();
    const current = draft();
    const reference = current.referenceInput.trim();
    const validation = relatedArtifactRefError(
      props.conversationRef,
      reference,
      current.references,
    );
    if (validation) {
      updateDraft(conversationId, (value) => ({
        ...value,
        referenceError: referenceErrorCopy(validation),
      }));
      return;
    }
    updateDraft(conversationId, (value) => ({
      ...value,
      content: `${value.content}${value.content && !/\s$/u.test(value.content) ? " " : ""}\uFFFC`,
      references: [...value.references, reference],
      clientNonce: crypto.randomUUID(),
      error: null,
      linking: false,
      referenceInput: "",
      referenceError: null,
    }));
  };

  const editorValue = (): EditorBlock[] => [{
    type: "paragraph",
    markdown: draft().content,
    references: draft().references,
  }];

  return (
    <div class="chat-composer">
      <div class="chat-composer-editor" classList={{ invalid: Boolean(draft().error) }}>
        <BlockEditor
          value={editorValue()}
          readOnly={!interactive() || draft().sending}
          inputLabel={props.threadRootId ? `Reply in ${props.topic}` : `Message ${props.topic}`}
          referenceLabel={artifactRefLabel}
          referenceHref={artifactRefHref}
          onSubmit={() => void send()}
          onChange={(blocks) => {
            const conversationId = draftId();
            updateDraft(conversationId, (current) => ({
              ...current,
              content: blocks.map((block) => block.markdown).join("\n"),
              references: blocks.flatMap((block) => block.references ?? []),
              clientNonce: crypto.randomUUID(),
              error: null,
            }));
          }}
        />
      </div>
      <Show when={draft().linking}>
        <form
          class="chat-reference-form"
          onSubmit={(event) => {
            event.preventDefault();
            addReference();
          }}
        >
          <label>
            Canonical Myelin reference
            <input
              value={draft().referenceInput}
              maxlength={1024}
              autocomplete="off"
              autofocus
              placeholder="myelin://workspace/issue/issue/ENG-41"
              aria-invalid={Boolean(draft().referenceError)}
              onInput={(event) => {
                const conversationId = draftId();
                updateDraft(conversationId, (current) => ({
                  ...current,
                  referenceInput: event.currentTarget.value,
                  referenceError: null,
                }));
              }}
            />
          </label>
          <div>
            <button
              type="submit"
              class="chat-button chat-button-primary"
              disabled={!draft().referenceInput.trim()}
            >
              Add reference
            </button>
            <button
              type="button"
              class="chat-button chat-button-secondary"
              onClick={() => {
                const conversationId = draftId();
                updateDraft(conversationId, (current) => ({
                  ...current,
                  linking: false,
                  referenceInput: "",
                  referenceError: null,
                }));
              }}
            >
              Cancel
            </button>
          </div>
          <Show
            when={draft().referenceError}
            fallback={<p>Paste a reference copied from an issue, pull request, CI run, or Knowledge page.</p>}
          >
            {(message) => <p role="alert" class="chat-field-error">{message()}</p>}
          </Show>
        </form>
      </Show>
      <div class="chat-composer-footer">
        <div class="chat-composer-guidance">
          <button
            type="button"
            class="chat-link-work"
            disabled={draft().sending || draft().references.length >= 32}
            title={draft().references.length >= 32
              ? "This message has reached its structured-reference limit"
              : undefined}
            onClick={() => {
              const conversationId = draftId();
              updateDraft(conversationId, (current) => ({ ...current, linking: true }));
            }}
          >
            <Icon name="link" /> Link work
          </button>
          <Show
            when={draft().error}
            fallback={<span id="chat-composer-hint">Enter to send · Shift+Enter for a new line</span>}
          >
            {(message) => <span id="chat-composer-error" role="alert" class="chat-field-error">{message()}</span>}
          </Show>
        </div>
        <button
          type="button"
          class="chat-button chat-button-primary"
          onClick={() => void send()}
          disabled={!interactive() || draft().sending || !draft().content.trim()}
        >
          <Icon name={draft().sending ? "cycle" : "message"} />
          {draft().sending ? "Sending…" : "Send"}
        </button>
      </div>
    </div>
  );
}

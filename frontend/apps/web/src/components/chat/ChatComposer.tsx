import { BlockEditor, Icon, type EditorBlock } from "@myelin/design-system";
import { useAction } from "@solidjs/router";
import { createEffect, createSignal, onMount, Show } from "solid-js";

import { chatMutate, type ChatErrorKind } from "~/lib/api";

export interface ChatComposerProps {
  conversationId: string;
  topic: string;
  onPosted: (conversationId: string) => Promise<void> | void;
}

interface ConversationDraft {
  content: string;
  clientNonce: string;
  error: string | null;
  sending: boolean;
}

const EMPTY_DRAFT: ConversationDraft = {
  content: "",
  clientNonce: "",
  error: null,
  sending: false,
};

function newDraft(): ConversationDraft {
  return { ...EMPTY_DRAFT, clientNonce: crypto.randomUUID() };
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

export function ChatComposer(props: ChatComposerProps) {
  const mutate = useAction(chatMutate);
  const [drafts, setDrafts] = createSignal(new Map<string, ConversationDraft>());
  const [interactive, setInteractive] = createSignal(false);

  onMount(() => setInteractive(true));

  createEffect(() => {
    const conversationId = props.conversationId;
    setDrafts((current) => {
      if (current.has(conversationId)) return current;
      return new Map(current).set(conversationId, newDraft());
    });
  });

  const draft = () => drafts().get(props.conversationId) ?? EMPTY_DRAFT;
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
    const conversationId = props.conversationId;
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
      result = await mutate({
        op: "post-message",
        conversationId,
        content: outgoing.content,
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
    if (result.op !== "post-message") {
      rejectSend(conversationId, outgoing.clientNonce, errorCopy("error"));
      return;
    }

    const completed = { ...newDraft(), sending: true };
    updateDraft(conversationId, (current) => current.clientNonce === outgoing.clientNonce
      ? completed
      : current);
    try {
      await props.onPosted(conversationId);
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

  const editorValue = (): EditorBlock[] => [{ type: "paragraph", markdown: draft().content }];

  return (
    <div class="chat-composer">
      <div class="chat-composer-editor" classList={{ invalid: Boolean(draft().error) }}>
        <BlockEditor
          value={editorValue()}
          readOnly={!interactive() || draft().sending}
          inputLabel={`Message ${props.topic}`}
          onSubmit={() => void send()}
          onChange={(blocks) => {
            const conversationId = props.conversationId;
            updateDraft(conversationId, (current) => ({
              ...current,
              content: blocks.map((block) => block.markdown).join("\n"),
              clientNonce: crypto.randomUUID(),
              error: null,
            }));
          }}
        />
      </div>
      <div class="chat-composer-footer">
        <Show
          when={draft().error}
          fallback={<span id="chat-composer-hint">Enter to send · Shift+Enter for a new line</span>}
        >
          {(message) => <span id="chat-composer-error" role="alert" class="chat-field-error">{message()}</span>}
        </Show>
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

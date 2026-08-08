import { BlockEditor, Icon, type EditorBlock } from "@myelin/design-system";
import { useAction } from "@solidjs/router";
import { createSignal, onMount, Show } from "solid-js";

import { chatMutate, type ChatErrorKind } from "~/lib/api";

export interface ChatComposerProps {
  conversationId: string;
  topic: string;
  onPosted: () => Promise<void> | void;
}

function errorCopy(kind: ChatErrorKind): string {
  switch (kind) {
    case "bad-input":
      return "Write a message before sending.";
    case "not-found":
      return "This topic is no longer available.";
    case "unavailable":
      return "Chat is temporarily unavailable. Your message was not confirmed.";
    case "conflict":
      return "That send was already handled. Refreshing the conversation may reveal it.";
    default:
      return "We couldn’t confirm the send. Refresh before trying again.";
  }
}

export function ChatComposer(props: ChatComposerProps) {
  const mutate = useAction(chatMutate);
  const [content, setContent] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [sending, setSending] = createSignal(false);
  const [interactive, setInteractive] = createSignal(false);

  onMount(() => setInteractive(true));

  const send = async () => {
    if (sending()) return;
    if (!content().trim()) {
      setError("Write a message before sending.");
      return;
    }
    const sentContent = content();
    setSending(true);
    setError(null);
    try {
      const result = await mutate({
        op: "post-message",
        conversationId: props.conversationId,
        content: sentContent,
        clientNonce: crypto.randomUUID(),
      });
      if (!result.ok) {
        setError(errorCopy(result.error));
        return;
      }
      if (result.op !== "post-message") {
        setError(errorCopy("error"));
        return;
      }
      setContent("");
      await props.onPosted();
    } catch {
      setError(errorCopy("error"));
    } finally {
      setSending(false);
    }
  };

  const editorValue = (): EditorBlock[] => [{ type: "paragraph", markdown: content() }];

  return (
    <div class="chat-composer">
      <div class="chat-composer-editor" classList={{ invalid: Boolean(error()) }}>
        <BlockEditor
          value={editorValue()}
          readOnly={!interactive() || sending()}
          inputLabel={`Message ${props.topic}`}
          onSubmit={() => void send()}
          onChange={(blocks) => {
            setContent(blocks.map((block) => block.markdown).join("\n"));
            setError(null);
          }}
        />
      </div>
      <div class="chat-composer-footer">
        <Show
          when={error()}
          fallback={<span id="chat-composer-hint">Enter to send · Shift+Enter for a new line</span>}
        >
          {(message) => <span id="chat-composer-error" role="alert" class="chat-field-error">{message()}</span>}
        </Show>
        <button
          type="button"
          class="chat-button chat-button-primary"
          onClick={() => void send()}
          disabled={!interactive() || sending() || !content().trim()}
        >
          <Icon name={sending() ? "cycle" : "message"} />
          {sending() ? "Sending…" : "Send"}
        </button>
      </div>
    </div>
  );
}

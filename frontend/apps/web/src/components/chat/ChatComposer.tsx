import { Icon } from "@myelin/design-system";
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
  let input: HTMLTextAreaElement | undefined;

  onMount(() => setInteractive(true));

  const send = async () => {
    if (sending()) return;
    if (!content().trim()) {
      setError("Write a message before sending.");
      input?.focus();
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
      input?.focus();
    } catch {
      setError(errorCopy("error"));
    } finally {
      setSending(false);
    }
  };

  const keydown = (event: KeyboardEvent) => {
    if (event.key !== "Enter" || event.shiftKey || event.isComposing) return;
    event.preventDefault();
    void send();
  };

  return (
    <div class="chat-composer">
      <label class="sr-only" for="chat-message-composer">Message {props.topic}</label>
      <textarea
        ref={input}
        id="chat-message-composer"
        rows={2}
        value={content()}
        onInput={(event) => {
          setContent(event.currentTarget.value);
          setError(null);
        }}
        onKeyDown={keydown}
        disabled={!interactive() || sending()}
        maxlength={32 * 1024}
        placeholder={`Message ${props.topic}`}
        aria-describedby={error() ? "chat-composer-error" : "chat-composer-hint"}
        aria-invalid={Boolean(error())}
      />
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

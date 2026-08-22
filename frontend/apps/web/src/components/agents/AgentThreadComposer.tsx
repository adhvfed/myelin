import { Icon } from "@myelin/design-system";
import { useAction } from "@solidjs/router";
import { createEffect, createSignal, onMount, Show } from "solid-js";

import { SharedComposer } from "~/components/SharedComposer";
import {
  mutateAgentThread,
  type AgentThreadErrorKind,
} from "~/lib/agent-thread-api";

interface Draft {
  content: string;
  clientNonce: string;
  sending: boolean;
  error: string | null;
}

function newDraft(): Draft {
  return { content: "", clientNonce: crypto.randomUUID(), sending: false, error: null };
}

function errorCopy(kind: AgentThreadErrorKind): string {
  if (kind === "bad-input") return "Write a message before sending.";
  if (kind === "not-found") return "This private thread is no longer available.";
  if (kind === "conflict") return "That send was already handled. Refreshing may reveal it.";
  if (kind === "unavailable") {
    return "Private work is temporarily unavailable. Retrying this unchanged draft is safe.";
  }
  return "We couldn’t confirm the send. This draft keeps its retry identity until you edit it.";
}

export function AgentThreadComposer(props: {
  threadId: string;
  threadName: string;
  disabled?: boolean;
  onPosted: (threadId: string) => Promise<void> | void;
}) {
  const mutate = useAction(mutateAgentThread);
  const [drafts, setDrafts] = createSignal(new Map<string, Draft>());
  const [interactive, setInteractive] = createSignal(false);

  onMount(() => setInteractive(true));
  createEffect(() => {
    const threadId = props.threadId;
    setDrafts((current) => current.has(threadId)
      ? current
      : new Map(current).set(threadId, newDraft()));
  });

  const draft = () => drafts().get(props.threadId) ?? newDraft();
  const update = (threadId: string, transform: (current: Draft) => Draft) => {
    setDrafts((current) => {
      const next = new Map(current);
      next.set(threadId, transform(current.get(threadId) ?? newDraft()));
      return next;
    });
  };

  const send = async () => {
    const threadId = props.threadId;
    const outgoing = draft();
    if (outgoing.sending || props.disabled) return;
    if (!outgoing.content.trim()) {
      update(threadId, (current) => ({ ...current, error: "Write a message before sending." }));
      return;
    }
    update(threadId, (current) => ({ ...current, error: null, sending: true }));
    let result;
    try {
      result = await mutate({
        op: "post-message",
        threadId,
        content: outgoing.content,
        clientNonce: outgoing.clientNonce,
      });
    } catch {
      update(threadId, (current) => current.clientNonce === outgoing.clientNonce
        ? { ...current, sending: false, error: errorCopy("error") }
        : current);
      return;
    }
    if (!result.ok || result.op !== "post-message") {
      update(threadId, (current) => current.clientNonce === outgoing.clientNonce
        ? { ...current, sending: false, error: errorCopy(result.ok ? "error" : result.error) }
        : current);
      return;
    }

    const completed = { ...newDraft(), sending: true };
    update(threadId, (current) => current.clientNonce === outgoing.clientNonce
      ? completed
      : current);
    try {
      await props.onPosted(threadId);
    } catch {
      update(threadId, (current) => current.clientNonce === completed.clientNonce
        ? { ...current, error: "Message sent, but the timeline couldn’t refresh. Reload to see it." }
        : current);
    } finally {
      update(threadId, (current) => current.clientNonce === completed.clientNonce
        ? { ...current, sending: false }
        : current);
    }
  };

  return (
    <div class="agent-thread-composer">
      <SharedComposer
        value={draft().content}
        onValue={(content) => update(props.threadId, (current) => ({
          ...current,
          content,
          clientNonce: crypto.randomUUID(),
          error: null,
        }))}
        label={`Message ${props.threadName}`}
        placeholder="Share the problem, a finding, or the next step…"
        disabled={!interactive() || draft().sending || props.disabled}
        submitShortcut="enter"
        onSubmit={() => void send()}
        invalid={Boolean(draft().error)}
      />
      <div class="agent-thread-composer-footer">
        <Show when={draft().error} fallback={
          <span>{props.disabled ? "This workspace is no longer active." : "Enter to send · Shift+Enter for a new line"}</span>
        }>
          {(message) => <span role="alert" class="agent-thread-error">{message()}</span>}
        </Show>
        <button
          type="button"
          class="agent-thread-button primary"
          onClick={() => void send()}
          disabled={!interactive() || draft().sending || props.disabled || !draft().content.trim()}
        >
          <Icon name={draft().sending ? "cycle" : "message"} />
          {draft().sending ? "Sending…" : "Send privately"}
        </button>
      </div>
    </div>
  );
}

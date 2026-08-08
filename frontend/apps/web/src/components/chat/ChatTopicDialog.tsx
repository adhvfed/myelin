import { Dialog, Icon } from "@myelin/design-system";
import { useAction } from "@solidjs/router";
import { createEffect, createSignal, Show } from "solid-js";

import {
  chatMutate,
  type ChatConversationReceipt,
  type ChatErrorKind,
} from "~/lib/api";

export interface ChatTopicDialogProps {
  open: boolean;
  onClose: () => void;
  onCreated: (receipt: ChatConversationReceipt) => void;
}

function errorCopy(kind: ChatErrorKind): string {
  switch (kind) {
    case "bad-input":
      return "Use a channel and topic name without surrounding whitespace.";
    case "conflict":
      return "That topic already exists in this channel.";
    case "not-found":
      return "Topic creation isn’t available to you.";
    case "unavailable":
      return "Chat is temporarily unavailable. Your topic was not confirmed.";
    default:
      return "We couldn’t confirm the topic. Check the topic list before retrying.";
  }
}

export function ChatTopicDialog(props: ChatTopicDialogProps) {
  const mutate = useAction(chatMutate);
  const [channel, setChannel] = createSignal("");
  const [topic, setTopic] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [submitting, setSubmitting] = createSignal(false);
  let channelInput: HTMLInputElement | undefined;

  createEffect(() => {
    if (!props.open) return;
    setChannel("");
    setTopic("");
    setError(null);
    setSubmitting(false);
  });

  const close = () => {
    if (!submitting()) props.onClose();
  };

  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    if (!channel().trim() || channel().trim() !== channel() ||
        !topic().trim() || topic().trim() !== topic()) {
      setError("Channel and topic are required and cannot start or end with spaces.");
      channelInput?.focus();
      return;
    }
    setSubmitting(true);
    setError(null);
    try {
      const result = await mutate({
        op: "create-conversation",
        channel: channel(),
        topic: topic(),
      });
      if (!result.ok) {
        setError(errorCopy(result.error));
        return;
      }
      if (result.op !== "create-conversation") {
        setError(errorCopy("error"));
        return;
      }
      props.onClose();
      props.onCreated(result.receipt);
    } catch {
      setError(errorCopy("error"));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog
      open={props.open}
      onClose={close}
      title="New topic"
      description="Start a focused conversation in a public channel. Everyone in this tenant can follow it."
      size="md"
      dismissable={!submitting()}
      initialFocus={() => channelInput}
      footer={
        <>
          <button type="button" class="chat-button chat-button-secondary" onClick={close} disabled={submitting()}>
            Cancel
          </button>
          <button type="submit" form="chat-topic-form" class="chat-button chat-button-primary" disabled={submitting()}>
            <Icon name={submitting() ? "cycle" : "channel"} />
            {submitting() ? "Creating…" : "Create topic"}
          </button>
        </>
      }
    >
      <form id="chat-topic-form" class="chat-topic-form" onSubmit={submit}>
        <label class="chat-field-label">
          Channel
          <input
            ref={channelInput}
            name="channel"
            value={channel()}
            onInput={(event) => {
              setChannel(event.currentTarget.value);
              setError(null);
            }}
            maxlength={255}
            autocomplete="off"
            disabled={submitting()}
            aria-invalid={Boolean(error())}
            class="chat-text-input"
            placeholder="engineering"
          />
        </label>
        <label class="chat-field-label">
          Topic
          <input
            name="topic"
            value={topic()}
            onInput={(event) => {
              setTopic(event.currentTarget.value);
              setError(null);
            }}
            maxlength={255}
            autocomplete="off"
            disabled={submitting()}
            aria-invalid={Boolean(error())}
            aria-describedby={error() ? "chat-topic-error" : "chat-topic-hint"}
            class="chat-text-input"
            placeholder="release readiness"
          />
        </label>
        <p id="chat-topic-hint" class="chat-field-hint">
          Topics keep decisions and agent work attached to the conversation that produced them.
        </p>
        <Show when={error()}>
          {(message) => <p id="chat-topic-error" role="alert" class="chat-field-error">{message()}</p>}
        </Show>
      </form>
    </Dialog>
  );
}

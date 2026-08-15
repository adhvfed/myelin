import { Dialog, Icon } from "@myelin/design-system";
import { A, useAction } from "@solidjs/router";
import { createEffect, createSignal, For, Show } from "solid-js";

import { createProjectCatalogue } from "~/components/projects/project-catalogue";
import {
  chatMutate,
  type ChatConversationReceipt,
  type ChatErrorKind,
} from "~/lib/api";

export interface ChatTopicDialogProps {
  open: boolean;
  preferredProjectId?: string;
  onClose: () => void;
  onCreated: (receipt: ChatConversationReceipt) => void;
}

function errorCopy(kind: ChatErrorKind): string {
  switch (kind) {
    case "bad-input":
      return "Check the project, channel, and topic. Names must be 1–255 bytes without surrounding whitespace or control characters.";
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
  const catalogue = createProjectCatalogue();
  const [channel, setChannel] = createSignal("");
  const [topic, setTopic] = createSignal("");
  const [clientNonce, setClientNonce] = createSignal(crypto.randomUUID());
  const [error, setError] = createSignal<string | null>(null);
  const [submitting, setSubmitting] = createSignal(false);
  let channelInput: HTMLInputElement | undefined;

  createEffect(() => {
    if (!props.open) return;
    setChannel("");
    setTopic("");
    setClientNonce(crypto.randomUUID());
    setError(null);
    setSubmitting(false);
  });

  createEffect(() => {
    const preferred = props.preferredProjectId;
    if (props.open && preferred &&
        catalogue.projects().some((project) => project.id === preferred)) {
      catalogue.select(preferred);
    }
  });

  const close = () => {
    if (!submitting()) props.onClose();
  };

  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    if (!catalogue.selectedId()) {
      setError("Choose the project whose collaborators should see this topic.");
      return;
    }
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
        projectId: catalogue.selectedId(),
        channel: channel(),
        topic: topic(),
        clientNonce: clientNonce(),
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
      description="Start a focused conversation for everyone who can access its project."
      size="md"
      dismissable={!submitting()}
      initialFocus={() => channelInput}
      footer={
        <>
          <button type="button" class="chat-button chat-button-secondary" onClick={close} disabled={submitting()}>
            Cancel
          </button>
          <button
            type="submit"
            form="chat-topic-form"
            class="chat-button chat-button-primary"
            disabled={submitting() || catalogue.loading() || catalogue.unavailable() || catalogue.empty()}
          >
            <Icon name={submitting() ? "cycle" : "channel"} />
            {submitting() ? "Creating…" : "Create topic"}
          </button>
        </>
      }
    >
      <form id="chat-topic-form" class="chat-topic-form" onSubmit={submit}>
        <Show when={catalogue.loading()}>
          <p class="chat-field-hint">Loading your projects…</p>
        </Show>
        <Show when={catalogue.unavailable()}>
          <p class="chat-field-error" role="alert">
            Projects couldn’t be loaded. <button type="button" onClick={() => void catalogue.retry()}>Try again</button>
          </p>
        </Show>
        <Show when={catalogue.empty()}>
          <p class="chat-field-hint">
            A topic belongs to a project. <A href="/issues?new=1" onClick={close}>Set up your first project</A> to continue.
          </p>
        </Show>
        <Show when={!catalogue.loading() && !catalogue.unavailable() && !catalogue.empty()}>
          <label class="chat-field-label">
            Project
            <select
              name="project"
              value={catalogue.selectedId()}
              onChange={(event) => {
                catalogue.select(event.currentTarget.value);
                setError(null);
              }}
              disabled={submitting()}
              class="chat-text-input"
            >
              <For each={catalogue.projects()}>{(project) => (
                <option value={project.id}>{project.name}</option>
              )}</For>
            </select>
          </label>
          <Show when={catalogue.nextCursor()}>
            <button
              type="button"
              class="chat-button chat-button-secondary"
              onClick={() => void catalogue.loadMore()}
              disabled={submitting() || catalogue.loadingMore()}
            >
              {catalogue.loadingMore() ? "Loading projects…" : "Load more projects"}
            </button>
          </Show>
          <Show when={catalogue.loadMoreError()}>
            <p class="chat-field-error" role="alert">More projects couldn’t be loaded. Try again.</p>
          </Show>
        </Show>
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

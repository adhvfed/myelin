import { Icon, Skeleton, SkeletonBlock } from "@myelin/design-system";
import { A } from "@solidjs/router";
import { createEffect, For, Show } from "solid-js";

import type { ChatConversation, ChatMessage, ChatMessageNode } from "~/lib/api";
import { artifactRefHref, artifactRefLabel } from "~/lib/artifact-ref";
import { chatAuthorLabel, chatTimestamp } from "~/lib/chat-view";
import { ChatComposer } from "./ChatComposer";

export interface ChatTimelineProps {
  conversation: ChatConversation;
  messages: ChatMessage[];
  loading: boolean;
  loadingEarlier: boolean;
  hasEarlier: boolean;
  onLoadEarlier: () => void;
  onPosted: () => Promise<void> | void;
}

type ChatReferenceNode = Extract<ChatMessageNode, { ref: string }>;

function ChatReference(props: { node: ChatReferenceNode }) {
  const label = () => props.node.kind === "embed"
    ? `Embedded · ${artifactRefLabel(props.node.ref)}`
    : artifactRefLabel(props.node.ref);
  const href = () => artifactRefHref(props.node.ref);
  return <Show
    when={href()}
    fallback={<span class="chat-message-node" data-kind={props.node.kind} title={props.node.ref}>
      {label()}
    </span>}
  >
    {(target) => <A
      class="chat-message-node"
      data-kind={props.node.kind}
      href={target()}
      title={props.node.ref}
    >
      {label()}
    </A>}
  </Show>;
}

function ChatInlineNode(props: { node: ChatMessageNode }) {
  const mention = () => props.node.kind === "mention" ? props.node : undefined;
  const reference = () => props.node.kind !== "mention" ? props.node : undefined;
  return <Show
    when={mention()}
    fallback={<Show when={reference()}>{(node) => <ChatReference node={node()} />}</Show>}
  >
    {(node) => <span class="chat-message-node" data-kind="mention" title={node().principal_id}>
      @{node().principal_id}
    </span>}
  </Show>;
}

function ChatMessageBody(props: { message: ChatMessage }) {
  return <p dir="auto">
    <For each={props.message.content.split("\uFFFC")}>
      {(part, index) => <>{part}<Show when={index() < props.message.nodes.length}>
        <ChatInlineNode node={props.message.nodes[index()]!} />
      </Show></>}
    </For>
  </p>;
}

export function ChatTimeline(props: ChatTimelineProps) {
  let timeline: HTMLDivElement | undefined;
  let previousCount = 0;

  createEffect(() => {
    const count = props.messages.length;
    const wasNearBottom = !timeline || previousCount === 0 ||
      timeline.scrollHeight - timeline.scrollTop - timeline.clientHeight < 140;
    previousCount = count;
    if (wasNearBottom) queueMicrotask(() => timeline?.scrollTo({ top: timeline.scrollHeight }));
  });

  return (
    <section class="chat-conversation" aria-labelledby="chat-topic-heading">
      <header class="chat-conversation-header">
        <A href="/chat" class="chat-mobile-back" aria-label="Back to topics">
          <Icon name="chevron" />
        </A>
        <div>
          <p><Icon name="channel" /> {props.conversation.channel}</p>
          <h2 id="chat-topic-heading">{props.conversation.topic}</h2>
        </div>
        <span class="chat-live-cue"><span aria-hidden="true" /> Updates automatically</span>
      </header>

      <div ref={timeline} class="chat-timeline" aria-live="polite" aria-busy={props.loading}>
        <Show when={!props.loading} fallback={
          <Skeleton label="Loading conversation…" rows={3}>
            <SkeletonBlock height="4rem" />
            <SkeletonBlock height="4rem" />
            <SkeletonBlock height="4rem" />
          </Skeleton>
        }>
          <Show when={props.hasEarlier}>
            <button
              type="button"
              class="chat-earlier-button"
              onClick={() => props.onLoadEarlier()}
              disabled={props.loadingEarlier}
            >
              <Icon name={props.loadingEarlier ? "cycle" : "chevron"} />
              {props.loadingEarlier ? "Loading earlier messages…" : "Load earlier messages"}
            </button>
          </Show>
          <Show when={props.messages.length > 0} fallback={
            <div class="chat-empty-timeline">
              <Icon name="message" size={28} />
              <h3>Start the conversation</h3>
              <p>Share context, make a decision, or bring an agent into the work.</p>
            </div>
          }>
            <ol class="chat-message-list">
              <For each={props.messages}>{(message) => (
                <li class="chat-message" classList={{ "chat-message-you": message.is_you }}>
                  <div class="chat-avatar" data-kind={message.author_kind} aria-hidden="true">
                    <Icon name={message.author_kind === "agent" ? "agent" : "human"} />
                  </div>
                  <article>
                    <header>
                      <strong>{chatAuthorLabel(message)}</strong>
                      <time datetime={message.created_at === null ? undefined : new Date(message.created_at * 1_000).toISOString()}>
                        {chatTimestamp(message.created_at)}
                      </time>
                      <Show when={message.edited}><span>edited</span></Show>
                    </header>
                    <ChatMessageBody message={message} />
                  </article>
                </li>
              )}</For>
            </ol>
          </Show>
        </Show>
      </div>

      <ChatComposer
        conversationId={props.conversation.id}
        topic={props.conversation.topic}
        onPosted={props.onPosted}
      />
    </section>
  );
}

import { Icon, Skeleton, SkeletonBlock } from "@myelin/design-system";
import { A } from "@solidjs/router";
import { createEffect, For, Show, type JSX } from "solid-js";

import type { ChatMessage, ChatMessageNode } from "~/lib/api";
import { artifactRefHref, artifactRefLabel } from "~/lib/artifact-ref";
import { chatAuthorLabel, chatTimestamp } from "~/lib/chat-view";

type ReferenceNode = Extract<ChatMessageNode, { ref: string }>;

function MessageReference(props: { node: ReferenceNode }) {
  const projection = () => props.node.card.kind === "projection" ? props.node.card : undefined;
  const label = () => projection()?.title ?? (props.node.card.kind === "tombstone"
    ? "Referenced work is not available"
    : artifactRefLabel(props.node.ref));
  const href = () => props.node.card.kind === "tombstone"
    ? undefined
    : artifactRefHref(props.node.ref, projection()?.render_hint);
  const accessibleLabel = () => {
    const card = projection();
    return card ? `${card.title}, ${card.state}` : label();
  };
  const content = () => <>
    <span>{label()}</span>
    <Show when={projection()}>{(card) => <small>{card().state}</small>}</Show>
  </>;
  return <Show
    when={href()}
    fallback={<span
      class="chat-message-node"
      data-kind={props.node.kind}
      data-card={props.node.card.kind}
    >
      {content()}
    </span>}
  >
    {(target) => <A
      class="chat-message-node"
      data-kind={props.node.kind}
      data-card={props.node.card.kind}
      aria-label={accessibleLabel()}
      href={target()}
      title={`${artifactRefLabel(props.node.ref)} · ${props.node.ref}`}
    >
      {content()}
    </A>}
  </Show>;
}

function InlineNode(props: { node: ChatMessageNode }) {
  const mention = () => props.node.kind === "mention" ? props.node : undefined;
  const reference = () => props.node.kind !== "mention" ? props.node : undefined;
  return <Show
    when={mention()}
    fallback={<Show when={reference()}>{(node) => <MessageReference node={node()} />}</Show>}
  >
    {(node) => <span class="chat-message-node" data-kind="mention" title={node().principal_id}>
      @{node().principal_id}
    </span>}
  </Show>;
}

function MessageBody(props: { message: ChatMessage }) {
  return <p dir="auto">
    <For each={props.message.content.split("\uFFFC")}>
      {(part, index) => <>{part}<Show when={index() < props.message.nodes.length}>
        <InlineNode node={props.message.nodes[index()]!} />
      </Show></>}
    </For>
  </p>;
}

export interface ConversationTimelineProps {
  headingId: string;
  header: JSX.Element;
  composer?: JSX.Element;
  messages: ChatMessage[];
  loading: boolean;
  loadingEarlier: boolean;
  hasEarlier: boolean;
  onLoadEarlier: () => void;
  emptyHeading?: string;
  emptyCopy?: string;
}

export function ConversationTimeline(props: ConversationTimelineProps) {
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
    <section class="chat-conversation" aria-labelledby={props.headingId}>
      <header class="chat-conversation-header">{props.header}</header>
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
              <h3>{props.emptyHeading ?? "Start the conversation"}</h3>
              <p>{props.emptyCopy ?? "Share context, make a decision, or bring an agent into the work."}</p>
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
                      <time datetime={message.created_at === null
                        ? undefined
                        : new Date(message.created_at * 1_000).toISOString()}>
                        {chatTimestamp(message.created_at)}
                      </time>
                      <Show when={message.edited}><span>edited</span></Show>
                    </header>
                    <MessageBody message={message} />
                  </article>
                </li>
              )}</For>
            </ol>
          </Show>
        </Show>
      </div>
      {props.composer}
    </section>
  );
}

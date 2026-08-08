import { Icon, Skeleton, SkeletonBlock } from "@myelin/design-system";
import { A } from "@solidjs/router";
import { createEffect, For, Show } from "solid-js";

import type { ChatConversation, ChatMessage } from "~/lib/api";
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
                    <p>{message.content}</p>
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

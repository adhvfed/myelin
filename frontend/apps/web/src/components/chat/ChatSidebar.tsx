import { Icon, Skeleton, SkeletonBlock } from "@myelin/design-system";
import { A } from "@solidjs/router";
import { For, Show } from "solid-js";

import type { ChatConversation, ChatErrorKind } from "~/lib/api";
import { chatHref, groupChatConversations } from "~/lib/chat-view";

export interface ChatSidebarProps {
  conversations: ChatConversation[];
  selectedId?: string;
  loading: boolean;
  interactive: boolean;
  error?: ChatErrorKind | null;
  loadingMore: boolean;
  hasMore: boolean;
  onNew: () => void;
  onLoadMore: () => void;
}

export function ChatSidebar(props: ChatSidebarProps) {
  const groups = () => groupChatConversations(props.conversations);
  return (
    <aside
      class="chat-sidebar"
      classList={{ "chat-sidebar-mobile-hidden": Boolean(props.selectedId) }}
      aria-label="Chat channels and topics"
    >
      <header class="chat-sidebar-header">
        <div>
          <p class="chat-eyebrow">Workspace</p>
          <h1><Icon name="nav-chat" /> Chat</h1>
        </div>
        <button type="button" class="chat-icon-button" onClick={() => props.onNew()} aria-label="Create a topic" disabled={!props.interactive}>
          <Icon name="channel" />
        </button>
      </header>

      <Show when={!props.loading} fallback={
        <Skeleton label="Loading channels…" rows={3}>
          <SkeletonBlock height="2.25rem" />
          <SkeletonBlock height="2.25rem" />
          <SkeletonBlock height="2.25rem" />
        </Skeleton>
      }>
        <Show when={!props.error} fallback={
          <div class="chat-sidebar-empty" role="alert">
            <Icon name="gate" size={24} />
            <strong>Topics couldn’t be loaded</strong>
            <span>{props.error === "unavailable"
              ? "Chat is temporarily unavailable."
              : "Refresh to try the topic list again."}</span>
          </div>
        }>
        <Show when={groups().length > 0} fallback={
          <div class="chat-sidebar-empty">
            <Icon name="channel" size={24} />
            <strong>No conversations yet</strong>
            <span>Create a topic for a decision, incident, or stream of work.</span>
            <button type="button" class="chat-button chat-button-primary" onClick={() => props.onNew()} disabled={!props.interactive}>
              Create the first topic
            </button>
          </div>
        }>
          <nav class="chat-channel-list" aria-label="Topics">
            <For each={groups()}>{(group) => (
              <section class="chat-channel-group">
                <h2><Icon name="channel" /> {group.channel}</h2>
                <ul>
                  <For each={group.topics}>{(topic) => (
                    <li>
                      <A
                        href={chatHref(topic.id)}
                        aria-current={props.selectedId === topic.id ? "page" : undefined}
                        data-testid="chat-topic-link"
                      >
                        <Icon name="message" />
                        <span>{topic.topic}</span>
                      </A>
                    </li>
                  )}</For>
                </ul>
              </section>
            )}</For>
          </nav>
          <Show when={props.hasMore}>
            <button
              type="button"
              class="chat-load-more"
              onClick={() => props.onLoadMore()}
              disabled={!props.interactive || props.loadingMore}
            >
              <Icon name={props.loadingMore ? "cycle" : "chevron"} />
              {props.loadingMore ? "Loading…" : "More topics"}
            </button>
          </Show>
        </Show>
        </Show>
      </Show>
    </aside>
  );
}

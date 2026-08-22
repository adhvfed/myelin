import { Icon, Skeleton, SkeletonBlock } from "@myelin/design-system";
import { A } from "@solidjs/router";
import { For, Show } from "solid-js";

import type { AgentThread } from "~/lib/agent-thread-response";

export function AgentThreadSidebar(props: {
  threads: AgentThread[];
  selectedId?: string;
  loading: boolean;
  error: boolean;
  interactive: boolean;
  hasMore: boolean;
  loadingMore: boolean;
  onLoadMore: () => void;
  onNew: () => void;
  onActivateAgent: () => void;
}) {
  return (
    <aside
      class="agent-thread-sidebar"
      classList={{ "agent-thread-sidebar-mobile-hidden": Boolean(props.selectedId) }}
      aria-label="Private agent threads"
    >
      <header class="agent-thread-sidebar-header">
        <div>
          <p>Private work</p>
          <h1><Icon name="agent" /> Agents</h1>
        </div>
        <div class="agent-thread-sidebar-actions">
          <button
            type="button"
            class="agent-thread-icon-button"
            aria-label="Activate an agent"
            disabled={!props.interactive}
            onClick={() => props.onActivateAgent()}
          >
            <Icon name="agent" />
          </button>
          <button
            type="button"
            class="agent-thread-icon-button"
            aria-label="New private thread"
            disabled={!props.interactive}
            onClick={() => props.onNew()}
          >
            <Icon name="message" />
          </button>
        </div>
      </header>
      <Show when={!props.loading} fallback={
        <Skeleton label="Loading private work…" rows={3}>
          <SkeletonBlock height="3rem" />
          <SkeletonBlock height="3rem" />
          <SkeletonBlock height="3rem" />
        </Skeleton>
      }>
        <Show when={!props.error} fallback={
          <div class="agent-thread-sidebar-empty" role="alert">
            <Icon name="gate" size={24} />
            <strong>Private work couldn’t be loaded</strong>
            <span>Refresh to try again.</span>
          </div>
        }>
          <Show when={props.threads.length > 0} fallback={
            <div class="agent-thread-sidebar-empty">
              <Icon name="agent" size={24} />
              <strong>No private agent work yet</strong>
              <span>Name a problem and keep its conversation and workspace together.</span>
            </div>
          }>
            <nav class="agent-thread-list" aria-label="Private work">
              <ul role="list">
                <For each={props.threads}>{(thread) => (
                  <li>
                    <A
                      href={`/agents?thread=${encodeURIComponent(thread.id)}`}
                      aria-current={props.selectedId === thread.id ? "page" : undefined}
                      data-testid="agent-thread-link"
                    >
                      <Icon name="message" />
                      <span>
                        <strong>{thread.name}</strong>
                        <small>{thread.workspace.state} · {thread.workspace.retention_days} days</small>
                      </span>
                    </A>
                  </li>
                )}</For>
              </ul>
            </nav>
            <Show when={props.hasMore}>
              <button
                type="button"
                class="agent-thread-load-more"
                onClick={() => props.onLoadMore()}
                disabled={props.loadingMore}
              >
                <Icon name={props.loadingMore ? "cycle" : "chevron"} />
                {props.loadingMore ? "Loading…" : "More private work"}
              </button>
            </Show>
          </Show>
        </Show>
      </Show>
    </aside>
  );
}

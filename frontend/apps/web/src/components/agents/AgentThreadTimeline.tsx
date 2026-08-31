import { Icon } from "@myelin/design-system";
import { A } from "@solidjs/router";
import { Show } from "solid-js";

import { ConversationTimeline } from "~/components/conversation/ConversationTimeline";
import type { AgentThread } from "~/lib/agent-thread-response";
import type { ChatMessage } from "~/lib/api";
import { AgentThreadComposer } from "./AgentThreadComposer";

export function AgentThreadTimeline(props: {
  thread: AgentThread;
  agentName: string;
  messages: ChatMessage[];
  loading: boolean;
  hasEarlier: boolean;
  loadingEarlier: boolean;
  onLoadEarlier: () => void;
  onPosted: (threadId: string) => Promise<void> | void;
  workspaceAvailable: boolean;
  markingReady: boolean;
  onMarkReady: () => void;
}) {
  return (
    <ConversationTimeline
      headingId="agent-thread-heading"
      header={<>
        <A href="/agents" class="chat-mobile-back" aria-label="Back to private work">
          <Icon name="chevron" />
        </A>
        <div>
          <p>
            <Icon name="agent" /> {props.thread.review_state === "ready" ? "Project review" : "Private"} with {props.agentName}
          </p>
          <h2 id="agent-thread-heading">{props.thread.name}</h2>
        </div>
        <div class="agent-thread-review-actions">
          <Show when={props.thread.review_state === "ready"} fallback={
            <>
              <span class="agent-thread-private-cue"><Icon name="gate" /> Owner and agent only</span>
              <Show when={props.thread.project_id}>
                <button
                  type="button"
                  class="agent-thread-button secondary agent-thread-review-button"
                  disabled={props.markingReady || !props.workspaceAvailable}
                  onClick={props.onMarkReady}
                  title="Make this conversation visible to current project collaborators. Workspace access stays private."
                >
                  <Icon name={props.markingReady ? "cycle" : "channel"} />
                  {props.markingReady ? "Sharing…" : "Ready for review"}
                </button>
              </Show>
            </>
          }>
            <span class="agent-thread-ready-cue">
              <Icon name="channel" /> Project collaborators can review
            </span>
          </Show>
        </div>
      </>}
      messages={props.messages}
      loading={props.loading}
      hasEarlier={props.hasEarlier}
      loadingEarlier={props.loadingEarlier}
      onLoadEarlier={props.onLoadEarlier}
      emptyHeading="Give the agent the problem"
      emptyCopy="Messages stay in this named thread so a fresh agent context can resume the same work."
      composer={<AgentThreadComposer
        threadId={props.thread.id}
        threadName={props.thread.name}
        sharedForReview={props.thread.review_state === "ready"}
        disabled={!props.workspaceAvailable}
        onPosted={props.onPosted}
      />}
    />
  );
}

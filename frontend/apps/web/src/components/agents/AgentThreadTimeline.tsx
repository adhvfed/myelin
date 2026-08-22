import { Icon } from "@myelin/design-system";
import { A } from "@solidjs/router";

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
}) {
  const ready = () => props.thread.workspace.state === "ready";
  return (
    <ConversationTimeline
      headingId="agent-thread-heading"
      header={<>
        <A href="/agents" class="chat-mobile-back" aria-label="Back to private work">
          <Icon name="chevron" />
        </A>
        <div>
          <p><Icon name="agent" /> Private with {props.agentName}</p>
          <h2 id="agent-thread-heading">{props.thread.name}</h2>
        </div>
        <span class="agent-thread-private-cue"><Icon name="gate" /> Owner and agent only</span>
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
        disabled={!ready()}
        onPosted={props.onPosted}
      />}
    />
  );
}

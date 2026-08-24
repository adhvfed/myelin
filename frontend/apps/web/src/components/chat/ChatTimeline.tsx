import { Icon } from "@myelin/design-system";
import { A } from "@solidjs/router";
import { ConversationTimeline } from "~/components/conversation/ConversationTimeline";

import type { ChatConversation, ChatMessage } from "~/lib/api";
import { ChatComposer } from "./ChatComposer";

export interface ChatTimelineProps {
  conversation: ChatConversation;
  messages: ChatMessage[];
  loading: boolean;
  loadingEarlier: boolean;
  hasEarlier: boolean;
  onLoadEarlier: () => void;
  onPosted: (conversationId: string) => Promise<void> | void;
  focusedMessageId?: string;
}

export function ChatTimeline(props: ChatTimelineProps) {
  return (
    <ConversationTimeline
      headingId="chat-topic-heading"
      header={<>
        <A href="/chat" class="chat-mobile-back" aria-label="Back to topics">
          <Icon name="chevron" />
        </A>
        <div>
          <p><Icon name="channel" /> {props.conversation.channel}</p>
          <h2 id="chat-topic-heading">{props.conversation.topic}</h2>
        </div>
        {props.focusedMessageId
          ? <A
              href={`/chat?conversation=${encodeURIComponent(props.conversation.id)}`}
              class="chat-live-cue"
            >View latest messages</A>
          : <span class="chat-live-cue"><span aria-hidden="true" /> Updates automatically</span>}
      </>}
      messages={props.messages}
      focusedMessageId={props.focusedMessageId}
      loading={props.loading}
      hasEarlier={props.hasEarlier}
      loadingEarlier={props.loadingEarlier}
      onLoadEarlier={props.onLoadEarlier}
      composer={props.focusedMessageId ? undefined : <ChatComposer
          conversationId={props.conversation.id}
          conversationRef={props.conversation.ref}
          topic={props.conversation.topic}
          onPosted={props.onPosted}
        />}
    />
  );
}

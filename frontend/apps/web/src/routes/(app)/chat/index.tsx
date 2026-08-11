import { Icon, Skeleton, SkeletonBlock, useToast } from "@myelin/design-system";
import { Title } from "@solidjs/meta";
import {
  A,
  createAsync,
  revalidate,
  useNavigate,
  useSearchParams,
} from "@solidjs/router";
import {
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  onMount,
  Show,
} from "solid-js";

import { ChatSidebar } from "~/components/chat/ChatSidebar";
import { ChatTimeline } from "~/components/chat/ChatTimeline";
import { ChatTopicDialog } from "~/components/chat/ChatTopicDialog";
import {
  getChatConversations,
  getChatMessages,
  type ChatConversationPage,
  type ChatErrorKind,
  type ChatMessagePage,
} from "~/lib/api";
import { isChatUlid } from "~/lib/chat-response";
import {
  chatErrorKind,
  chatHref,
  mergeConversationPages,
  mergeMessagePages,
} from "~/lib/chat-view";

interface PageResult<T> {
  page: T | null;
  error: ChatErrorKind | null;
}

export default function ChatIndex() {
  const [search] = useSearchParams();
  const navigate = useNavigate();
  const toast = useToast();
  const selectedId = () => typeof search.conversation === "string" ? search.conversation : undefined;
  const validSelectedId = () => isChatUlid(selectedId()) ? selectedId() : undefined;
  const createOpen = () => search.new === "1";

  const [conversationPages, setConversationPages] = createSignal<ChatConversationPage[]>([]);
  const [interactive, setInteractive] = createSignal(false);
  const [loadingConversations, setLoadingConversations] = createSignal(false);
  const [conversationContinuationError, setConversationContinuationError] = createSignal(false);
  const [earlierMessagePages, setEarlierMessagePages] = createSignal<ChatMessagePage[]>([]);
  const [loadingEarlier, setLoadingEarlier] = createSignal(false);
  const [earlierError, setEarlierError] = createSignal(false);

  const firstConversations = createAsync(async (): Promise<PageResult<ChatConversationPage>> => {
    try {
      return { page: await getChatConversations({ limit: 100 }), error: null };
    } catch (error) {
      if (error instanceof Response) throw error;
      return { page: null, error: chatErrorKind(error) };
    }
  }, { deferStream: true });

  const recentMessages = createAsync(async (): Promise<PageResult<ChatMessagePage> | null> => {
    const raw = selectedId();
    if (!raw) return null;
    if (!isChatUlid(raw)) return { page: null, error: "bad-input" };
    try {
      return {
        page: await getChatMessages({ conversationId: raw, limit: 100 }),
        error: null,
      };
    } catch (error) {
      if (error instanceof Response) throw error;
      return { page: null, error: chatErrorKind(error) };
    }
  }, { deferStream: true });

  const conversations = createMemo(() =>
    mergeConversationPages(firstConversations()?.page, conversationPages()));
  const selectedConversation = createMemo(() => {
    const id = validSelectedId();
    if (!id) return undefined;
    return recentMessages()?.page?.conversation ?? conversations().find((row) => row.id === id);
  });
  const messages = createMemo(() =>
    mergeMessagePages(recentMessages()?.page, earlierMessagePages()));
  const nextConversationCursor = () => {
    const pages = conversationPages();
    return pages.length
      ? pages[pages.length - 1]?.page.next_cursor ?? null
      : firstConversations()?.page?.page.next_cursor ?? null;
  };
  const nextMessageCursor = () => {
    const pages = earlierMessagePages();
    return pages.length
      ? pages[pages.length - 1]?.page.next_cursor ?? null
      : recentMessages()?.page?.page.next_cursor ?? null;
  };

  createEffect(() => {
    validSelectedId();
    setEarlierMessagePages([]);
    setEarlierError(false);
    setLoadingEarlier(false);
  });

  onMount(() => {
    setInteractive(true);
    const timer = window.setInterval(() => {
      const conversationId = validSelectedId();
      if (!conversationId || document.hidden) return;
      void revalidate(getChatMessages.keyFor({ conversationId, limit: 100 }));
    }, 5_000);
    onCleanup(() => window.clearInterval(timer));
  });

  const openCreate = () => {
    const current = validSelectedId();
    navigate(current ? `${chatHref(current)}&new=1` : "/chat?new=1");
  };
  const closeCreate = () => navigate(chatHref(validSelectedId()), { replace: true });

  const loadMoreConversations = async () => {
    const cursor = nextConversationCursor();
    if (!cursor || loadingConversations()) return;
    setLoadingConversations(true);
    setConversationContinuationError(false);
    try {
      const page = await getChatConversations({ cursor, limit: 100 });
      setConversationPages((pages) => [...pages, page]);
    } catch {
      setConversationContinuationError(true);
    } finally {
      setLoadingConversations(false);
    }
  };

  const loadEarlier = async () => {
    const conversationId = validSelectedId();
    const before = nextMessageCursor();
    if (!conversationId || !before || loadingEarlier()) return;
    setLoadingEarlier(true);
    setEarlierError(false);
    try {
      const page = await getChatMessages({ conversationId, before, limit: 100 });
      setEarlierMessagePages((pages) => [...pages, page]);
    } catch {
      setEarlierError(true);
    } finally {
      setLoadingEarlier(false);
    }
  };

  const refreshMessages = async () => {
    const conversationId = validSelectedId();
    if (!conversationId) return;
    await revalidate(getChatMessages.keyFor({ conversationId, limit: 100 }));
  };

  const messagePosted = async () => {
    await refreshMessages();
    toast.show({ title: "Message sent", variant: "success" });
  };

  return (
    <>
      <Title>Chat · Myelin</Title>
      <div
        class="chat-screen"
        classList={{ "chat-has-selection": Boolean(validSelectedId()) }}
        data-testid="chat-screen"
      >
        <ChatSidebar
          conversations={conversations()}
          selectedId={validSelectedId()}
          loading={firstConversations() === undefined}
          interactive={interactive()}
          error={firstConversations()?.error}
          hasMore={Boolean(nextConversationCursor())}
          loadingMore={loadingConversations()}
          onLoadMore={() => void loadMoreConversations()}
          onNew={openCreate}
        />

        <section class="chat-workspace" aria-label="Conversation">
          <Show when={conversationContinuationError()}>
            <p role="alert" class="chat-inline-error">More topics couldn’t be loaded. Try again.</p>
          </Show>
          <Show when={earlierError()}>
            <p role="alert" class="chat-inline-error">Earlier messages couldn’t be loaded. Try again.</p>
          </Show>
          <Show
            when={validSelectedId()}
            fallback={<ChatWelcome onNew={openCreate} hasTopics={conversations().length > 0} interactive={interactive()} />}
          >
            <Show
              when={recentMessages() !== undefined}
              fallback={<ChatLoading />}
            >
              <Show
                when={selectedConversation() && recentMessages()?.page}
                fallback={<ChatConversationError kind={recentMessages()?.error ?? "error"} />}
              >
                <ChatTimeline
                  conversation={selectedConversation()!}
                  messages={messages()}
                  loading={false}
                  hasEarlier={Boolean(nextMessageCursor())}
                  loadingEarlier={loadingEarlier()}
                  onLoadEarlier={() => void loadEarlier()}
                  onPosted={messagePosted}
                />
              </Show>
            </Show>
          </Show>
        </section>
      </div>

      <ChatTopicDialog
        open={createOpen()}
        preferredProjectId={selectedConversation()?.project_id}
        onClose={closeCreate}
        onCreated={(receipt) => {
          void revalidate("chat-conversations");
          setConversationPages([]);
          navigate(chatHref(receipt.conversation.id));
          toast.show({ title: `Topic created in ${receipt.conversation.channel}`, variant: "success" });
        }}
      />
    </>
  );
}

function ChatWelcome(props: { hasTopics: boolean; interactive: boolean; onNew: () => void }) {
  return (
    <div class="chat-welcome">
      <Icon name="nav-chat" size={32} />
      <p class="chat-eyebrow">Conversations with context</p>
      <h2>{props.hasTopics ? "Choose a topic" : "Bring the work together"}</h2>
      <p>
        Keep decisions, handoffs, and agent collaboration in focused topics your engineering org can follow.
      </p>
      <button type="button" class="chat-button chat-button-primary" onClick={() => props.onNew()} disabled={!props.interactive}>
        <Icon name="channel" /> New topic
      </button>
    </div>
  );
}

function ChatLoading() {
  return (
    <div class="chat-loading">
      <Skeleton label="Loading conversation…" rows={4}>
        <SkeletonBlock height="3.5rem" />
        <SkeletonBlock height="4rem" />
        <SkeletonBlock height="4rem" />
        <SkeletonBlock height="5rem" />
      </Skeleton>
    </div>
  );
}

function ChatConversationError(props: { kind: ChatErrorKind }) {
  const copy = () => {
    if (props.kind === "bad-input") return "That conversation address is invalid.";
    if (props.kind === "not-found") return "That conversation isn’t available to you.";
    if (props.kind === "unavailable") return "Chat is temporarily unavailable.";
    return "The conversation couldn’t be loaded.";
  };
  return (
    <div class="chat-conversation-error" role="alert">
      <Icon name="gate" size={28} />
      <h2>Conversation unavailable</h2>
      <p>{copy()}</p>
      <A href="/chat" class="chat-button chat-button-secondary">Back to topics</A>
    </div>
  );
}

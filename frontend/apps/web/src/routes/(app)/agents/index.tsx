import { useToast } from "@myelin/design-system";
import { Title } from "@solidjs/meta";
import { createAsync, revalidate, useNavigate, useSearchParams } from "@solidjs/router";
import {
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  onMount,
  Show,
} from "solid-js";

import { AgentActivationDialog } from "~/components/agents/AgentActivationDialog";
import { AgentThreadCreateDialog } from "~/components/agents/AgentThreadCreateDialog";
import { AgentThreadSidebar } from "~/components/agents/AgentThreadSidebar";
import {
  AgentThreadLoading,
  AgentThreadUnavailable,
  AgentThreadWelcome,
} from "~/components/agents/AgentThreadStates";
import { AgentThreadTimeline } from "~/components/agents/AgentThreadTimeline";
import { AgentWorkspaceContext } from "~/components/agents/AgentWorkspaceContext";
import { useContextPane } from "~/components/AppShell";
import {
  agentThreadErrorKind,
  getAgentChoices,
  getAgentThread,
  getAgentThreadMessages,
  getAgentThreads,
  getWorkspaceSessions,
  type AgentThreadErrorKind,
} from "~/lib/agent-thread-api";
import {
  isAgentThreadId,
  type AgentChoice,
  type AgentChoicePage,
  type AgentThread,
  type AgentThreadPage,
  type WorkspaceSessionPage,
} from "~/lib/agent-thread-response";
import { mergeMessagePages } from "~/lib/chat-view";

interface PageResult<T> {
  page: T | null;
  error: AgentThreadErrorKind | null;
}

function mergeById<T extends { id: string }>(first: T[] = [], pages: T[][] = []): T[] {
  const seen = new Set<string>();
  const merged: T[] = [];
  for (const row of first.concat(...pages)) {
    if (seen.has(row.id)) continue;
    seen.add(row.id);
    merged.push(row);
  }
  return merged;
}

export default function AgentThreadsIndex() {
  const [search] = useSearchParams();
  const navigate = useNavigate();
  const toast = useToast();
  const pane = useContextPane();
  const selectedId = () => typeof search.thread === "string" ? search.thread : undefined;
  const validSelectedId = () => isAgentThreadId(selectedId()) ? selectedId() : undefined;
  const createOpen = () => search.new === "1";
  const activationOpen = () => search.activate === "1";
  const [interactive, setInteractive] = createSignal(false);

  const [threadPages, setThreadPages] = createSignal<AgentThreadPage[]>([]);
  const [loadingMoreThreads, setLoadingMoreThreads] = createSignal(false);
  const [agentPages, setAgentPages] = createSignal<AgentChoicePage[]>([]);
  const [activatedAgents, setActivatedAgents] = createSignal<AgentChoice[]>([]);
  const [preferredAgentId, setPreferredAgentId] = createSignal<string>();
  const [loadingMoreAgents, setLoadingMoreAgents] = createSignal(false);
  const [earlierMessagePages, setEarlierMessagePages] = createSignal<Awaited<ReturnType<typeof getAgentThreadMessages>>[]>([]);
  const [loadingEarlier, setLoadingEarlier] = createSignal(false);
  const [workspaceSessionPages, setWorkspaceSessionPages] = createSignal<WorkspaceSessionPage[]>([]);
  const [loadingMoreSessions, setLoadingMoreSessions] = createSignal(false);
  let threadGeneration = 0;
  let messageGeneration = 0;
  let workspaceSessionGeneration = 0;

  const firstThreads = createAsync(async (): Promise<PageResult<AgentThreadPage>> => {
    try {
      return { page: await getAgentThreads({ limit: 100 }), error: null };
    } catch (error) {
      if (error instanceof Response) throw error;
      return { page: null, error: agentThreadErrorKind(error) };
    }
  }, { deferStream: true });

  const firstAgents = createAsync(async (): Promise<PageResult<AgentChoicePage>> => {
    try {
      return { page: await getAgentChoices({ limit: 100 }), error: null };
    } catch (error) {
      if (error instanceof Response) throw error;
      return { page: null, error: agentThreadErrorKind(error) };
    }
  }, { deferStream: true });

  const selectedThread = createAsync(async (): Promise<PageResult<AgentThread> | null> => {
    const id = selectedId();
    if (!id) return null;
    if (!isAgentThreadId(id)) return { page: null, error: "bad-input" };
    try {
      return { page: await getAgentThread(id), error: null };
    } catch (error) {
      if (error instanceof Response) throw error;
      return { page: null, error: agentThreadErrorKind(error) };
    }
  }, { deferStream: true });

  const recentMessages = createAsync(async () => {
    const id = validSelectedId();
    if (!id) return null;
    try {
      return { page: await getAgentThreadMessages({ threadId: id, limit: 100 }), error: null };
    } catch (error) {
      if (error instanceof Response) throw error;
      return { page: null, error: agentThreadErrorKind(error) };
    }
  }, { deferStream: true });

  const recentSessions = createAsync(async (): Promise<PageResult<WorkspaceSessionPage> | null> => {
    const id = validSelectedId();
    if (!id) return null;
    try {
      return { page: await getWorkspaceSessions({ threadId: id, limit: 100 }), error: null };
    } catch (error) {
      if (error instanceof Response) throw error;
      return { page: null, error: agentThreadErrorKind(error) };
    }
  }, { deferStream: true });

  const threads = createMemo(() => mergeById(
    firstThreads()?.page?.items,
    threadPages().map((page) => page.items),
  ));
  const agents = createMemo(() => mergeById(
    activatedAgents(),
    [firstAgents()?.page?.items ?? [], ...agentPages().map((page) => page.items)],
  ));
  const messages = createMemo(() => mergeMessagePages(
    recentMessages()?.page,
    earlierMessagePages(),
  ));
  const workspaceSessions = createMemo(() => mergeById(
    recentSessions()?.page?.items,
    workspaceSessionPages().map((page) => page.items),
  ));
  const agentName = () => {
    const id = selectedThread()?.page?.agent_id;
    return agents().find((agent) => agent.id === id)?.name ?? (id ? `Agent ${id.slice(0, 8)}` : "Agent");
  };
  const nextThreadCursor = () => threadPages().at(-1)?.page.next_cursor ??
    firstThreads()?.page?.page.next_cursor ?? null;
  const nextAgentCursor = () => agentPages().at(-1)?.page.next_cursor ??
    firstAgents()?.page?.page.next_cursor ?? null;
  const nextMessageCursor = () => earlierMessagePages().at(-1)?.page.next_cursor ??
    recentMessages()?.page?.page.next_cursor ?? null;
  const nextWorkspaceSessionCursor = () => workspaceSessionPages().at(-1)?.page.next_cursor ??
    recentSessions()?.page?.page.next_cursor ?? null;

  onMount(() => {
    setInteractive(true);
    // SSE is the fast path below. The slower poll keeps a silently dead stream
    // from leaving a private thread frozen indefinitely.
    const timer = window.setInterval(() => {
      const threadId = validSelectedId();
      if (!threadId || document.hidden) return;
      void revalidate(getAgentThreadMessages.keyFor({ threadId, limit: 100 }));
      void revalidate(getAgentThread.keyFor(threadId));
      void revalidate(getWorkspaceSessions.keyFor({ threadId, limit: 100 }));
    }, 30_000);
    onCleanup(() => window.clearInterval(timer));
  });
  createEffect(() => {
    validSelectedId();
    messageGeneration += 1;
    workspaceSessionGeneration += 1;
    setEarlierMessagePages([]);
    setLoadingEarlier(false);
    setWorkspaceSessionPages([]);
    setLoadingMoreSessions(false);
  });
  createEffect(() => {
    const id = validSelectedId();
    const conversationId = selectedThread()?.page?.conversation_id;
    if (!id || !conversationId || !interactive()) return;
    const source = new EventSource(
      `/api/chat/conversations/${encodeURIComponent(conversationId)}/events`,
    );
    let everOpened = false;
    const refresh = () =>
      void revalidate(getAgentThreadMessages.keyFor({ threadId: id, limit: 100 }));
    source.addEventListener("chat.message.posted", refresh);
    source.onopen = () => {
      if (everOpened) refresh();
      everOpened = true;
    };
    onCleanup(() => source.close());
  });
  createEffect(() => {
    const thread = selectedThread()?.page;
    pane.setContextPaneLabel("Agent workspace");
    pane.setContextPane(thread ? () => <AgentWorkspaceContext
      thread={thread}
      agentName={agentName()}
      sessions={workspaceSessions()}
      sessionsLoading={recentSessions() === undefined}
      sessionsUnavailable={Boolean(recentSessions()?.error)}
      sessionsHaveMore={Boolean(nextWorkspaceSessionCursor())}
      sessionsLoadingMore={loadingMoreSessions()}
      onLoadMoreSessions={() => void loadMoreWorkspaceSessions()}
    /> : null);
    onCleanup(() => pane.setContextPane(null));
  });

  const loadMoreThreads = async () => {
    const cursor = nextThreadCursor();
    if (!cursor || loadingMoreThreads()) return;
    const generation = threadGeneration;
    setLoadingMoreThreads(true);
    try {
      const page = await getAgentThreads({ cursor, limit: 100 });
      if (generation === threadGeneration) setThreadPages((pages) => [...pages, page]);
    } catch {
      toast.show({ title: "More private work couldn’t be loaded", variant: "warning" });
    } finally {
      if (generation === threadGeneration) setLoadingMoreThreads(false);
    }
  };

  const loadMoreAgents = async () => {
    const cursor = nextAgentCursor();
    if (!cursor || loadingMoreAgents()) return;
    setLoadingMoreAgents(true);
    try {
      const page = await getAgentChoices({ cursor, limit: 100 });
      setAgentPages((pages) => [...pages, page]);
    } catch {
      toast.show({ title: "More agents couldn’t be loaded", variant: "warning" });
    } finally {
      setLoadingMoreAgents(false);
    }
  };

  const loadMoreWorkspaceSessions = async () => {
    const threadId = validSelectedId();
    const cursor = nextWorkspaceSessionCursor();
    if (!threadId || !cursor || loadingMoreSessions()) return;
    const generation = workspaceSessionGeneration;
    setLoadingMoreSessions(true);
    try {
      const page = await getWorkspaceSessions({ threadId, cursor, limit: 100 });
      if (generation === workspaceSessionGeneration) {
        setWorkspaceSessionPages((pages) => [...pages, page]);
      }
    } catch {
      if (generation === workspaceSessionGeneration) {
        toast.show({ title: "More workspace entries couldn’t be loaded", variant: "warning" });
      }
    } finally {
      if (generation === workspaceSessionGeneration) setLoadingMoreSessions(false);
    }
  };

  const loadEarlier = async () => {
    const threadId = validSelectedId();
    const before = nextMessageCursor();
    if (!threadId || !before || loadingEarlier()) return;
    const generation = messageGeneration;
    setLoadingEarlier(true);
    try {
      const page = await getAgentThreadMessages({ threadId, before, limit: 100 });
      if (generation === messageGeneration) {
        setEarlierMessagePages((pages) => [...pages, page]);
      }
    } catch {
      toast.show({ title: "Earlier private messages couldn’t be loaded", variant: "warning" });
    } finally {
      if (generation === messageGeneration) setLoadingEarlier(false);
    }
  };

  const refreshMessages = async (threadId: string) => {
    await revalidate(getAgentThreadMessages.keyFor({ threadId, limit: 100 }));
    toast.show({ title: "Message sent privately", variant: "success" });
  };
  const openCreate = () => navigate(validSelectedId()
    ? `/agents?thread=${encodeURIComponent(validSelectedId()!)}&new=1`
    : "/agents?new=1");
  const closeCreate = () => navigate(validSelectedId()
    ? `/agents?thread=${encodeURIComponent(validSelectedId()!)}`
    : "/agents", { replace: true });
  const openActivation = () => navigate(validSelectedId()
    ? `/agents?thread=${encodeURIComponent(validSelectedId()!)}&activate=1`
    : "/agents?activate=1");
  const closeActivation = () => navigate(validSelectedId()
    ? `/agents?thread=${encodeURIComponent(validSelectedId()!)}`
    : "/agents", { replace: true });

  return (
    <>
      <Title>Agents · Myelin</Title>
      <div
        class="agent-thread-screen"
        classList={{ "agent-thread-has-selection": Boolean(selectedId()) }}
        data-testid="agent-thread-screen"
      >
        <AgentThreadSidebar
          threads={threads()}
          selectedId={validSelectedId()}
          loading={firstThreads() === undefined}
          error={Boolean(firstThreads()?.error)}
          interactive={interactive()}
          hasMore={Boolean(nextThreadCursor())}
          loadingMore={loadingMoreThreads()}
          onLoadMore={() => void loadMoreThreads()}
          onNew={openCreate}
          onActivateAgent={openActivation}
        />
        <section class="agent-thread-workspace" aria-label="Private agent work">
          <Show when={selectedId()} fallback={<AgentThreadWelcome onNew={openCreate} interactive={interactive()} />}>
            <Show when={selectedThread() !== undefined && recentMessages() !== undefined} fallback={<AgentThreadLoading />}>
              <Show
                when={selectedThread()?.page && recentMessages()?.page}
                fallback={<AgentThreadUnavailable kind={selectedThread()?.error ?? recentMessages()?.error ?? "error"} />}
              >
                <AgentThreadTimeline
                  thread={selectedThread()!.page!}
                  agentName={agentName()}
                  messages={messages()}
                  loading={recentMessages() === undefined}
                  hasEarlier={Boolean(nextMessageCursor())}
                  loadingEarlier={loadingEarlier()}
                  onLoadEarlier={() => void loadEarlier()}
                  onPosted={refreshMessages}
                />
              </Show>
            </Show>
          </Show>
        </section>
      </div>
      <AgentThreadCreateDialog
        open={createOpen()}
        agents={agents()}
        preferredAgentId={preferredAgentId()}
        agentsLoading={firstAgents() === undefined}
        agentsHaveMore={Boolean(nextAgentCursor())}
        agentsLoadingMore={loadingMoreAgents()}
        onLoadMoreAgents={() => void loadMoreAgents()}
        onActivateAgent={openActivation}
        onClose={closeCreate}
        onCreated={(receipt) => {
          threadGeneration += 1;
          setThreadPages([]);
          void revalidate("agent-threads");
          navigate(`/agents?thread=${encodeURIComponent(receipt.thread.id)}`);
          toast.show({ title: `Private thread started with ${receipt.thread.workspace.retention_days} days of workspace`, variant: "success" });
        }}
      />
      <AgentActivationDialog
        open={activationOpen()}
        onClose={closeActivation}
        onActivated={(receipt) => {
          setActivatedAgents((current) => mergeById([receipt.agent], [current]));
          setPreferredAgentId(receipt.agent.id);
          void revalidate(getAgentChoices.keyFor({ limit: 100 }));
          openCreate();
          toast.show({ title: `${receipt.agent.name} is ready for private work`, variant: "success" });
        }}
      />
    </>
  );
}

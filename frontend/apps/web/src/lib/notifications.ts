import { revalidate, useAction } from "@solidjs/router";
import { createResource, createSignal, type Accessor } from "solid-js";

import {
  decideAgentEffectApproval,
  decideAutomationApproval,
  getInbox,
  markInboxRead,
  type AutomationApprovalDecision,
} from "./api";
import type { InboxItem, InboxPage } from "./inbox-response";

export type InboxAvailability = "loading" | "ready" | "unavailable";

export interface InboxState {
  items: Accessor<InboxItem[]>;
  unreadCount: Accessor<number>;
  availability: Accessor<InboxAvailability>;
  hasMore: Accessor<boolean>;
  loadingMore: Accessor<boolean>;
  loadMoreError: Accessor<boolean>;
  retry: () => void;
  loadMore: () => Promise<boolean>;
  markRead: (itemId: string) => Promise<boolean>;
  decideAutomation: (
    automationId: string,
    eventId: string,
    decision: AutomationApprovalDecision,
  ) => Promise<boolean>;
  decideAgentEffect: (
    gateId: string,
    decision: AutomationApprovalDecision,
  ) => Promise<boolean>;
}

/** Load the real recipient-scoped inbox through the server-only cookie-auth gateway client. */
export function createInbox(): InboxState {
  const markReadAction = useAction(markInboxRead);
  const decideAutomationAction = useAction(decideAutomationApproval);
  const decideAgentEffectAction = useAction(decideAgentEffectApproval);
  const [failed, setFailed] = createSignal(false);
  const [morePages, setMorePages] = createSignal<InboxPage[]>([]);
  const [generation, setGeneration] = createSignal(0);
  const [loadingMore, setLoadingMore] = createSignal(false);
  const [loadMoreError, setLoadMoreError] = createSignal(false);
  const [page, { refetch }] = createResource(async () => {
    try {
      const next = await getInbox(null);
      setFailed(false);
      return next;
    } catch {
      setFailed(true);
      return undefined;
    }
  });
  const items = (): InboxItem[] => {
    const byId = new Map<string, InboxItem>();
    const loaded = page();
    const pages = loaded === undefined ? morePages() : [loaded, ...morePages()];
    for (const item of pages.flatMap((current) => current.items)) {
      if (!byId.has(item.id)) byId.set(item.id, item);
    }
    return [...byId.values()];
  };
  const unreadCount = () => items().filter((item) => item.state === "unread").length;
  const availability = (): InboxAvailability => {
    if (page.loading) return "loading";
    if (failed()) return "unavailable";
    return "ready";
  };
  const nextCursor = () => {
    const continuation = morePages().at(-1);
    return continuation === undefined
      ? page()?.page.next_cursor ?? null
      : continuation.page.next_cursor;
  };
  const hasMore = () => nextCursor() !== null;
  const reload = async (): Promise<void> => {
    setGeneration((current) => current + 1);
    setMorePages([]);
    setLoadMoreError(false);
    await revalidate(getInbox.key);
    await refetch();
  };
  const loadMore = async (): Promise<boolean> => {
    const cursor = nextCursor();
    if (cursor === null || loadingMore()) return cursor === null;
    const startedInGeneration = generation();
    setLoadingMore(true);
    setLoadMoreError(false);
    try {
      const next = await getInbox(cursor);
      setMorePages((current) => (
        generation() === startedInGeneration && nextCursor() === cursor
          ? [...current, next]
          : current
      ));
      return true;
    } catch {
      setLoadMoreError(true);
      return false;
    } finally {
      setLoadingMore(false);
    }
  };
  const markRead = async (itemId: string): Promise<boolean> => {
    const result = await markReadAction(itemId);
    if (!result.ok) return false;
    await reload();
    return true;
  };
  const decideAutomation = async (
    automationId: string,
    eventId: string,
    decision: AutomationApprovalDecision,
  ): Promise<boolean> => {
    const result = await decideAutomationAction({ automationId, eventId, decision });
    if (!result.ok) return false;
    await reload();
    return true;
  };
  const decideAgentEffect = async (
    gateId: string,
    decision: AutomationApprovalDecision,
  ): Promise<boolean> => {
    const result = await decideAgentEffectAction({ gateId, decision });
    if (!result.ok) return false;
    await reload();
    return true;
  };
  return {
    items,
    unreadCount,
    availability,
    hasMore,
    loadingMore,
    loadMoreError,
    retry: () => void reload(),
    loadMore,
    markRead,
    decideAutomation,
    decideAgentEffect,
  };
}

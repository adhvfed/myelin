import { createResource, createSignal, type Accessor } from "solid-js";

import { getInbox } from "./api";
import type { InboxItem } from "./inbox-response";

export type InboxAvailability = "loading" | "ready" | "unavailable";

export interface InboxState {
  items: Accessor<InboxItem[]>;
  unreadCount: Accessor<number>;
  availability: Accessor<InboxAvailability>;
  hasMore: Accessor<boolean>;
  retry: () => void;
}

/** Load the real recipient-scoped inbox through the server-only cookie-auth gateway client. */
export function createInbox(): InboxState {
  const [failed, setFailed] = createSignal(false);
  const [page, { refetch }] = createResource(async () => {
    try {
      const next = await getInbox();
      setFailed(false);
      return next;
    } catch {
      setFailed(true);
      return undefined;
    }
  });
  const items = () => page()?.items ?? [];
  const unreadCount = () => items().filter((item) => item.state === "unread").length;
  const availability = (): InboxAvailability => {
    if (page.loading) return "loading";
    if (failed()) return "unavailable";
    return "ready";
  };
  const hasMore = () => page()?.page.next_cursor != null;
  return { items, unreadCount, availability, hasMore, retry: () => void refetch() };
}

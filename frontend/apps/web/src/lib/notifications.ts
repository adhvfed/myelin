// The honest inbox state (R3.5, dissolves ux-ux-firstrun #3 "fake inbox"). The chrome's inbox badge +
// panel bind to REAL notification state — even when empty. No hardcoded "2 unread", no demo rows.
//
// FLOOR (named): the durable inbox endpoint (`GET /v1/inbox` + its resume-cursor SSE delivery,
// `GET /v1/inbox/events`) is not wired yet, so the honest real state is inbox-zero — the calm
// "you're all caught up". When the endpoint lands, this hook fetches the items + subscribes for live
// delivery; the badge (absent at zero) and the empty/populated panel render logic below are already
// correct against real (empty-or-not) data, so no chrome change is needed then.
//
// NOTE (OQ-4): the repo.created/repo.pushed firehose events the first-run repos screen consumes are
// NOT inbox items (your own push is not a notification) — transport is unified, content policy is
// separate. This store deliberately never ingests them.
import { createSignal, type Accessor } from "solid-js";

/** One inbox item once the durable inbox lands (subject/why-line resolve server-side, never here). */
export interface InboxItem {
  id: string;
  /** The item kind (e.g. review-requested, mention) — drives the leading glyph server-side. */
  kind: string;
  /** The already-humanised, permission-/erasure-safe title (the frontend owns no humanisation). */
  title: string;
  /** Optional deep-link target. */
  href?: string;
}

export interface InboxState {
  /** The current items (empty until the durable inbox is wired). */
  items: Accessor<InboxItem[]>;
  /** The unread count — drives the topbar badge (ABSENT when zero). */
  unreadCount: Accessor<number>;
}

/** The inbox hook the shell consumes. Empty by construction today (the honest floor above). */
export function createInbox(): InboxState {
  const [items] = createSignal<InboxItem[]>([]);
  const unreadCount = () => items().length;
  return { items, unreadCount };
}

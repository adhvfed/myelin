import { For, Match, Show, Switch, createSignal } from "solid-js";
import { Dialog, Icon, useToast } from "@myelin/design-system";

import type { AutomationApprovalDecision } from "../lib/api";
import { inboxReasonLabel, type InboxItem } from "../lib/inbox-response";
import type { InboxState } from "../lib/notifications";

interface InboxDialogProps {
  open: boolean;
  onClose: () => void;
  inbox: InboxState;
}

interface PendingMutation {
  itemId: string;
  verb: "read" | AutomationApprovalDecision;
}

export function InboxDialog(props: InboxDialogProps) {
  const toast = useToast();
  const [pending, setPending] = createSignal<PendingMutation | null>(null);
  const [mutationError, setMutationError] = createSignal<string | null>(null);

  const runMutation = async (
    item: InboxItem,
    verb: PendingMutation["verb"],
    operation: (inbox: InboxState) => Promise<boolean>,
  ): Promise<void> => {
    if (pending() !== null) return;
    setPending({ itemId: item.id, verb });
    setMutationError(null);
    try {
      if (!await operation(props.inbox)) {
        setMutationError(
          "We couldn’t save that change. It may already have been handled; trying again is safe.",
        );
        toast.show({ title: "Inbox change wasn’t saved", variant: "danger" });
        return;
      }
      if (verb !== "read") {
        toast.show({
          title: verb === "approve" ? "Approval saved" : "Rejection saved",
          variant: "success",
        });
      }
    } catch {
      setMutationError(
        "We couldn’t reach Myelin to save that change. Check your connection and try again.",
      );
      toast.show({ title: "Inbox change wasn’t saved", variant: "danger" });
    } finally {
      setPending(null);
    }
  };

  const decide = async (
    item: InboxItem,
    decision: AutomationApprovalDecision,
  ): Promise<void> => {
    const action = item.action;
    if (action === null) return;
    await runMutation(item, decision, (inbox) => (
      action.kind === "agent_effect_approval"
        ? inbox.decideAgentEffect(action.gate_id, decision)
        : inbox.decideAutomation(action.automation_id, action.event_id, decision)
    ));
  };

  const isBusy = (item: InboxItem, verb: PendingMutation["verb"]): boolean => {
    const active = pending();
    return active?.itemId === item.id && active.verb === verb;
  };

  return (
    <Dialog
      open={props.open}
      onClose={props.onClose}
      title="Inbox"
      description="Things that need you land here."
      size="sm"
    >
      <Switch>
        <Match when={props.inbox.availability() === "loading"}>
          <InboxStateMessage testId="inbox-loading" title="Loading your inbox…" />
        </Match>
        <Match when={props.inbox.availability() === "unavailable"}>
          <div role="alert" data-testid="inbox-unavailable" class="inbox-state">
            <Icon name="inbox" />
            <strong>Inbox unavailable</strong>
            <span>We couldn&rsquo;t confirm your notifications. Nothing has been marked as read.</span>
            <button type="button" class="inbox-action" onClick={() => props.inbox.retry()}>
              Try again
            </button>
          </div>
        </Match>
        <Match when={props.inbox.items().length === 0}>
          <InboxStateMessage
            testId="inbox-empty"
            title="You’re all caught up"
            detail="When a pull request needs your review or someone mentions you, it’ll show up here."
          />
        </Match>
        <Match when={true}>
          <div
            class="inbox-content"
            aria-busy={pending() !== null || props.inbox.loadingMore()}
          >
            <Show when={mutationError()}>
              {(message) => (
                <p role="alert" class="inbox-mutation-error" data-testid="inbox-mutation-error">
                  <Icon name="check-fail" /> {message()}
                </p>
              )}
            </Show>
            <ul class="inbox-list">
              <For each={props.inbox.items()}>
                {(item) => (
                  <li class="inbox-item">
                    <Icon name="inbox" />
                    <span class="inbox-item-copy">
                      <strong>
                        {inboxReasonLabel(item.reason)}
                        <Show when={item.coalesce_count > 1}> · {item.coalesce_count} events</Show>
                      </strong>
                      <code>{item.subject}</code>
                      <Show when={item.state === "unread"}>
                        <button
                          type="button"
                          class="inbox-action"
                          disabled={pending() !== null}
                          onClick={() => void runMutation(
                            item,
                            "read",
                            (inbox) => inbox.markRead(item.id),
                          )}
                        >
                          {isBusy(item, "read") ? "Marking read…" : "Mark read"}
                        </button>
                      </Show>
                      <Show when={item.state !== "done" && item.action !== null}>
                        <span class="inbox-decision-actions">
                          <button
                            type="button"
                            class="inbox-action inbox-action-primary"
                            disabled={pending() !== null}
                            onClick={() => void decide(item, "approve")}
                          >
                            {isBusy(item, "approve") ? "Approving…" : "Approve"}
                          </button>
                          <button
                            type="button"
                            class="inbox-action"
                            disabled={pending() !== null}
                            onClick={() => void decide(item, "reject")}
                          >
                            {isBusy(item, "reject") ? "Rejecting…" : "Reject"}
                          </button>
                        </span>
                      </Show>
                    </span>
                  </li>
                )}
              </For>
            </ul>
            <Show when={props.inbox.hasMore()}>
              <Show when={props.inbox.loadMoreError()}>
                <p role="alert" class="inbox-mutation-error" data-testid="inbox-more-error">
                  <Icon name="check-fail" /> We couldn&rsquo;t load more notifications. Trying again is safe.
                </p>
              </Show>
              <button
                type="button"
                class="inbox-action"
                disabled={pending() !== null || props.inbox.loadingMore()}
                onClick={() => void props.inbox.loadMore()}
              >
                {props.inbox.loadingMore() ? "Loading more…" : "Load more"}
              </button>
            </Show>
          </div>
        </Match>
      </Switch>
    </Dialog>
  );
}

function InboxStateMessage(props: { testId: string; title: string; detail?: string }) {
  return (
    <div role="status" data-testid={props.testId} class="inbox-state">
      <Icon name="inbox" />
      <strong>{props.title}</strong>
      <Show when={props.detail}>{(detail) => <span>{detail()}</span>}</Show>
    </div>
  );
}

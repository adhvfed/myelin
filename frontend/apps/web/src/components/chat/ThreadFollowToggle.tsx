import { Icon } from "@myelin/design-system";
import { revalidate, useAction } from "@solidjs/router";
import { createEffect, createSignal, on, onMount, Show, untrack } from "solid-js";

import { chatMutate, getChatThread } from "~/lib/api";

const HINT = "Following controls whether new replies in this thread notify you.";

/** Optimistic follow/quiet switch for one thread; rolls back and says so when the edge refuses. */
export function ThreadFollowToggle(props: { rootMessageId: string; following: boolean }) {
  const mutate = useAction(chatMutate);
  const [following, setFollowing] = createSignal(untrack(() => props.following));
  const [pending, setPending] = createSignal(false);
  const [failed, setFailed] = createSignal(false);
  const [interactive, setInteractive] = createSignal(false);
  const hintId = () => `chat-follow-hint-${props.rootMessageId}`;

  onMount(() => setInteractive(true));
  createEffect(on(() => props.following, (value) => {
    if (!pending()) setFollowing(value);
  }, { defer: true }));

  const toggle = async () => {
    if (pending()) return;
    const previous = following();
    setPending(true);
    setFailed(false);
    setFollowing(!previous);
    try {
      const result = await mutate({
        op: "set-thread-follow",
        rootMessageId: props.rootMessageId,
        following: !previous,
      });
      if (!result.ok || result.op !== "set-thread-follow") throw new Error("follow rejected");
      setFollowing(result.receipt.following);
      void Promise.resolve(
        revalidate(getChatThread.keyFor({ rootMessageId: props.rootMessageId, limit: 100 })),
      ).catch(() => undefined);
    } catch {
      setFollowing(previous);
      setFailed(true);
    } finally {
      setPending(false);
    }
  };

  return (
    <div class="chat-follow">
      <button
        type="button"
        class="chat-button chat-button-secondary chat-follow-button"
        aria-pressed={following()}
        aria-describedby={hintId()}
        aria-busy={pending()}
        title={HINT}
        disabled={!interactive() || pending()}
        onClick={() => void toggle()}
      >
        <Show when={following()}><Icon name="check-pass" /></Show>
        {following() ? "Following" : "Follow thread"}
      </button>
      <span id={hintId()} class="sr-only">{HINT}</span>
      <Show when={failed()}>
        <p role="alert" class="chat-follow-error">Couldn’t update this thread. Try again.</p>
      </Show>
    </div>
  );
}

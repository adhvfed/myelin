import { Icon, Skeleton, SkeletonBlock } from "@myelin/design-system";

import type { AgentThreadErrorKind } from "~/lib/agent-thread-api";

export function AgentThreadWelcome(props: { interactive: boolean; onNew: () => void }) {
  return (
    <div class="agent-thread-welcome">
      <Icon name="agent" size={32} />
      <p class="agent-thread-eyebrow">Focused, private, resumable</p>
      <h2>Keep one problem with one agent</h2>
      <p>Name the work, share context privately, and return to the same bounded workspace from a fresh context.</p>
      <button type="button" class="agent-thread-button primary" disabled={!props.interactive} onClick={() => props.onNew()}>
        <Icon name="message" /> Start private thread
      </button>
    </div>
  );
}

export function AgentThreadLoading() {
  return (
    <div class="agent-thread-loading">
      <Skeleton label="Loading private work…" rows={4}>
        <SkeletonBlock height="3.5rem" />
        <SkeletonBlock height="4rem" />
        <SkeletonBlock height="4rem" />
        <SkeletonBlock height="5rem" />
      </Skeleton>
    </div>
  );
}

export function AgentThreadUnavailable(props: { kind: AgentThreadErrorKind }) {
  const copy = () => {
    if (props.kind === "bad-input") return "That private-thread address is invalid.";
    if (props.kind === "not-found") return "That private thread isn’t available to you.";
    if (props.kind === "unavailable") return "Private agent work is temporarily unavailable.";
    return "The private thread couldn’t be loaded.";
  };
  return (
    <div class="agent-thread-welcome" role="alert">
      <Icon name="gate" size={28} />
      <h2>Private work unavailable</h2>
      <p>{copy()}</p>
      <a href="/agents" class="agent-thread-button secondary">Back to private work</a>
    </div>
  );
}

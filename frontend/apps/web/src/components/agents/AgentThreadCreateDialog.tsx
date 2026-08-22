import { Dialog, Icon } from "@myelin/design-system";
import { useAction } from "@solidjs/router";
import { createEffect, createSignal, For, onCleanup, Show, untrack } from "solid-js";

import {
  mutateAgentThread,
  type AgentThreadErrorKind,
} from "~/lib/agent-thread-api";
import type { AgentChoice, AgentThreadCreateReceipt } from "~/lib/agent-thread-response";

function errorCopy(kind: AgentThreadErrorKind): string {
  if (kind === "bad-input") return "Use a clean name and choose an active external agent.";
  if (kind === "not-found") return "That agent is no longer available.";
  if (kind === "conflict") return "You already have a live private thread with that name.";
  if (kind === "unavailable") return "Private work is temporarily unavailable. This draft is safe to retry.";
  return "We couldn’t confirm the new thread. This draft keeps its retry identity until you edit it.";
}

export function AgentThreadCreateDialog(props: {
  open: boolean;
  agents: AgentChoice[];
  preferredAgentId?: string;
  agentsLoading: boolean;
  agentsHaveMore: boolean;
  agentsLoadingMore: boolean;
  onLoadMoreAgents: () => void;
  onActivateAgent: () => void;
  onClose: () => void;
  onCreated: (receipt: AgentThreadCreateReceipt) => void;
}) {
  const mutate = useAction(mutateAgentThread);
  const [name, setName] = createSignal("");
  const [agentId, setAgentId] = createSignal("");
  const [retentionDays, setRetentionDays] = createSignal(3);
  const [clientNonce, setClientNonce] = createSignal(crypto.randomUUID());
  const [submitting, setSubmitting] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  let nameInput: HTMLInputElement | undefined;
  let openingGeneration = 0;

  const available = () => props.agents.filter((agent) =>
    agent.status === "active" && agent.runtime_ref === "external:mcp");

  createEffect(() => {
    openingGeneration += 1;
    if (!props.open) return;
    setName("");
    const preferred = untrack(() => props.preferredAgentId);
    const canUsePreferred = untrack(() => available().some((agent) => agent.id === preferred));
    setAgentId(canUsePreferred && preferred ? preferred : "");
    setRetentionDays(3);
    setClientNonce(crypto.randomUUID());
    setSubmitting(false);
    setError(null);
  });
  onCleanup(() => { openingGeneration += 1; });

  const edit = () => {
    setClientNonce(crypto.randomUUID());
    setError(null);
  };
  const close = () => { if (!submitting()) props.onClose(); };
  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    if (!name().trim() || name().trim() !== name() || !agentId()) {
      setError("A clean problem name and an active external agent are required.");
      nameInput?.focus();
      return;
    }
    const generation = openingGeneration;
    setSubmitting(true);
    setError(null);
    try {
      const result = await mutate({
        op: "create",
        name: name(),
        agentId: agentId(),
        retentionDays: retentionDays(),
        clientNonce: clientNonce(),
      });
      if (generation !== openingGeneration) return;
      if (!result.ok || result.op !== "create") {
        setError(errorCopy(result.ok ? "error" : result.error));
        return;
      }
      props.onClose();
      props.onCreated(result.receipt);
    } catch {
      if (generation === openingGeneration) setError(errorCopy("error"));
    } finally {
      if (generation === openingGeneration) setSubmitting(false);
    }
  };

  return (
    <Dialog
      open={props.open}
      onClose={close}
      title="Start private agent thread"
      description="Keep one named problem, its conversation, and its workspace together. Only you and the chosen agent can enter."
      size="lg"
      dismissable={!submitting()}
      initialFocus={() => nameInput}
      footer={<>
        <button type="button" class="agent-thread-button secondary" onClick={close} disabled={submitting()}>
          Cancel
        </button>
        <button type="submit" form="agent-thread-create" class="agent-thread-button primary" disabled={submitting() || available().length === 0}>
          <Icon name={submitting() ? "cycle" : "agent"} />
          {submitting() ? "Starting…" : "Start thread"}
        </button>
      </>}
    >
      <form id="agent-thread-create" class="agent-thread-create" onSubmit={submit}>
        <label>
          Problem name
          <input
            ref={nameInput}
            value={name()}
            maxlength={80}
            autocomplete="off"
            placeholder="Investigate checkout race"
            disabled={submitting()}
            aria-invalid={Boolean(error())}
            onInput={(event) => { setName(event.currentTarget.value); edit(); }}
          />
        </label>
        <label>
          Agent
          <select
            value={agentId()}
            disabled={submitting() || props.agentsLoading}
            onChange={(event) => { setAgentId(event.currentTarget.value); edit(); }}
          >
            <option value="">Choose an agent</option>
            <For each={available()}>{(agent) => <option value={agent.id}>{agent.name}</option>}</For>
          </select>
        </label>
        <Show when={!props.agentsLoading && available().length === 0}>
          <div class="agent-thread-empty-choice">
            <p class="agent-thread-note">No active external agents are available.</p>
            <button
              type="button"
              class="agent-thread-button secondary"
              onClick={() => props.onActivateAgent()}
            >
              <Icon name="agent" /> Activate an agent
            </button>
          </div>
        </Show>
        <Show when={props.agentsHaveMore}>
          <button
            type="button"
            class="agent-thread-load-more"
            disabled={props.agentsLoadingMore}
            onClick={() => props.onLoadMoreAgents()}
          >
            <Icon name={props.agentsLoadingMore ? "cycle" : "chevron"} />
            {props.agentsLoadingMore ? "Loading more agents…" : "Load more agents"}
          </button>
        </Show>
        <label>
          Keep workspace for
          <select
            value={retentionDays()}
            disabled={submitting()}
            onChange={(event) => { setRetentionDays(Number(event.currentTarget.value)); edit(); }}
          >
            <option value="1">1 day</option>
            <option value="3">3 days</option>
            <option value="7">7 days</option>
            <option value="14">14 days</option>
            <option value="30">30 days</option>
          </select>
        </label>
        <p class="agent-thread-note">
          A fresh agent context can reopen this conversation and the same workspace until it expires.
        </p>
        <Show when={error()}>{(message) => <p role="alert" class="agent-thread-error">{message()}</p>}</Show>
      </form>
    </Dialog>
  );
}

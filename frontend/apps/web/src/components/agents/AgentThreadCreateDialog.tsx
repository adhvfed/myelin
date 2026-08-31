import { Dialog, Icon } from "@myelin/design-system";
import { useAction } from "@solidjs/router";
import { createEffect, createSignal, For, onCleanup, Show, untrack } from "solid-js";

import {
  mutateAgentThread,
  type AgentThreadErrorKind,
} from "~/lib/agent-thread-api";
import { createProjectCatalogue } from "~/components/projects/project-catalogue";
import type { AgentChoice, AgentThreadCreateReceipt } from "~/lib/agent-thread-response";

function errorCopy(kind: AgentThreadErrorKind): string {
  if (kind === "bad-input") return "Use a clean name and choose an active external agent.";
  if (kind === "not-found") return "That agent or repository is no longer available to you.";
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
  const catalogue = createProjectCatalogue();
  const [name, setName] = createSignal("");
  const [agentId, setAgentId] = createSignal("");
  const [projectId, setProjectId] = createSignal<string>();
  const [retentionDays, setRetentionDays] = createSignal(3);
  const [repository, setRepository] = createSignal("");
  const [baseRef, setBaseRef] = createSignal("main");
  const [branch, setBranch] = createSignal("");
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
    setProjectId(undefined);
    setRetentionDays(3);
    setRepository("");
    setBaseRef("main");
    setBranch("");
    setClientNonce(crypto.randomUUID());
    setSubmitting(false);
    setError(null);
  });
  createEffect(() => {
    if (props.open && projectId() === undefined && !catalogue.loading()) {
      setProjectId(catalogue.projects()[0]?.id ?? "");
    }
  });
  onCleanup(() => { openingGeneration += 1; });

  const edit = () => {
    setClientNonce(crypto.randomUUID());
    setError(null);
  };
  const close = () => { if (!submitting()) props.onClose(); };
  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    if (!name().trim() || name().trim() !== name() || !agentId() ||
        repository().trim() !== repository() ||
        (repository() !== "" && (!baseRef() || baseRef().trim() !== baseRef() ||
          branch().trim() !== branch()))) {
      setError("Use a clean problem name, active external agent, and valid repository fields.");
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
        projectId: projectId() ?? "",
        retentionDays: retentionDays(),
        repository: repository(),
        baseRef: baseRef(),
        branch: branch(),
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
      description="Keep one named problem, its conversation, and its workspace together privately. You decide when project collaborators can review it."
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
          Project for eventual review <span aria-hidden="true">(optional)</span>
          <select
            value={projectId() ?? ""}
            disabled={submitting() || catalogue.loading()}
            onChange={(event) => { setProjectId(event.currentTarget.value); edit(); }}
          >
            <option value="">Personal only</option>
            <For each={catalogue.projects()}>{(project) => (
              <option value={project.id}>{project.name}</option>
            )}</For>
          </select>
        </label>
        <Show when={catalogue.loading()}>
          <p class="agent-thread-note">Loading projects… You can still start personal-only work.</p>
        </Show>
        <Show when={catalogue.unavailable()}>
          <p class="agent-thread-note">
            Projects couldn’t be loaded. This thread will remain personal-only.
            {" "}<button type="button" onClick={() => void catalogue.retry()}>Try again</button>
          </p>
        </Show>
        <Show when={catalogue.nextCursor()}>
          <button
            type="button"
            class="agent-thread-load-more"
            disabled={submitting() || catalogue.loadingMore()}
            onClick={() => void catalogue.loadMore()}
          >
            <Icon name={catalogue.loadingMore() ? "cycle" : "chevron"} />
            {catalogue.loadingMore() ? "Loading more projects…" : "Load more projects"}
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
        <label>
          Repository slug <span aria-hidden="true">(optional)</span>
          <input
            value={repository()}
            maxlength={255}
            autocomplete="off"
            placeholder="platform/api"
            disabled={submitting()}
            onInput={(event) => { setRepository(event.currentTarget.value); edit(); }}
          />
        </label>
        <Show when={repository() !== ""}>
          <div class="agent-thread-create-repository">
            <label>
              Start from branch
              <input
                value={baseRef()}
                maxlength={1024}
                autocomplete="off"
                placeholder="main"
                disabled={submitting()}
                onInput={(event) => { setBaseRef(event.currentTarget.value); edit(); }}
              />
            </label>
            <label>
              Private branch <span aria-hidden="true">(generated if blank)</span>
              <input
                value={branch()}
                maxlength={1024}
                autocomplete="off"
                placeholder="myelin/thread-…"
                disabled={submitting()}
                onInput={(event) => { setBranch(event.currentTarget.value); edit(); }}
              />
            </label>
          </div>
        </Show>
        <p class="agent-thread-note">
          A fresh agent context can reopen this conversation and the same workspace until it expires.
          Add a Myelin repository to begin from an exact commit on a private branch. Project work
          stays private until you explicitly mark it ready for review.
        </p>
        <Show when={error()}>{(message) => <p role="alert" class="agent-thread-error">{message()}</p>}</Show>
      </form>
    </Dialog>
  );
}

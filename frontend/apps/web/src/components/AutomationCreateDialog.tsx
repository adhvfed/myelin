import { Dialog, Icon } from "@myelin/design-system";
import { A, useAction } from "@solidjs/router";
import { createEffect, createSignal, For, Show, untrack } from "solid-js";

import { createAutomation, type AutomationErrorKind } from "~/lib/automation-api";
import type { AutomationCreateReceipt } from "~/lib/automation-response";
import type { AgentChoice } from "~/lib/agent-thread-response";

function errorCopy(kind: AutomationErrorKind): string {
  if (kind === "bad-input") return "Use a registered event, hosted agent, bounded task, budget, and firing limit.";
  if (kind === "not-found") return "That hosted agent is no longer available to your account.";
  if (kind === "conflict") return "The retained request conflicts with another automation. Edit any field to start a fresh request.";
  if (kind === "unavailable") return "Automation creation is temporarily unavailable. Your draft is safe to retry.";
  return "Myelin couldn’t confirm this automation. Your draft and retry identity are still here.";
}

export function AutomationCreateDialog(props: {
  open: boolean;
  agents: AgentChoice[];
  agentsLoading: boolean;
  agentsHaveMore: boolean;
  agentsLoadingMore: boolean;
  agentsUnavailable: boolean;
  earlierAgentsUnavailable: boolean;
  onLoadMoreAgents: () => void;
  onRetryAgents: () => void;
  onClose: () => void;
  onCreated: (receipt: AutomationCreateReceipt) => void;
}) {
  const mutate = useAction(createAutomation);
  const [eventType, setEventType] = createSignal("ci.run.failed");
  const [agentId, setAgentId] = createSignal("");
  const [task, setTask] = createSignal("");
  const [budgetMinorUnits, setBudgetMinorUnits] = createSignal(250_000);
  const [maxFirings, setMaxFirings] = createSignal(10);
  const [requireApproval, setRequireApproval] = createSignal(true);
  const [clientNonce, setClientNonce] = createSignal(crypto.randomUUID());
  const [submitting, setSubmitting] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  let taskInput: HTMLTextAreaElement | undefined;

  createEffect(() => {
    if (!props.open) return;
    const agents = untrack(() => props.agents);
    setEventType("ci.run.failed");
    setAgentId(agents.length === 1 ? agents[0]!.id : "");
    setTask("");
    setBudgetMinorUnits(250_000);
    setMaxFirings(10);
    setRequireApproval(true);
    setClientNonce(crypto.randomUUID());
    setSubmitting(false);
    setError(null);
  });

  const edit = () => {
    setClientNonce(crypto.randomUUID());
    setError(null);
  };
  const close = () => { if (!submitting()) props.onClose(); };
  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    if (submitting()) return;
    setSubmitting(true);
    setError(null);
    try {
      const result = await mutate({
        eventType: eventType(),
        agentId: agentId(),
        task: task(),
        budgetMinorUnits: budgetMinorUnits(),
        maxFirings: maxFirings(),
        requireHumanApproval: requireApproval(),
        clientNonce: clientNonce(),
      });
      if (!result.ok) {
        setError(errorCopy(result.error));
        return;
      }
      props.onCreated(result.receipt);
      props.onClose();
    } catch {
      setError(errorCopy("error"));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog
      open={props.open}
      onClose={close}
      title="Create governed automation"
      description="Wake one hosted agent from a platform event with an explicit budget, firing limit, and approval rule."
      size="md"
      dismissable={!submitting()}
      initialFocus={() => taskInput}
      footer={<>
        <button type="button" class="automation-button automation-button-secondary" onClick={close} disabled={submitting()}>
          Cancel
        </button>
        <button type="submit" form="automation-create" class="automation-button automation-button-primary" disabled={submitting() || props.agentsLoading || props.agents.length === 0}>
          <Icon name={submitting() ? "cycle" : "run"} />
          {submitting() ? "Creating…" : "Create automation"}
        </button>
      </>}
    >
      <form id="automation-create" class="agent-thread-create" onSubmit={submit}>
        <Show when={!props.agentsLoading && props.agents.length === 0 && props.agentsUnavailable}>
          <div class="automation-empty" role="alert">
            <strong>Hosted agents couldn’t be loaded</strong>
            <p>Retry the personal agent catalogue without losing this automation draft.</p>
            <button type="button" class="automation-button automation-button-secondary" onClick={() => props.onRetryAgents()}>
              <Icon name="cycle" /> Try again
            </button>
          </div>
        </Show>
        <Show when={!props.agentsLoading && props.agents.length === 0 && !props.agentsUnavailable && !props.agentsHaveMore}>
          <div class="automation-empty">
            <strong>No active hosted agent is ready</strong>
            <p>Activate a named agent with the Hosted Myelin runtime first.</p>
            <A href="/agents?activate=1" class="automation-button automation-button-secondary">
              <Icon name="agent" /> Activate agent
            </A>
          </div>
        </Show>
        <Show when={!props.agentsLoading && props.agents.length === 0 && !props.agentsUnavailable && props.agentsHaveMore}>
          <div class="automation-empty">
            <strong>No hosted agent in the newest results</strong>
            <p>Continue through your private agent catalogue to find an earlier active agent.</p>
          </div>
        </Show>
        <Show when={props.agents.length > 0}>
          <label>
            Run as hosted agent
            <select
              value={agentId()}
              disabled={submitting()}
              onChange={(event) => { setAgentId(event.currentTarget.value); edit(); }}
            >
              <option value="">Choose an agent</option>
              <For each={props.agents}>{(agent) => (
                <option value={agent.id}>{agent.name}</option>
              )}</For>
            </select>
          </label>
          <label>
            Platform event
            <input
              value={eventType()}
              maxlength={255}
              autocomplete="off"
              list="automation-event-suggestions"
              disabled={submitting()}
              onInput={(event) => { setEventType(event.currentTarget.value); edit(); }}
            />
            <datalist id="automation-event-suggestions">
              <option value="ci.run.failed" />
              <option value="issue.issue.updated" />
            </datalist>
          </label>
          <label>
            Agent task
            <textarea
              ref={taskInput}
              value={task()}
              rows={5}
              maxlength={4096}
              placeholder="Read the failed run and open one focused issue with the evidence."
              disabled={submitting()}
              onInput={(event) => { setTask(event.currentTarget.value); edit(); }}
            />
          </label>
          <div class="agent-thread-create-grid">
            <label>
              Budget per firing · minor units
              <input
                type="number"
                min="1"
                max="1000000000000"
                step="1"
                value={budgetMinorUnits()}
                disabled={submitting()}
                onInput={(event) => { setBudgetMinorUnits(event.currentTarget.valueAsNumber); edit(); }}
              />
            </label>
            <label>
              Maximum firings
              <input
                type="number"
                min="1"
                max="1000000"
                step="1"
                value={maxFirings()}
                disabled={submitting()}
                onInput={(event) => { setMaxFirings(event.currentTarget.valueAsNumber); edit(); }}
              />
            </label>
          </div>
          <label class="agent-thread-check">
            <input
              type="checkbox"
              checked={requireApproval()}
              disabled={submitting()}
              onChange={(event) => { setRequireApproval(event.currentTarget.checked); edit(); }}
            />
            Require a human approval before every firing
          </label>
          <p class="agent-thread-note">
            Personal-data events are refused. The hosted run receives only this agent’s selected
            tools under your current authority ceiling.
          </p>
        </Show>
        <Show when={!props.agentsUnavailable && props.agentsHaveMore}>
          <button
            type="button"
            class="agent-thread-load-more"
            disabled={submitting() || props.agentsLoadingMore}
            onClick={() => props.onLoadMoreAgents()}
          >
            <Icon name={props.agentsLoadingMore ? "cycle" : "chevron"} />
            {props.agentsLoadingMore ? "Loading earlier agents…" : "Load earlier agents"}
          </button>
        </Show>
        <Show when={props.earlierAgentsUnavailable}>
          <p class="agent-thread-note" role="alert">
            Earlier agents couldn’t be loaded. {" "}
            <button type="button" onClick={() => props.onLoadMoreAgents()}>Try again</button>
          </p>
        </Show>
        <Show when={error()}>{(message) => (
          <p role="alert" class="automation-mutation-error">{message()}</p>
        )}</Show>
      </form>
    </Dialog>
  );
}

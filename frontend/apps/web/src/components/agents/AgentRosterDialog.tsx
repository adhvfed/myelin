import {
  ConfirmDialog,
  Dialog,
  Icon,
  Skeleton,
  SkeletonBlock,
} from "@myelin/design-system";
import { useAction } from "@solidjs/router";
import { For, Show, createSignal } from "solid-js";

import { mutateAgentThread } from "~/lib/agent-thread-api";
import type { AgentChoice, AgentLifecycleReceipt } from "~/lib/agent-thread-response";

type LifecycleAction = AgentLifecycleReceipt["action"];

export function AgentRosterDialog(props: {
  open: boolean;
  agents: AgentChoice[];
  viewerPrincipalId: string;
  loading: boolean;
  unavailable: boolean;
  hasMore: boolean;
  loadingMore: boolean;
  onLoadMore: () => void;
  onActivate: () => void;
  onConfigure: (agent: AgentChoice) => void;
  onChanged: (receipt: AgentLifecycleReceipt) => void;
  onClose: () => void;
}) {
  const mutate = useAction(mutateAgentThread);
  const [pending, setPending] = createSignal<{ agentId: string; action: LifecycleAction } | null>(null);
  const [retry, setRetry] = createSignal<{
    agentId: string;
    action: LifecycleAction;
    nonce: string;
  } | null>(null);
  const [errorAgentId, setErrorAgentId] = createSignal<string | null>(null);
  const [retiringAgent, setRetiringAgent] = createSignal<AgentChoice | null>(null);
  const ownedAgents = () => props.agents.filter((agent) =>
    agent.owner_principal_id === props.viewerPrincipalId);
  const isPending = (agent: AgentChoice, action?: LifecycleAction) =>
    pending()?.agentId === agent.id && (action === undefined || pending()?.action === action);
  const close = () => {
    if (pending()) return;
    setRetiringAgent(null);
    props.onClose();
  };
  const applyLifecycle = async (agent: AgentChoice, action: LifecycleAction) => {
    if (pending()) return;
    const retained = retry();
    const attempt = retained?.agentId === agent.id && retained.action === action
      ? retained
      : { agentId: agent.id, action, nonce: crypto.randomUUID() };
    setRetry(attempt);
    setPending({ agentId: agent.id, action });
    setErrorAgentId(null);
    try {
      const result = await mutate({
        op: "lifecycle-agent",
        agentId: agent.id,
        action,
        clientNonce: attempt.nonce,
      });
      if (!result.ok || result.op !== "lifecycle-agent") {
        setErrorAgentId(agent.id);
        return;
      }
      setRetry(null);
      setRetiringAgent(null);
      props.onChanged(result.receipt);
    } catch {
      setErrorAgentId(agent.id);
    } finally {
      setPending(null);
    }
  };

  return (
    <>
      <Dialog
        open={props.open}
        onClose={close}
        title="My named agents"
        description="Configure private workspace defaults, pause access, or retire identities you no longer use."
        size="md"
        dismissable={!pending()}
        footer={<>
          <button type="button" class="agent-thread-button secondary" disabled={Boolean(pending())} onClick={close}>
            Close
          </button>
          <button type="button" class="agent-thread-button primary" disabled={Boolean(pending())} onClick={props.onActivate}>
            <Icon name="agent" /> Activate agent
          </button>
        </>}
      >
        <Show when={!props.loading} fallback={
          <Skeleton label="Loading your named agents…" rows={2}>
            <SkeletonBlock height="4.5rem" />
            <SkeletonBlock height="4.5rem" />
          </Skeleton>
        }>
          <Show when={!props.unavailable} fallback={
            <div class="agent-roster-empty" role="alert">
              <Icon name="gate" size={28} />
              <strong>Your agents couldn’t be loaded</strong>
              <p>Close this dialog and try again.</p>
            </div>
          }>
            <Show when={ownedAgents().length > 0} fallback={
              <div class="agent-roster-empty">
                <Icon name="agent" size={28} />
                <strong>No personal agents yet</strong>
                <p>Activate a named agent to give it private, reusable workspace instructions.</p>
              </div>
            }>
              <ul class="agent-roster-list" role="list">
                <For each={ownedAgents()}>{(agent) => (
                  <li>
                    <div class="agent-roster-summary">
                      <strong>{agent.name}</strong>
                      <span>{agent.status} · {agent.runtime_ref === "external:mcp" ? "external MCP" : "hosted"}</span>
                      <small>
                        {agent.workspace_profile.personalization
                          ? `Workspace defaults · revision ${agent.workspace_profile.revision}`
                          : "No workspace defaults"}
                      </small>
                      <Show when={errorAgentId() === agent.id}>
                        <small class="agent-roster-error" role="alert">
                          Myelin couldn’t confirm that change. Retry it with the retained request.
                        </small>
                      </Show>
                    </div>
                    <div class="agent-roster-actions">
                      <Show when={agent.status !== "disabled"} fallback={
                        <span class="agent-roster-retired"><Icon name="gate" /> Retired permanently</span>
                      }>
                        <button
                          type="button"
                          class="agent-thread-button secondary"
                          disabled={Boolean(pending())}
                          onClick={() => props.onConfigure(agent)}
                        >
                          <Icon name="edit" /> Configure
                        </button>
                        <Show when={agent.status === "active"} fallback={
                          <button
                            type="button"
                            class="agent-thread-button secondary"
                            disabled={Boolean(pending())}
                            onClick={() => void applyLifecycle(agent, "resume")}
                          >
                            <Icon name={isPending(agent, "resume") ? "cycle" : "approve"} />
                            {isPending(agent, "resume") ? "Resuming…" : "Resume"}
                          </button>
                        }>
                          <button
                            type="button"
                            class="agent-thread-button secondary"
                            disabled={Boolean(pending())}
                            onClick={() => void applyLifecycle(agent, "suspend")}
                          >
                            <Icon name={isPending(agent, "suspend") ? "cycle" : "gate"} />
                            {isPending(agent, "suspend") ? "Suspending…" : "Suspend"}
                          </button>
                        </Show>
                        <button
                          type="button"
                          class="agent-thread-button danger"
                          disabled={Boolean(pending())}
                          onClick={() => setRetiringAgent(agent)}
                        >
                          <Icon name="close" /> Retire
                        </button>
                      </Show>
                    </div>
                  </li>
                )}</For>
              </ul>
            </Show>
            <Show when={props.hasMore}>
              <button
                type="button"
                class="agent-thread-load-more"
                disabled={props.loadingMore || Boolean(pending())}
                onClick={() => props.onLoadMore()}
              >
                <Icon name={props.loadingMore ? "cycle" : "chevron"} />
                {props.loadingMore ? "Loading more agents…" : "Load more agents"}
              </button>
            </Show>
          </Show>
        </Show>
      </Dialog>
      <ConfirmDialog
        open={Boolean(retiringAgent())}
        onCancel={() => !pending() && setRetiringAgent(null)}
        onConfirm={() => {
          const agent = retiringAgent();
          if (agent) void applyLifecycle(agent, "retire");
        }}
        title={retiringAgent() ? `Retire ${retiringAgent()!.name}?` : "Retire this agent?"}
        description="This permanently disables the identity, revokes unfinished runs, and cannot be undone. Existing retained work remains readable to its owner."
        confirmLabel={retiringAgent() && isPending(retiringAgent()!, "retire") ? "Retiring…" : "Retire agent"}
        cancelLabel="Keep agent"
        variant="destructive"
      />
    </>
  );
}

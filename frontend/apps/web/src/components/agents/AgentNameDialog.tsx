import { Dialog, Icon } from "@myelin/design-system";
import { useAction } from "@solidjs/router";
import { createEffect, createSignal, Show } from "solid-js";

import { mutateAgentThread } from "~/lib/agent-thread-api";
import type { AgentChoice, AgentNameReceipt } from "~/lib/agent-thread-response";

const utf8 = new TextEncoder();

function validName(value: string): boolean {
  return value.length > 0 && value.trim() === value && utf8.encode(value).byteLength <= 80 &&
    !/[\p{Cc}]/u.test(value);
}

export function AgentNameDialog(props: {
  agent: AgentChoice | null;
  onClose: () => void;
  onConflict: (agentId: string) => void;
  onRenamed: (receipt: AgentNameReceipt) => void;
}) {
  const mutate = useAction(mutateAgentThread);
  const [name, setName] = createSignal("");
  const [submitting, setSubmitting] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [clientNonce, setClientNonce] = createSignal(crypto.randomUUID());

  createEffect(() => {
    setName(props.agent?.name ?? "");
    setSubmitting(false);
    setError(null);
    setClientNonce(crypto.randomUUID());
  });

  const close = () => { if (!submitting()) props.onClose(); };
  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    const agent = props.agent;
    if (!agent || submitting()) return;
    if (!validName(name())) {
      setError("Use a name up to 80 bytes without leading, trailing, or control characters.");
      return;
    }
    setSubmitting(true);
    setError(null);
    try {
      const result = await mutate({
        op: "rename-agent",
        agentId: agent.id,
        expectedName: agent.name,
        name: name(),
        clientNonce: clientNonce(),
      });
      if (!result.ok || result.op !== "rename-agent") {
        if (!result.ok && result.error === "conflict") props.onConflict(agent.id);
        setError(!result.ok && result.error === "conflict"
          ? "That name is already used, or this agent changed elsewhere. Close and reopen to review the latest name."
          : !result.ok && result.error === "not-found"
            ? "This agent is no longer available to your account."
            : "Myelin couldn’t rename this agent. Your edit is still here; retry when ready.");
        return;
      }
      props.onRenamed(result.receipt);
      props.onClose();
    } catch {
      setError("Myelin couldn’t rename this agent. Your edit is still here; retry when ready.");
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog
      open={Boolean(props.agent)}
      onClose={close}
      title={props.agent ? `Rename ${props.agent.name}` : "Rename agent"}
      description="Change the private display name without replacing the identity, its workspaces, or its authority."
      size="sm"
      dismissable={!submitting()}
      footer={<>
        <button type="button" class="agent-thread-button secondary" onClick={close} disabled={submitting()}>
          Cancel
        </button>
        <button type="submit" form="agent-name" class="agent-thread-button primary" disabled={submitting()}>
          <Icon name={submitting() ? "cycle" : "approve"} />
          {submitting() ? "Renaming…" : "Rename agent"}
        </button>
      </>}
    >
      <form id="agent-name" class="agent-thread-create" onSubmit={submit}>
        <label>
          Agent name
          <input
            value={name()}
            maxlength={80}
            autofocus
            disabled={submitting()}
            onInput={(event) => {
              setName(event.currentTarget.value);
              setClientNonce(crypto.randomUUID());
              setError(null);
            }}
          />
        </label>
        <p class="agent-thread-note">
          Existing threads and retained work stay attached to this agent. Names remain unique in
          your organization.
        </p>
        <Show when={error()}>{(message) => (
          <p role="alert" class="agent-thread-error">{message()}</p>
        )}</Show>
      </form>
    </Dialog>
  );
}

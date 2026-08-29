import { Dialog, Icon } from "@myelin/design-system";
import { useAction } from "@solidjs/router";
import { createEffect, createSignal, onCleanup, Show } from "solid-js";

import {
  mutateAgentThread,
  type AgentThreadErrorKind,
} from "~/lib/agent-thread-api";
import type { AgentActivationReceipt } from "~/lib/agent-thread-response";

function errorCopy(kind: AgentThreadErrorKind): string {
  if (kind === "bad-input") return "Use a clean agent name.";
  if (kind === "conflict") return "That retry identity was already used for another agent.";
  if (kind === "unavailable") return "Agent activation is temporarily unavailable. This draft is safe to retry.";
  return "We couldn’t confirm the agent. This draft keeps its retry identity until you edit it.";
}

export function AgentActivationDialog(props: {
  open: boolean;
  onClose: () => void;
  onActivated: (receipt: AgentActivationReceipt) => void;
}) {
  const mutate = useAction(mutateAgentThread);
  const [name, setName] = createSignal("");
  const [allowWorkspaceCommands, setAllowWorkspaceCommands] = createSignal(false);
  const [clientNonce, setClientNonce] = createSignal(crypto.randomUUID());
  const [submitting, setSubmitting] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  let nameInput: HTMLInputElement | undefined;
  let openingGeneration = 0;

  createEffect(() => {
    openingGeneration += 1;
    if (!props.open) return;
    setName("");
    setAllowWorkspaceCommands(false);
    setClientNonce(crypto.randomUUID());
    setSubmitting(false);
    setError(null);
  });
  onCleanup(() => { openingGeneration += 1; });

  const editName = (value: string) => {
    setName(value);
    setClientNonce(crypto.randomUUID());
    setError(null);
  };
  const close = () => { if (!submitting()) props.onClose(); };
  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    if (!name().trim() || name().trim() !== name()) {
      setError("Use a clean agent name without leading or trailing whitespace.");
      nameInput?.focus();
      return;
    }
    const generation = openingGeneration;
    setSubmitting(true);
    setError(null);
    try {
      const result = await mutate({
        op: "activate-agent",
        name: name(),
        allowWorkspaceCommands: allowWorkspaceCommands(),
        clientNonce: clientNonce(),
      });
      if (generation !== openingGeneration) return;
      if (!result.ok || result.op !== "activate-agent") {
        setError(errorCopy(result.ok ? "error" : result.error));
        return;
      }
      props.onClose();
      props.onActivated(result.receipt);
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
      title="Activate private-work agent"
      description="Give an external agent a Myelin identity for private conversations and bounded workspace files. No provider credential or API key is created."
      size="md"
      dismissable={!submitting()}
      initialFocus={() => nameInput}
      footer={<>
        <button type="button" class="agent-thread-button secondary" onClick={close} disabled={submitting()}>
          Cancel
        </button>
        <button type="submit" form="private-agent-activation" class="agent-thread-button primary" disabled={submitting() || !name().trim()}>
          <Icon name={submitting() ? "cycle" : "agent"} />
          {submitting() ? "Activating…" : "Activate agent"}
        </button>
      </>}
    >
      <form id="private-agent-activation" class="agent-thread-create" onSubmit={submit}>
        <label>
          Agent name
          <input
            ref={nameInput}
            value={name()}
            maxlength={80}
            autocomplete="off"
            placeholder="Checkout companion"
            disabled={submitting()}
            aria-invalid={Boolean(error())}
            onInput={(event) => editName(event.currentTarget.value)}
          />
        </label>
        <div class="agent-activation-scope">
          <Icon name="gate" />
          <p>
            The initial tool set covers conversation history, private replies, and files in a
            generation-fenced workspace. Your live permissions remain the authority ceiling.
          </p>
        </div>
        <label class="agent-activation-command-toggle" for="agent-allow-workspace-commands">
          <input
            id="agent-allow-workspace-commands"
            type="checkbox"
            checked={allowWorkspaceCommands()}
            disabled={submitting()}
            onChange={(event) => {
              setAllowWorkspaceCommands(event.currentTarget.checked);
              setClientNonce(crypto.randomUUID());
              setError(null);
            }}
          />
          <strong>Allow bounded workspace commands</strong>
          <small>Lets this agent run generation-fenced, network-denied commands with durable effect receipts.</small>
        </label>
        <Show when={error()}>{(message) => <p role="alert" class="agent-thread-error">{message()}</p>}</Show>
      </form>
    </Dialog>
  );
}

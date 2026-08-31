import { Dialog, Icon } from "@myelin/design-system";
import { useAction } from "@solidjs/router";
import { createEffect, createSignal, onCleanup, Show } from "solid-js";

import { mutateAgentThread } from "~/lib/agent-thread-api";
import { validAgentToolPalette, type AgentToolDefinition } from "~/lib/agent-tool-palette";
import type { AgentChoice, AgentToolPaletteReceipt } from "~/lib/agent-thread-response";
import { AgentToolPaletteEditor } from "./AgentToolPaletteEditor";

export function AgentToolsDialog(props: {
  agent: AgentChoice | null;
  onClose: () => void;
  onConfigured: (receipt: AgentToolPaletteReceipt) => void;
  availableTools: AgentToolDefinition[];
}) {
  const mutate = useAction(mutateAgentThread);
  const [tools, setTools] = createSignal<string[]>([]);
  const [expectedTools, setExpectedTools] = createSignal<string[]>([]);
  const [submitting, setSubmitting] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [clientNonce, setClientNonce] = createSignal(crypto.randomUUID());
  let generation = 0;

  createEffect(() => {
    generation += 1;
    const selected = props.agent?.selected_tools ?? [];
    setTools([...selected]);
    setExpectedTools([...selected]);
    setSubmitting(false);
    setError(null);
    setClientNonce(crypto.randomUUID());
  });
  onCleanup(() => { generation += 1; });

  const close = () => { if (!submitting()) props.onClose(); };
  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    const agent = props.agent;
    if (!agent || submitting()) return;
    if (!validAgentToolPalette(tools())) {
      setError("Select at least one valid tool for this agent.");
      return;
    }
    const current = generation;
    setSubmitting(true);
    setError(null);
    try {
      const result = await mutate({
        op: "configure-agent-tools",
        agentId: agent.id,
        expectedTools: expectedTools(),
        tools: tools(),
        clientNonce: clientNonce(),
      });
      if (current !== generation) return;
      if (!result.ok || result.op !== "configure-agent-tools") {
        setError(!result.ok && result.error === "conflict"
          ? "This palette changed elsewhere. Close and reopen to review the current tools."
          : "Myelin couldn’t save the tool palette. Your selection is still here.");
        return;
      }
      props.onConfigured(result.receipt);
      props.onClose();
    } catch {
      if (current === generation) {
        setError("Myelin couldn’t save the tool palette. Your selection is still here.");
      }
    } finally {
      if (current === generation) setSubmitting(false);
    }
  };

  return (
    <Dialog
      open={Boolean(props.agent)}
      onClose={close}
      title={props.agent ? `${props.agent.name} · tools` : "Agent tools"}
      description="Keep this agent’s authority legible and no broader than the work it should do. A change immediately closes its live runs."
      size="md"
      dismissable={!submitting()}
      footer={<>
        <button type="button" class="agent-thread-button secondary" onClick={close} disabled={submitting()}>
          Cancel
        </button>
        <button type="submit" form="agent-tool-palette" class="agent-thread-button primary" disabled={submitting()}>
          <Icon name={submitting() ? "cycle" : "approve"} />
          {submitting() ? "Saving…" : "Save tools"}
        </button>
      </>}
    >
      <form id="agent-tool-palette" class="agent-thread-create" onSubmit={submit}>
        <AgentToolPaletteEditor
          tools={tools()}
          availableTools={props.availableTools}
          disabled={submitting()}
          onChange={(next) => {
            setTools(next);
            setClientNonce(crypto.randomUUID());
            setError(null);
          }}
        />
        <p class="agent-thread-note">
          Existing workspace data stays intact. Any live agent connection is revoked; the next
          connection receives only the saved palette and its newly derived grants.
        </p>
        <Show when={error()}>{(message) => (
          <p role="alert" class="agent-thread-error">{message()}</p>
        )}</Show>
      </form>
    </Dialog>
  );
}

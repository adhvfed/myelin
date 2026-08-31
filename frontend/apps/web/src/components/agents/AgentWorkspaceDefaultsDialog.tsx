import { Dialog, Icon } from "@myelin/design-system";
import { useAction } from "@solidjs/router";
import { createEffect, createSignal, onCleanup, Show } from "solid-js";

import {
  getAgentWorkspaceGuidance,
  mutateAgentThread,
} from "~/lib/agent-thread-api";
import type {
  AgentChoice,
  AgentWorkspaceFile,
  AgentWorkspaceGuidanceReceipt,
} from "~/lib/agent-thread-response";
import {
  AgentWorkspaceFilesEditor,
  validAgentWorkspaceAugmentation,
} from "./AgentWorkspaceFilesEditor";

export function AgentWorkspaceDefaultsDialog(props: {
  agent: AgentChoice | null;
  onClose: () => void;
  onConfigured: (receipt: AgentWorkspaceGuidanceReceipt) => void;
}) {
  const mutate = useAction(mutateAgentThread);
  const [instructions, setInstructions] = createSignal("");
  const [files, setFiles] = createSignal<AgentWorkspaceFile[]>([]);
  const [revision, setRevision] = createSignal(0);
  const [loading, setLoading] = createSignal(false);
  const [submitting, setSubmitting] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [clientNonce, setClientNonce] = createSignal(crypto.randomUUID());
  let generation = 0;

  createEffect(() => {
    const agent = props.agent;
    generation += 1;
    const current = generation;
    setInstructions("");
    setFiles([]);
    setRevision(agent?.workspace_profile.revision ?? 0);
    setError(null);
    setSubmitting(false);
    setClientNonce(crypto.randomUUID());
    if (!agent) {
      setLoading(false);
      return;
    }
    setLoading(true);
    void getAgentWorkspaceGuidance(agent.id)
      .then((config) => {
        if (current !== generation) return;
        setInstructions(config.instructions ?? "");
        setFiles(config.files);
        setRevision(config.revision);
      })
      .catch(() => {
        if (current === generation) {
          setError("Myelin couldn’t load this agent’s private workspace defaults.");
        }
      })
      .finally(() => {
        if (current === generation) setLoading(false);
      });
  });
  onCleanup(() => { generation += 1; });

  const close = () => { if (!submitting()) props.onClose(); };
  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    const agent = props.agent;
    if (!agent || loading() || submitting()) return;
    const value = instructions();
    if (value.includes("\0") || (value !== "" && value.trim() === "") ||
        !validAgentWorkspaceAugmentation(value, files())) {
      setError("Use a valid instruction and file overlay within the 12 KiB encrypted limit.");
      return;
    }
    const current = generation;
    setSubmitting(true);
    setError(null);
    try {
      const result = await mutate({
        op: "configure-agent",
        agentId: agent.id,
        workspaceInstructions: value === "" ? null : value,
        workspaceFiles: files(),
        expectedRevision: revision(),
        clientNonce: clientNonce(),
      });
      if (current !== generation) return;
      if (!result.ok || result.op !== "configure-agent") {
        setError(!result.ok && result.error === "conflict"
          ? "These defaults changed elsewhere. Close and reopen to review the latest revision."
          : "Myelin couldn’t save the workspace defaults. Your edit is still here.");
        return;
      }
      props.onConfigured(result.receipt);
      props.onClose();
    } catch {
      if (current === generation) {
        setError("Myelin couldn’t save the workspace defaults. Your edit is still here.");
      }
    } finally {
      if (current === generation) setSubmitting(false);
    }
  };

  return (
    <Dialog
      open={Boolean(props.agent)}
      onClose={close}
      title={props.agent ? `${props.agent.name} · workspace defaults` : "Workspace defaults"}
      description="Set the owner-only instructions this named agent carries into each new private workspace."
      size="md"
      dismissable={!submitting()}
      footer={<>
        <button type="button" class="agent-thread-button secondary" onClick={close} disabled={submitting()}>
          Cancel
        </button>
        <button type="submit" form="agent-workspace-defaults" class="agent-thread-button primary" disabled={loading() || submitting()}>
          <Icon name={submitting() ? "cycle" : "approve"} />
          {submitting() ? "Saving…" : "Save defaults"}
        </button>
      </>}
    >
      <form id="agent-workspace-defaults" class="agent-thread-create" onSubmit={submit}>
        <label>
          Default <code>AGENTS.md</code> instructions
          <textarea
            value={instructions()}
            rows={10}
            disabled={loading() || submitting()}
            aria-busy={loading()}
            placeholder={loading() ? "Loading private defaults…" : "# How I work\n\nExplain tradeoffs and run focused tests."}
            onInput={(event) => {
              setInstructions(event.currentTarget.value);
              setClientNonce(crypto.randomUUID());
              setError(null);
            }}
          />
        </label>
        <p class="agent-thread-note">
          Thread-specific instructions are appended after these defaults, so they can narrow the
          approach for one task. Existing workspaces keep their encrypted snapshot.
        </p>
        <AgentWorkspaceFilesEditor
          files={files()}
          disabled={loading() || submitting()}
          onChange={(nextFiles) => {
            setFiles(nextFiles);
            setClientNonce(crypto.randomUUID());
            setError(null);
          }}
        />
        <Show when={error()}>{(message) => (
          <p role="alert" class="agent-thread-error">{message()}</p>
        )}</Show>
      </form>
    </Dialog>
  );
}

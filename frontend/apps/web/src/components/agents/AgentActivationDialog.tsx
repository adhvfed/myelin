import { Dialog, Icon } from "@myelin/design-system";
import { useAction } from "@solidjs/router";
import { createEffect, createSignal, onCleanup, Show } from "solid-js";

import {
  mutateAgentThread,
  type AgentThreadErrorKind,
} from "~/lib/agent-thread-api";
import type { AgentActivationReceipt, AgentWorkspaceFile } from "~/lib/agent-thread-response";
import { validAgentWorkspaceAugmentation } from "~/lib/agent-workspace-augmentation";
import {
  DEFAULT_PERSONAL_AGENT_TOOLS,
  type AgentToolDefinition,
  validAgentToolPalette,
} from "~/lib/agent-tool-palette";
import { AgentToolPaletteEditor } from "./AgentToolPaletteEditor";
import { AgentWorkspaceFilesEditor } from "./AgentWorkspaceFilesEditor";

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
  availableTools: AgentToolDefinition[];
}) {
  const mutate = useAction(mutateAgentThread);
  const [name, setName] = createSignal("");
  const [tools, setTools] = createSignal<string[]>([...DEFAULT_PERSONAL_AGENT_TOOLS]);
  const [workspaceInstructions, setWorkspaceInstructions] = createSignal("");
  const [workspaceFiles, setWorkspaceFiles] = createSignal<AgentWorkspaceFile[]>([]);
  const [clientNonce, setClientNonce] = createSignal(crypto.randomUUID());
  const [submitting, setSubmitting] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  let nameInput: HTMLInputElement | undefined;
  let openingGeneration = 0;

  createEffect(() => {
    openingGeneration += 1;
    if (!props.open) return;
    setName("");
    setTools([...DEFAULT_PERSONAL_AGENT_TOOLS]);
    setWorkspaceInstructions("");
    setWorkspaceFiles([]);
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
    if (!name().trim() || name().trim() !== name() || !validAgentToolPalette(tools()) ||
        workspaceInstructions().includes("\0") ||
        (workspaceInstructions() !== "" && workspaceInstructions().trim() === "") ||
        !validAgentWorkspaceAugmentation(
          workspaceInstructions() === "" ? null : workspaceInstructions(),
          workspaceFiles(),
        )) {
      setError("Use a clean agent name and a workspace overlay within 12 KiB.");
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
        tools: tools(),
        workspaceInstructions: workspaceInstructions(),
        workspaceFiles: workspaceFiles(),
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
        <label>
          Default workspace instructions <span aria-hidden="true">(optional)</span>
          <textarea
            value={workspaceInstructions()}
            rows={6}
            placeholder={"# How I work\n\nExplain tradeoffs before changing interfaces. Run the narrow journey first."}
            disabled={submitting()}
            onInput={(event) => {
              setWorkspaceInstructions(event.currentTarget.value);
              setClientNonce(crypto.randomUUID());
              setError(null);
            }}
          />
        </label>
        <p class="agent-thread-note">
          New private threads inherit this owner-only overlay. You can add thread-specific
          instructions and files later; existing workspaces keep their original snapshot.
        </p>
        <AgentWorkspaceFilesEditor
          files={workspaceFiles()}
          disabled={submitting()}
          legend="Default private context files"
          onChange={(files) => {
            setWorkspaceFiles(files);
            setClientNonce(crypto.randomUUID());
            setError(null);
          }}
        />
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
        <Show when={error()}>{(message) => <p role="alert" class="agent-thread-error">{message()}</p>}</Show>
      </form>
    </Dialog>
  );
}

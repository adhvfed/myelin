import { For, Show } from "solid-js";

import type { AgentWorkspaceFile } from "~/lib/agent-workspace-augmentation";

export function AgentWorkspaceFilesEditor(props: {
  files: readonly AgentWorkspaceFile[];
  disabled: boolean;
  legend?: string;
  onChange: (files: AgentWorkspaceFile[]) => void;
}) {
  const update = (index: number, field: "path" | "contents", value: string) => {
    props.onChange(props.files.map((file, fileIndex) => fileIndex === index
      ? { ...file, [field]: value }
      : file));
  };
  const remove = (index: number) => {
    props.onChange(props.files.filter((_, fileIndex) => fileIndex !== index));
  };

  return (
    <fieldset class="agent-workspace-files">
      <legend>{props.legend ?? "Private context files"} <span aria-hidden="true">(optional)</span></legend>
      <For each={props.files}>{(file, index) => (
        <div class="agent-workspace-file">
          <label>
            File {index() + 1} path
            <input
              value={file.path}
              autocomplete="off"
              placeholder=".myelin/checkout-context.md"
              disabled={props.disabled}
              onInput={(event) => update(index(), "path", event.currentTarget.value)}
            />
          </label>
          <label>
            File {index() + 1} contents
            <textarea
              value={file.contents}
              rows={5}
              placeholder={"# Private context\n\nDescribe the durable constraints for this agent."}
              disabled={props.disabled}
              onInput={(event) => update(index(), "contents", event.currentTarget.value)}
            />
          </label>
          <button
            type="button"
            class="agent-thread-button secondary"
            disabled={props.disabled}
            onClick={() => remove(index())}
          >
            Remove file {index() + 1}
          </button>
        </div>
      )}</For>
      <Show when={props.files.length < 8}>
        <button
          type="button"
          class="agent-thread-button secondary"
          disabled={props.disabled}
          onClick={() => props.onChange([
            ...props.files,
            { path: "", contents: "", strategy: "create" },
          ])}
        >
          Add context file
        </button>
      </Show>
      <p class="agent-thread-note">
        Files are create-only and encrypted with the instructions. A conflicting repository file,
        symlink, traversal, <code>AGENTS.md</code>, or <code>.git</code> path fails closed.
      </p>
    </fieldset>
  );
}

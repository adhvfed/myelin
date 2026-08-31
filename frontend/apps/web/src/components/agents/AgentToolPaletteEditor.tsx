import { For } from "solid-js";

import {
  PERSONAL_AGENT_TOOLS,
  type AgentToolDefinition,
} from "~/lib/agent-tool-palette";

export function AgentToolPaletteEditor(props: {
  tools: readonly string[];
  availableTools: readonly AgentToolDefinition[];
  disabled: boolean;
  onChange: (tools: string[]) => void;
}) {
  const friendly = new Map(PERSONAL_AGENT_TOOLS.map((tool) => [tool.name, tool]));
  const choices = () => {
    const available = props.availableTools.filter((tool) => tool.exposed_over_mcp);
    const source = available.length > 0
      ? available.map((tool) => ({
        name: tool.name,
        label: friendly.get(tool.name)?.label ?? tool.name,
        description: friendly.get(tool.name)?.description ??
          `${tool.effect_kind} tool in ${tool.subsystem}${tool.requires_approval ? "; each effect requires approval" : ""}.`,
      }))
      : [...PERSONAL_AGENT_TOOLS];
    const known = new Set(source.map((tool) => tool.name));
    return [
      ...source,
      ...props.tools.filter((name) => !known.has(name)).map((name) => ({
        name,
        label: name,
        description: "A platform tool already assigned to this agent but absent from current discovery.",
      })),
    ];
  };
  const toggle = (name: string, selected: boolean) => {
    const next = selected
      ? [...props.tools, name]
      : props.tools.filter((tool) => tool !== name);
    props.onChange([...new Set(next)].sort());
  };

  return (
    <fieldset class="agent-tool-palette">
      <legend>Tool palette</legend>
      <p>Choose the exact tools available to fresh agent runs. Your live permissions remain the ceiling.</p>
      <div class="agent-tool-palette-choices">
        <For each={choices()}>{(tool) => (
          <label for={`agent-tool-${tool.name}`}>
            <input
              id={`agent-tool-${tool.name}`}
              type="checkbox"
              checked={props.tools.includes(tool.name)}
              disabled={props.disabled}
              onChange={(event) => toggle(tool.name, event.currentTarget.checked)}
            />
            <strong>{tool.label}</strong>
            <small><code>{tool.name}</code> · {tool.description}</small>
          </label>
        )}</For>
      </div>
    </fieldset>
  );
}

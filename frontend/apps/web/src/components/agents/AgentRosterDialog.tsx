import { Dialog, Icon, Skeleton, SkeletonBlock } from "@myelin/design-system";
import { For, Show } from "solid-js";

import type { AgentChoice } from "~/lib/agent-thread-response";

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
  onClose: () => void;
}) {
  const ownedAgents = () => props.agents.filter((agent) =>
    agent.owner_principal_id === props.viewerPrincipalId);

  return (
    <Dialog
      open={props.open}
      onClose={props.onClose}
      title="My named agents"
      description="Configure the private defaults your agents carry into new workspaces. Existing threads keep their original snapshot."
      size="md"
      footer={<>
        <button type="button" class="agent-thread-button secondary" onClick={props.onClose}>
          Close
        </button>
        <button type="button" class="agent-thread-button primary" onClick={props.onActivate}>
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
                  <div>
                    <strong>{agent.name}</strong>
                    <span>{agent.status} · {agent.runtime_ref === "external:mcp" ? "external MCP" : "hosted"}</span>
                    <small>
                      {agent.workspace_profile.instructions
                        ? `AGENTS.md defaults · revision ${agent.workspace_profile.revision}`
                        : "No workspace defaults"}
                    </small>
                  </div>
                  <button
                    type="button"
                    class="agent-thread-button secondary"
                    onClick={() => props.onConfigure(agent)}
                  >
                    <Icon name="edit" /> Configure
                  </button>
                </li>
              )}</For>
            </ul>
          </Show>
          <Show when={props.hasMore}>
            <button
              type="button"
              class="agent-thread-load-more"
              disabled={props.loadingMore}
              onClick={() => props.onLoadMore()}
            >
              <Icon name={props.loadingMore ? "cycle" : "chevron"} />
              {props.loadingMore ? "Loading more agents…" : "Load more agents"}
            </button>
          </Show>
        </Show>
      </Show>
    </Dialog>
  );
}

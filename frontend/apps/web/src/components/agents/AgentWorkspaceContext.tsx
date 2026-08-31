import { Icon } from "@myelin/design-system";
import { For, Show } from "solid-js";

import {
  agentThreadAwaitingRecovery,
  externalAgentCanConnect,
  type AgentChoice,
  type AgentThread,
  type WorkspaceSession,
} from "~/lib/agent-thread-response";

function dateTime(value: string): string {
  return `${new Date(value).toISOString().slice(0, 16).replace("T", " ")} UTC`;
}

function sessionLabel(mode: WorkspaceSession["mode"]): string {
  if (mode === "shell") return "Interactive shell";
  if (mode === "file_transfer") return "File transfer";
  return "Remote command";
}

export function AgentWorkspaceContext(props: {
  thread: AgentThread;
  agent: AgentChoice | undefined;
  sessions: WorkspaceSession[];
  sessionsLoading: boolean;
  sessionsUnavailable: boolean;
  sessionsHaveMore: boolean;
  sessionsLoadingMore: boolean;
  onLoadMoreSessions: () => void;
  workspaceAvailable: boolean;
  retrying: boolean;
  retryError: string | null;
  onRetry: () => void;
}) {
  const agentCommand = () =>
    `myelin mcp serve --as ${props.thread.agent_id} --thread ${props.thread.id}`;
  const command = () => `myelin agent thread ssh ${props.thread.id}`;
  const fileCommand = () => `${command()} --sftp`;
  const editorCommand = () =>
    `myelin agent thread ssh-config ${props.thread.id} --identity /path/to/ssh_identity`;
  const agentName = () => props.agent?.name ?? `Agent ${props.thread.agent_id.slice(0, 8)}`;
  const commandScope = () => props.agent?.selected_tools.includes("workspace.exec");
  const retryable = () => agentThreadAwaitingRecovery(props.thread);
  const connectUnavailableCopy = () => {
    if (!props.agent) return "Agent connection is unavailable until its identity can be loaded.";
    if (props.agent.status === "suspended") {
      return `${agentName()} is suspended. Resume the identity before connecting it.`;
    }
    if (props.agent.status === "disabled") return `${agentName()} has been retired and cannot connect.`;
    return `${agentName()} is managed by Myelin and does not use an external MCP command.`;
  };
  return (
    <div class="agent-workspace-context">
      <section>
        <p class="agent-thread-eyebrow"><Icon name="database" /> Durable workspace</p>
        <h2>Generation {props.thread.workspace.generation}</h2>
        <dl>
          <div><dt>State</dt><dd>{props.thread.workspace.state}</dd></div>
          <div><dt>Agent</dt><dd>{agentName()}</dd></div>
          <div><dt>Expires</dt><dd><time datetime={props.thread.workspace.expires_at}>{dateTime(props.thread.workspace.expires_at)}</time></dd></div>
          <Show when={props.thread.workspace.repository}>{(repository) => <>
            <div><dt>Repository</dt><dd>{repository().slug}</dd></div>
            <div><dt>Branch</dt><dd>{repository().branch}</dd></div>
            <div><dt>Base</dt><dd>{repository().base_ref} · {repository().base_oid.slice(0, 12)}</dd></div>
          </>}</Show>
          <Show when={props.thread.workspace.environment}>{(environment) => <>
            <div data-testid="workspace-environment">
              <dt>Environment</dt>
              <dd>Service Federation · {environment().state}</dd>
            </div>
            <div>
              <dt>Manifest</dt>
              <dd>
                <code>{environment().config_path}</code> ·{" "}
                <code title={environment().content_digest}>
                  {environment().content_digest.slice(0, 19)}…
                </code>
              </dd>
            </div>
          </>}</Show>
          <Show when={props.thread.workspace.personalization}><>
            <div data-testid="workspace-personalization">
              <dt>Personalization</dt>
              <dd>Declarative workspace overlay</dd>
            </div>
          </></Show>
          <div><dt>Workspace</dt><dd><code>{props.thread.workspace.id}</code></dd></div>
        </dl>
        <Show when={props.thread.workspace.environment}>
          <p class="agent-thread-note" data-testid="workspace-environment-state">
            The manifest is pinned to this source commit. Environment activation is not available yet.
          </p>
        </Show>
        <Show when={props.thread.workspace.personalization}>{(personalization) => (
          <p class="agent-thread-note" title={personalization().content_digest}>
            Owner instructions and create-only files are encrypted at rest and applied without
            changing the source repository.
          </p>
        )}</Show>
      </section>
      <section>
        <h3>Agent workspace authority</h3>
        <Show when={props.agent} fallback={<p data-testid="agent-command-scope">Tool scope is temporarily unavailable.</p>}>
          <p data-testid="agent-command-scope">
            {commandScope()
              ? "Bounded commands authorized · files and command effects remain generation-fenced."
              : "File access authorized · command execution was not selected."}
          </p>
        </Show>
      </section>
      <Show when={props.workspaceAvailable} fallback={
        <section data-testid="workspace-unavailable">
          <h3>Workspace unavailable</h3>
          <Show when={retryable()} fallback={
            <p>This retained receipt is no longer active. New messages, agent runs, and SSH access are unavailable.</p>
          }>
            <p>
              {props.thread.workspace.state === "failed"
                ? "Myelin couldn’t finish starting this workspace. Its private identities are retained and safe to retry."
                : "This workspace is still being prepared. Retry the same receipt if startup was interrupted."}
            </p>
            <button
              type="button"
              class="agent-thread-button primary"
              disabled={props.retrying}
              onClick={() => props.onRetry()}
              data-testid="agent-thread-retry"
            >
              <Icon name={props.retrying ? "cycle" : "agent"} />
              {props.retrying ? "Retrying…" : "Retry startup"}
            </button>
            <Show when={props.retryError}>
              <p role="alert">{props.retryError}</p>
            </Show>
          </Show>
        </section>
      }>
        <section>
          <h3>Connect the agent</h3>
          <Show when={externalAgentCanConnect(props.agent)} fallback={
            <p data-testid="agent-connect-unavailable">{connectUnavailableCopy()}</p>
          }>
            <p>Use this as the MCP server command for an external client. Myelin exchanges your signed-in session for short-lived run authority.</p>
            <code class="agent-workspace-command" data-testid="agent-connect-command">{agentCommand()}</code>
          </Show>
        </section>
        <section>
          <h3>Enter with SSH</h3>
          <Show when={externalAgentCanConnect(props.agent)} fallback={
            <p data-testid="workspace-ssh-unavailable">
              SSH access is unavailable until the selected agent identity is active.
            </p>
          }>
            <p>Run this from an authenticated Myelin CLI. It creates a one-shot key and pins the workspace host.</p>
            <code class="agent-workspace-command" data-testid="agent-workspace-command">{command()}</code>
            <p>Open the same workspace with the standard SFTP client for remote file editing.</p>
            <code class="agent-workspace-command" data-testid="agent-workspace-file-command">{fileCommand()}</code>
            <p>Or authorize your existing key and generate an expiring OpenSSH profile for an SFTP-capable editor.</p>
            <code class="agent-workspace-command" data-testid="agent-workspace-editor-command">{editorCommand()}</code>
          </Show>
        </section>
      </Show>
      <section>
        <h3>Workspace entries</h3>
        <Show when={!props.sessionsLoading} fallback={<p>Loading accountable entries…</p>}>
          <Show when={!props.sessionsUnavailable} fallback={<p role="alert">Workspace entries are temporarily unavailable.</p>}>
            <Show when={props.sessions.length > 0} fallback={<p>No workspace entries yet.</p>}>
              <ol class="agent-workspace-session-list">
                <For each={props.sessions}>{(session) => (
                  <li>
                    <Icon name="external-link" />
                    <span>
                      <strong>{sessionLabel(session.mode)}</strong>
                      <small>{session.terminal ? "terminal allocated" : "no terminal"} · {dateTime(session.started_at)}</small>
                    </span>
                  </li>
                )}</For>
              </ol>
              <Show when={props.sessionsHaveMore}>
                <button
                  type="button"
                  class="agent-thread-load-more"
                  disabled={props.sessionsLoadingMore}
                  onClick={() => props.onLoadMoreSessions()}
                >
                  <Icon name={props.sessionsLoadingMore ? "cycle" : "chevron"} />
                  {props.sessionsLoadingMore ? "Loading entries…" : "Earlier workspace entries"}
                </button>
              </Show>
            </Show>
          </Show>
        </Show>
      </section>
      <p class="agent-thread-note">
        Commands, keys, host routes, and file contents are never stored in this access history.
      </p>
    </div>
  );
}

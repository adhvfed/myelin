import { Icon } from "@myelin/design-system";
import { For, Show } from "solid-js";

import type { AgentThread, WorkspaceSession } from "~/lib/agent-thread-response";

function dateTime(value: string): string {
  return `${new Date(value).toISOString().slice(0, 16).replace("T", " ")} UTC`;
}

export function AgentWorkspaceContext(props: {
  thread: AgentThread;
  agentName: string;
  sessions: WorkspaceSession[];
  sessionsLoading: boolean;
  sessionsUnavailable: boolean;
  sessionsHaveMore: boolean;
  sessionsLoadingMore: boolean;
  onLoadMoreSessions: () => void;
}) {
  const command = () => `myelin agent thread ssh ${props.thread.id}`;
  return (
    <div class="agent-workspace-context">
      <section>
        <p class="agent-thread-eyebrow"><Icon name="database" /> Durable workspace</p>
        <h2>Generation {props.thread.workspace.generation}</h2>
        <dl>
          <div><dt>State</dt><dd>{props.thread.workspace.state}</dd></div>
          <div><dt>Agent</dt><dd>{props.agentName}</dd></div>
          <div><dt>Expires</dt><dd><time datetime={props.thread.workspace.expires_at}>{dateTime(props.thread.workspace.expires_at)}</time></dd></div>
          <div><dt>Workspace</dt><dd><code>{props.thread.workspace.id}</code></dd></div>
        </dl>
      </section>
      <section>
        <h3>Enter with SSH</h3>
        <p>Run this from an authenticated Myelin CLI. It creates a one-shot key and pins the workspace host.</p>
        <code class="agent-workspace-command" data-testid="agent-workspace-command">{command()}</code>
      </section>
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
                      <strong>{session.mode === "shell" ? "Interactive shell" : "Remote command"}</strong>
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

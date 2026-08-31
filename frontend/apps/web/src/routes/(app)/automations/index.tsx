import { ErrorBoundary, For, Show, Suspense, createMemo, createSignal } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, revalidate, useNavigate, useSearchParams } from "@solidjs/router";
import { Icon, Skeleton } from "@myelin/design-system";
import { AutomationErrorState, automationErrorKind } from "~/components/AutomationErrorState";
import { AutomationStateLabel } from "~/components/AutomationStateLabel";
import { AutomationCreateDialog } from "~/components/AutomationCreateDialog";
import { useAppViewer } from "~/components/AppShell";
import { getAutomations } from "~/lib/automation-api";
import { mergeAutomationAgentPages } from "~/lib/automation-agent-catalogue";
import { getAgentChoices } from "~/lib/agent-thread-api";
import {
  AUTOMATION_PAGE_LIMIT,
  type AutomationVM,
} from "~/lib/automation-response";
import type { AgentChoicePage } from "~/lib/agent-thread-response";
import { nextCursorForPages } from "~/lib/pagination";

export default function AutomationsIndex() {
  const [search] = useSearchParams();
  const navigate = useNavigate();
  const viewer = useAppViewer();
  const [creating, setCreating] = createSignal(false);
  const [agentPages, setAgentPages] = createSignal<AgentChoicePage[]>([]);
  const [loadingMoreAgents, setLoadingMoreAgents] = createSignal(false);
  const [agentPageUnavailable, setAgentPageUnavailable] = createSignal(false);
  const cursor = () => search.cursor as string | undefined;
  const limit = () => search.limit === undefined
    ? AUTOMATION_PAGE_LIMIT
    : typeof search.limit === "string" && /^(?:[1-9]|[1-9][0-9]|100)$/.test(search.limit)
      ? Number(search.limit)
      : Number.NaN;
  const page = createAsync(
    () => getAutomations(cursor() === undefined
      ? { limit: limit() }
      : { limit: limit(), cursor: cursor() }),
    { deferStream: true },
  );
  const firstAgentPage = createAsync(async () => {
    try {
      return await getAgentChoices({ limit: 100 });
    } catch (error) {
      if (error instanceof Response) throw error;
      return null;
    }
  }, { deferStream: true });
  const allAgents = createMemo(() => mergeAutomationAgentPages(
    firstAgentPage()?.items,
    agentPages(),
  ));
  const hostedAgents = createMemo(() => allAgents().filter((agent) =>
    agent.owner_principal_id === viewer.principalId && agent.status === "active" &&
    agent.runtime_ref === "hosted:luna"));
  const nextAgentCursor = () => nextCursorForPages(firstAgentPage(), agentPages());
  const loadMoreAgents = async () => {
    const cursor = nextAgentCursor();
    if (!cursor || loadingMoreAgents()) return;
    setLoadingMoreAgents(true);
    setAgentPageUnavailable(false);
    try {
      const page = await getAgentChoices({ cursor, limit: 100 });
      if (nextAgentCursor() === cursor) setAgentPages((pages) => [...pages, page]);
    } catch {
      setAgentPageUnavailable(true);
    } finally {
      setLoadingMoreAgents(false);
    }
  };

  return (
    <section aria-labelledby="automations-heading" class="automation-screen">
      <Title>Automations · Myelin</Title>
      <header class="automation-heading-row">
        <div>
          <p class="automation-eyebrow">Governed agent work</p>
          <h1 id="automations-heading"><Icon name="run" /> Automations</h1>
        </div>
        <div class="automation-actions">
          <Show when={search.cursor}>
            <A href="/automations" class="automation-button automation-button-secondary">
              <Icon name="cycle" /> Latest
            </A>
          </Show>
          <button type="button" class="automation-button automation-button-primary" onClick={() => setCreating(true)}>
            <Icon name="run" /> New automation
          </button>
        </div>
      </header>
      <p class="automation-intro">
        Platform events wake narrowly delegated agents. Myelin supplies their integrations and keeps
        every firing, approval, budget, and result attached to the owning human.
      </p>

      <ErrorBoundary
        fallback={(error, reset) => (
          <AutomationErrorState kind={automationErrorKind(error)} onRetry={reset} />
        )}
      >
        <Suspense fallback={<Skeleton label="Loading automations…" rows={5} rowHeight="4.5rem" />}>
          <Show when={page()} keyed>
            {(view) => (
              <Show
                when={view.items.length > 0}
                fallback={
                  <div role="note" class="automation-empty" data-testid="automations-empty">
                    <Icon name="agent" size={28} title="No automations" />
                    <h2>No automations yet</h2>
                    <p>
                      Create one here with a hosted named agent. The resulting governed work stays
                      visible without configuring a provider API key.
                    </p>
                  </div>
                }
              >
                <ul class="automation-list" data-testid="automations-list">
                  <For each={view.items}>{(item) => <AutomationRow automation={item} />}</For>
                </ul>
                <Show when={view.page.next_cursor}>
                  {(next) => (
                    <nav aria-label="Automation pages" class="automation-pagination">
                      <A
                        href={automationPageHref(next(), view.page.limit)}
                        class="automation-button automation-button-secondary"
                        data-testid="automations-next"
                      >
                        Older automations <Icon name="chevron" />
                      </A>
                    </nav>
                  )}
                </Show>
              </Show>
            )}
          </Show>
        </Suspense>
      </ErrorBoundary>
      <AutomationCreateDialog
        open={creating()}
        agents={hostedAgents()}
        agentsLoading={firstAgentPage() === undefined}
        agentsHaveMore={Boolean(nextAgentCursor())}
        agentsLoadingMore={loadingMoreAgents()}
        agentsUnavailable={firstAgentPage() === null}
        earlierAgentsUnavailable={agentPageUnavailable()}
        onLoadMoreAgents={() => void loadMoreAgents()}
        onRetryAgents={() => {
          setAgentPages([]);
          setAgentPageUnavailable(false);
          void revalidate(getAgentChoices.keyFor({ limit: 100 }));
        }}
        onClose={() => setCreating(false)}
        onCreated={(receipt) => {
          void revalidate("automations");
          navigate(`/automations/${encodeURIComponent(receipt.trigger.id)}`);
        }}
      />
    </section>
  );
}

function AutomationRow(props: { automation: AutomationVM }) {
  return (
    <li>
      <A
        href={`/automations/${props.automation.id}`}
        class="automation-row"
        data-testid="automation-row"
      >
        <AutomationStateLabel state={props.automation.state} />
        <span class="automation-row-main">
          <strong>{props.automation.event_type}</strong>
          <span>{props.automation.task}</span>
          <Show when={props.automation.last_evaluation_error}>
            <span class="automation-row-error">
              <Icon name="check-fail" /> Latest event could not be evaluated
            </span>
          </Show>
        </span>
        <span class="automation-row-meta">
          <span>{props.automation.firings_used} / {props.automation.max_firings} firings</span>
          <code>agent:{props.automation.run_as_agent_id.slice(0, 8)}</code>
        </span>
      </A>
    </li>
  );
}

function automationPageHref(cursor: string, limit: number): string {
  const query = new URLSearchParams({ cursor });
  if (limit !== AUTOMATION_PAGE_LIMIT) query.set("limit", String(limit));
  return `/automations?${query.toString()}`;
}

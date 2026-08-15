import { ErrorBoundary, For, Show, Suspense } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, useSearchParams } from "@solidjs/router";
import { Icon, Skeleton } from "@myelin/design-system";
import { AutomationErrorState, automationErrorKind } from "~/components/AutomationErrorState";
import { AutomationStateLabel } from "~/components/AutomationStateLabel";
import { getAutomations } from "~/lib/automation-api";
import {
  AUTOMATION_PAGE_LIMIT,
  type AutomationVM,
} from "~/lib/automation-response";

export default function AutomationsIndex() {
  const [search] = useSearchParams();
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

  return (
    <section aria-labelledby="automations-heading" class="automation-screen">
      <Title>Automations · Myelin</Title>
      <header class="automation-heading-row">
        <div>
          <p class="automation-eyebrow">Governed agent work</p>
          <h1 id="automations-heading"><Icon name="run" /> Automations</h1>
        </div>
        <Show when={search.cursor}>
          <A href="/automations" class="automation-button automation-button-secondary">
            <Icon name="cycle" /> Latest
          </A>
        </Show>
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
                      Start with <code>myelin automation create</code>. The resulting governed work
                      will be visible here without configuring a provider API key.
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

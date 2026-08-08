// CT-005 CI run front door. Durable server-side data only: the browser never receives a bearer token,
// and pagination is the Edge's opaque repository-visibility-bound keyset cursor.
import { ErrorBoundary, For, Show, Suspense } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, useSearchParams } from "@solidjs/router";
import { Icon, Skeleton } from "@myelin/design-system";
import { CiRouteError, getCiRuns } from "~/lib/api";
import { CI_RUN_STATES, type CiRunStateFilter } from "~/lib/ci-read-input";
import {
  ciRunsHref,
  ciRunsInputFromSearch,
  CI_WEB_PAGE_LIMIT,
} from "~/lib/ci-list-state";
import {
  ciRepoLabel,
  type CiRunVM,
} from "~/lib/ci-read-response";
import { CiErrorState, ciErrKind } from "~/components/CiErrorState";
import { CiStatus, ciStatusView, formatCiDate } from "~/components/CiStatus";

export default function CIIndex() {
  const [search] = useSearchParams();
  const runs = createAsync(async () => {
    const input = ciRunsInputFromSearch(search.state, search.limit, search.cursor);
    if (!input) throw new CiRouteError("bad-input");
    return getCiRuns(input);
  }, { deferStream: true });
  const activeState = (): CiRunStateFilter =>
    typeof search.state === "string" && CI_RUN_STATES.includes(search.state as CiRunStateFilter)
      ? search.state as CiRunStateFilter
      : "all";
  const activeLimit = () =>
    typeof search.limit === "string" && /^(?:[1-9]|[1-9][0-9]|100)$/.test(search.limit)
      ? Number(search.limit)
      : CI_WEB_PAGE_LIMIT;

  return (
    <section aria-labelledby="ci-runs-heading" class="ci-screen">
      <Title>CI · Myelin</Title>
      <header class="ci-heading-row">
        <div>
          <p class="ci-eyebrow">Durable run history</p>
          <h1 id="ci-runs-heading"><Icon name="nav-ci" /> CI runs</h1>
        </div>
        <Show when={search.cursor}>
          <A href={ciRunsHref({ state: activeState(), limit: activeLimit() })} class="ci-secondary-action">
            <Icon name="cycle" /> Latest runs
          </A>
        </Show>
      </header>

      <form method="get" action="/ci" class="ci-filter">
        <label>
          <span>Run state</span>
          <select id="ci-state" name="state" value={activeState()}>
            <For each={CI_RUN_STATES}>
              {(state) => <option value={state}>{stateLabel(state)}</option>}
            </For>
          </select>
        </label>
        <Show when={activeLimit() !== CI_WEB_PAGE_LIMIT}>
          <input type="hidden" name="limit" value={activeLimit()} />
        </Show>
        <button type="submit">Apply filter</button>
      </form>

      <ErrorBoundary
        fallback={(error, reset) => (
          <CiErrorState
            kind={ciErrKind(error)}
            latestHref={ciRunsHref({ state: activeState(), limit: activeLimit() })}
            onRetry={reset}
          />
        )}
      >
        <Suspense
          fallback={
            <Skeleton
              label="Loading CI runs…"
              rows={5}
              rowHeight="4.5rem"
              data-testid="ci-runs-loading"
            />
          }
        >
          <Show when={runs()} keyed>
            {(page) => (
              <Show
                when={page.items.length > 0}
                fallback={
                  <div role="note" data-testid="ci-runs-empty" class="ci-empty">
                    <Icon name="run" size={28} title="No runs" />
                    <h2>{activeState() === "all" ? "No authorized runs yet" : `No ${stateLabel(activeState()).toLowerCase()} runs`}</h2>
                    <p>Runs appear here after a visible repository triggers Myelin CI.</p>
                  </div>
                }
              >
                <ul data-testid="ci-runs-list" class="ci-run-list">
                  <For each={page.items}>{(run) => <RunRow run={run} />}</For>
                </ul>
                <Show when={page.page.next_cursor}>
                  {(next) => (
                    <nav aria-label="CI run pages" class="ci-pagination">
                      <A
                        data-testid="ci-runs-next"
                        href={ciRunsHref({
                          state: activeState(),
                          limit: page.page.limit,
                          cursor: next(),
                        })}
                        class="ci-secondary-action"
                      >
                        Older runs <Icon name="chevron" />
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

function RunRow(props: { run: CiRunVM }) {
  return (
    <li>
      <A href={`/ci/runs/${props.run.run_id}`} data-testid="ci-run-row" class="ci-run-row">
        <CiStatus state={props.run.state} />
        <span class="ci-run-main">
          <strong>{ciRepoLabel(props.run.repo_ref)}</strong>
          <span>
            <code>{props.run.commit_oid ?? "no commit"}</code>
            <span aria-hidden="true"> · </span>
            {triggerLabel(props.run.trigger_kind)}
          </span>
        </span>
        <span class="ci-run-meta">
          <time datetime={props.run.created_at}>{formatCiDate(props.run.created_at)}</time>
          <code title={`Run ${props.run.run_id}`}>{props.run.run_id.slice(0, 8)}</code>
        </span>
      </A>
    </li>
  );
}

function stateLabel(state: CiRunStateFilter): string {
  return state === "all" ? "All states" : ciStatusView(state).label;
}

function triggerLabel(trigger: CiRunVM["trigger_kind"]): string {
  return trigger.replaceAll("_", " ");
}

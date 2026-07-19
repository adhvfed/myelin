import { Title } from "@solidjs/meta";
import {
  A,
  createAsync,
  revalidate,
  useAction,
  useNavigate,
  useSearchParams,
} from "@solidjs/router";
import {
  Icon,
  Skeleton,
  SkeletonBlock,
  StatusPill,
  useToast,
} from "@myelin/design-system";
import {
  ErrorBoundary,
  For,
  Show,
  Suspense,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
} from "solid-js";
import { IssueCreateDialog } from "~/components/issues/IssueCreateDialog";
import {
  getIssues,
  issuesMutate,
  type IssueCreateReceipt,
  type IssueErrorKind,
  type IssueListState,
  type IssuesPage,
  type IssueVM,
} from "~/lib/api";
import {
  issueErrorKind,
  issueKeyError,
  issueListHref,
  issueListState,
  issueTimestamp,
  mergeIssuePages,
  normalizeIssueKey,
  pollIssueActivation,
  type PendingIssue,
} from "~/lib/issue-view";

const STATES: { key: IssueListState; label: string }[] = [
  { key: "open", label: "Open" },
  { key: "closed", label: "Closed" },
  { key: "all", label: "All" },
];

export default function IssuesIndex() {
  const [search] = useSearchParams();
  const navigate = useNavigate();
  const toast = useToast();
  const act = useAction(issuesMutate);
  const state = () => issueListState(search.state);
  const rawKey = () => (typeof search.key === "string" ? search.key : "");
  const key = () => normalizeIssueKey(rawKey());
  const invalidUrlKey = () => Boolean(rawKey() && !key());
  const createOpen = () => search.new === "1";
  const [keyDraft, setKeyDraft] = createSignal(rawKey());
  const [keyFormError, setKeyFormError] = createSignal<string | null>(null);
  const [extraPages, setExtraPages] = createSignal<IssuesPage[]>([]);
  const [loadingMore, setLoadingMore] = createSignal(false);
  const [loadMoreError, setLoadMoreError] = createSignal(false);
  const [pending, setPending] = createSignal<PendingIssue[]>([]);
  const [activeRow, setActiveRow] = createSignal(0);
  const pollControllers = new Set<AbortController>();
  let filterGeneration = 0;
  let loadMoreRequest = 0;
  let loadMoreController: AbortController | undefined;

  const resetPagination = () => {
    filterGeneration += 1;
    loadMoreRequest += 1;
    loadMoreController?.abort();
    loadMoreController = undefined;
    setExtraPages([]);
    setLoadingMore(false);
    setLoadMoreError(false);
    setActiveRow(0);
  };

  onCleanup(() => {
    loadMoreController?.abort();
    pollControllers.forEach((controller) => controller.abort());
    pollControllers.clear();
  });

  createEffect(() => {
    setKeyDraft(rawKey());
    state();
    key();
    resetPagination();
  });

  const firstPage = createAsync(async (): Promise<{
    page: IssuesPage | null;
    error: IssueErrorKind | null;
  }> => {
    if (invalidUrlKey()) return { page: null, error: "bad-input" };
    try {
      return {
        page: await getIssues({ state: state(), key: key(), limit: 50 }),
        error: null,
      };
    } catch (error) {
      return { page: null, error: issueErrorKind(error) };
    }
  });
  const rows = createMemo(() => mergeIssuePages(firstPage()?.page ?? undefined, extraPages()));
  const nextCursor = () => {
    const pages = extraPages();
    return pages.length
      ? pages[pages.length - 1]?.page.next_cursor ?? null
      : firstPage()?.page?.page.next_cursor ?? null;
  };
  const visiblePending = () => pending().filter((receipt) => {
    if (state() === "closed") return false;
    const filter = key();
    return !filter || receipt.key.startsWith(filter);
  });

  const closeCreate = () => navigate(issueListHref({ state: state(), key: key() }), { replace: true });
  const openCreate = () => navigate(issueListHref({ state: state(), key: key(), create: true }));

  const submitKey = (event: SubmitEvent) => {
    event.preventDefault();
    const error = issueKeyError(keyDraft());
    if (error) {
      setKeyFormError(error);
      return;
    }
    setKeyFormError(null);
    navigate(issueListHref({ state: state(), key: keyDraft() }));
  };

  const loadMore = async () => {
    const cursor = nextCursor();
    if (!cursor || loadingMore()) return;
    const generation = filterGeneration;
    const request = ++loadMoreRequest;
    const controller = new AbortController();
    loadMoreController?.abort();
    loadMoreController = controller;
    const requestState = state();
    const requestKey = key();
    setLoadingMore(true);
    setLoadMoreError(false);
    try {
      const page = await getIssues({ state: requestState, key: requestKey, cursor, limit: 50 });
      if (
        controller.signal.aborted ||
        generation !== filterGeneration ||
        request !== loadMoreRequest
      ) return;
      setExtraPages((pages) => [...pages, page]);
    } catch {
      if (
        !controller.signal.aborted &&
        generation === filterGeneration &&
        request === loadMoreRequest
      ) setLoadMoreError(true);
    } finally {
      if (
        !controller.signal.aborted &&
        generation === filterGeneration &&
        request === loadMoreRequest
      ) {
        loadMoreController = undefined;
        setLoadingMore(false);
      }
    }
  };

  const focusRow = (index: number, elements: HTMLAnchorElement[]) => {
    const count = elements.length;
    if (!count) return;
    const next = ((index % count) + count) % count;
    setActiveRow(next);
    elements[next]?.focus();
  };

  const onListKeyDown = (event: KeyboardEvent) => {
    const current = event.currentTarget as HTMLAnchorElement;
    const elements = Array.from(
      current.closest(".issues-list")?.querySelectorAll<HTMLAnchorElement>("[data-testid='issue-row']") ?? [],
    );
    const index = elements.indexOf(current);
    if (index < 0) return;
    if (event.key === "ArrowDown" || event.key === "j") {
      event.preventDefault();
      focusRow(index + 1, elements);
    } else if (event.key === "ArrowUp" || event.key === "k") {
      event.preventDefault();
      focusRow(index - 1, elements);
    } else if (event.key === "Home") {
      event.preventDefault();
      focusRow(0, elements);
    } else if (event.key === "End") {
      event.preventDefault();
      focusRow(elements.length - 1, elements);
    }
  };

  const onTabKeyDown = (event: KeyboardEvent, index: number) => {
    if (event.key !== "ArrowRight" && event.key !== "ArrowLeft") return;
    event.preventDefault();
    const direction = event.key === "ArrowRight" ? 1 : -1;
    const next = ((index + direction) % STATES.length + STATES.length) % STATES.length;
    document.getElementById(`issues-state-tab-${next}`)?.focus();
  };

  const accepted = (receipt: IssueCreateReceipt) => {
    const item: PendingIssue = {
      id: receipt.issue.id,
      key: receipt.issue.key,
      requestEventId: receipt.authorization.request_event_id,
      phase: "pending",
    };
    setPending((items) => [item, ...items.filter((entry) => entry.id !== item.id)]);
    toast.show({ title: `${item.key} accepted — activating access…`, variant: "info" });
    const controller = new AbortController();
    pollControllers.add(controller);
    void pollIssueActivation(
      item.requestEventId,
      async (requestEventId) => {
        const result = await act({ op: "activation", requestEventId });
        if (!result.ok || result.op !== "activation") throw new Error("activation unavailable");
        return result.status;
      },
      { signal: controller.signal },
    ).then((outcome) => {
      if (controller.signal.aborted) return;
      if (outcome.phase === "active") {
        setPending((items) => items.filter((entry) => entry.id !== item.id));
        resetPagination();
        void revalidate("issues-list");
        toast.show({ title: `${item.key} is ready`, variant: "success" });
      } else {
        setPending((items) => items.map((entry) =>
          entry.id === item.id ? { ...entry, phase: "unconfirmed" } : entry,
        ));
      }
    }).catch(() => {
      if (!controller.signal.aborted) {
        setPending((items) => items.map((entry) =>
          entry.id === item.id ? { ...entry, phase: "unconfirmed" } : entry,
        ));
      }
    }).finally(() => pollControllers.delete(controller));
  };

  return (
    <section aria-labelledby="issues-heading" class="issues-screen">
      <Title>Issues · Myelin</Title>
      <header class="issues-heading-row">
        <div>
          <p class="issues-eyebrow">Myelin dogfood</p>
          <h1 id="issues-heading"><Icon name="issue" /> Issues</h1>
        </div>
        <button type="button" class="issues-button issues-button-primary" onClick={openCreate}>
          <Icon name="issue" /> New issue
        </button>
      </header>

      <div class="issues-toolbar">
        <form class="issues-search" role="search" onSubmit={submitKey}>
          <span id="issues-key-label">Find by issue key</span>
          <div class="issues-search-controls">
            <span aria-hidden="true"><Icon name="search" /></span>
            <input
              id="issues-key-search"
              type="search"
              value={keyDraft()}
              onInput={(event) => {
                setKeyDraft(event.currentTarget.value);
                setKeyFormError(null);
              }}
              placeholder="MYL-"
              autocomplete="off"
              aria-labelledby="issues-key-label"
              aria-invalid={Boolean(keyFormError())}
              aria-describedby={keyFormError() ? "issues-key-error" : "issues-key-help"}
            />
            <button type="submit" class="issues-button issues-button-secondary">Find</button>
          </div>
          <p id="issues-key-help" class="issues-field-hint">Key prefix only; titles are encrypted and not searched.</p>
          <Show when={keyFormError()}>
            {(message) => <p id="issues-key-error" role="alert" class="issues-field-error">{message()}</p>}
          </Show>
        </form>

        <div role="tablist" aria-label="Filter issues by state" class="issues-tabs">
          <For each={STATES}>
            {(tab, index) => (
              <A
                id={`issues-state-tab-${index()}`}
                role="tab"
                href={issueListHref({ state: tab.key, key: key() })}
                aria-selected={state() === tab.key}
                tabindex={state() === tab.key ? 0 : -1}
                onKeyDown={(event) => onTabKeyDown(event, index())}
              >
                {tab.label}
              </A>
            )}
          </For>
        </div>
      </div>

      <Show when={visiblePending().length > 0}>
        <section aria-labelledby="issues-activating-heading" class="issues-pending-group">
          <h2 id="issues-activating-heading">Activating</h2>
          <ul>
            <For each={visiblePending()}>
              {(receipt) => (
                <li data-testid="pending-issue">
                  <code>{receipt.key}</code>
                  <span role="status" aria-live="polite" aria-atomic="true">
                    <Icon name="cycle" /> {receipt.phase === "pending" ? "Activating access…" : "Activation could not be confirmed"}
                  </span>
                  <Show when={receipt.phase === "unconfirmed"}>
                    <small>No failure is inferred. Refresh the list later.</small>
                  </Show>
                </li>
              )}
            </For>
          </ul>
        </section>
      </Show>

      <ErrorBoundary fallback={(error) => <IssuesListError kind={issueErrorKind(error)} clear={() => navigate("/issues")} />}>
        <Suspense fallback={<IssuesSkeleton />}>
          <Show when={firstPage()?.error} fallback={<Show when={firstPage()?.page}>
            <Show
              when={rows().length > 0}
              fallback={<IssuesEmpty state={state()} filtered={Boolean(key())} clear={() => navigate("/issues")} create={openCreate} />}
            >
              <p class="sr-only" role="status" aria-live="polite">{rows().length} issues shown.</p>
              <ul class="issues-list" data-testid="issues-list">
                <For each={rows()}>{(issue, index) => (
                  <IssueRow
                    issue={issue}
                    active={activeRow() === index()}
                    onFocus={() => setActiveRow(index())}
                    onKeyDown={onListKeyDown}
                  />
                )}</For>
              </ul>
              <Show when={nextCursor()}>
                <div class="issues-load-more">
                  <button type="button" class="issues-button issues-button-secondary" onClick={() => void loadMore()} disabled={loadingMore()}>
                    <Icon name="chevron" /> {loadingMore() ? "Loading…" : "Load more"}
                  </button>
                  <Show when={loadMoreError()}>
                    <span role="alert">We couldn't load more issues. Try again.</span>
                  </Show>
                </div>
              </Show>
            </Show>
          </Show>}>
            {(kind) => <IssuesListError kind={kind()} clear={() => navigate("/issues")} />}
          </Show>
        </Suspense>
      </ErrorBoundary>

      <IssueCreateDialog open={createOpen()} onClose={closeCreate} onAccepted={accepted} />
    </section>
  );
}

function IssueRow(props: {
  issue: IssueVM;
  active: boolean;
  onFocus: () => void;
  onKeyDown: (event: KeyboardEvent) => void;
}) {
  return (
    <li>
      <A
        href={`/issues/${encodeURIComponent(props.issue.id)}`}
        class="issue-row"
        data-testid="issue-row"
        tabindex={props.active ? 0 : -1}
        onFocus={props.onFocus}
        onKeyDown={props.onKeyDown}
      >
        <code class="issue-row-key">{props.issue.key}</code>
        <span class="issue-row-title">{props.issue.title}</span>
        <StatusPill kind="issue-state" category={props.issue.state_category} label={props.issue.state} />
        <time datetime={props.issue.updated_at} class="issue-row-time">{issueTimestamp(props.issue.updated_at)}</time>
      </A>
    </li>
  );
}

function IssuesSkeleton() {
  return (
    <Skeleton label="Loading issues…" rows={4} rowHeight="3.25rem" data-testid="issues-loading">
      <For each={[0, 1, 2, 3]}>{() => (
        <div class="issue-row">
          <SkeletonBlock width="5rem" height="0.8rem" />
          <SkeletonBlock width="60%" height="0.9rem" />
          <SkeletonBlock width="4rem" height="1rem" radius="var(--radius-pill)" />
        </div>
      )}</For>
    </Skeleton>
  );
}

function IssuesEmpty(props: {
  state: IssueListState;
  filtered: boolean;
  clear: () => void;
  create: () => void;
}) {
  const heading = () => props.filtered
    ? "No issues match these filters"
    : props.state === "open"
      ? "No open issues"
      : props.state === "closed"
        ? "No closed issues"
        : "No issues yet";
  const description = () => props.filtered
    ? "Try another issue key or state."
    : props.state === "open"
      ? "Nothing is currently open."
      : props.state === "closed"
        ? "Nothing has reached a closed state."
        : "Capture the first rough edge in Myelin.";
  const createAction = () => props.state === "open" || props.state === "all";
  return (
    <div class="issues-empty" data-testid={props.filtered ? "issues-no-results" : props.state === "all" ? "issues-empty" : "issues-state-empty"}>
      <Icon name="issue" />
      <h2>{heading()}</h2>
      <p>{description()}</p>
      <button type="button" class="issues-button issues-button-secondary" onClick={() => props.filtered || !createAction() ? props.clear() : props.create()}>
        {props.filtered ? "Clear filters" : createAction() ? props.state === "all" ? "Create the first issue" : "Create issue" : "View open issues"}
      </button>
    </div>
  );
}

function IssuesListError(props: { kind: ReturnType<typeof issueErrorKind>; clear: () => void }) {
  const invalid = () => props.kind === "bad-input";
  return (
    <div role="alert" class="issues-error" data-testid="issues-error">
      <Icon name="check-fail" title="Error" />
      <div>
        <h2>{invalid() ? "That issue-key filter isn't valid" : props.kind === "unavailable" ? "Issue authorization is catching up" : "We couldn't load issues"}</h2>
        <p>{invalid() ? "Use letters, numbers, and hyphens only." : props.kind === "unavailable" ? "The list stays closed until the authorization projection is current." : "Something went wrong on our side."}</p>
        <button type="button" class="issues-button issues-button-secondary" onClick={() => invalid() ? props.clear() : location.reload()}>
          {invalid() ? "Clear search" : "Retry"}
        </button>
      </div>
    </div>
  );
}

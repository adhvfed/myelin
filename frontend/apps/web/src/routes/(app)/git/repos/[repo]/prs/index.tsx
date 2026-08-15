// Repository PR list with filtering, sorting, cursor pagination, and roving keyboard focus. The
// server applies repository visibility before returning rows.
import { ErrorBoundary, For, Show, Suspense, createSignal, createEffect } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, useParams, useSearchParams, useNavigate } from "@solidjs/router";
import { Icon, Skeleton, SkeletonBlock, StatusPill, Menu, type MenuItemSpec } from "@myelin/design-system";
import { getRepoPrs, type PrListRowVM, type PrListPage } from "~/lib/api";
import { prTitleText, isTitleFallback, updatedLabel, stateTabs, reviewMarker, isFilteredNoResults } from "~/lib/pr-view";
import { NotAvailable } from "~/components/NotAvailable";
import { gitRepositoryPath, parseGitRepositoryRouteParam } from "~/lib/git-route";

const STATES = ["open", "merged", "closed", "all"] as const;

export default function RepoPrListScreen() {
  const params = useParams();
  const [search] = useSearchParams();
  const navigate = useNavigate();

  const state = () => {
    const s = typeof search.state === "string" ? search.state : "open";
    return (STATES as readonly string[]).includes(s) ? s : "open";
  };
  const sort = () => (search.sort === "created" ? "created" : "updated");
  const cursor = () => (typeof search.cursor === "string" ? search.cursor : undefined);
  const repo = () => parseGitRepositoryRouteParam(params.repo) ?? "";
  const repoPath = () => gitRepositoryPath(repo());
  const ready = () => repo() !== "";

  const data = createAsync(
    async () => {
      return ready()
        ? getRepoPrs({ repo: repo(), state: state(), sort: sort(), cursor: cursor() })
        : undefined;
    },
    { deferStream: true },
  );

  const href = (patch: { state?: string; sort?: string; cursor?: string }) => {
    const nextState = patch.state ?? state();
    const nextSort = patch.sort ?? sort();
    const p = new URLSearchParams();
    if (nextState !== "open") p.set("state", nextState);
    if (nextSort !== "updated") p.set("sort", nextSort);
    if (patch.cursor) p.set("cursor", patch.cursor);
    const q = p.toString();
    return `${repoPath()}/prs${q ? `?${q}` : ""}`;
  };

  const sortItems = (): MenuItemSpec[] => [
    { label: "Recently updated", icon: "cycle", onSelect: () => navigate(href({ sort: "updated", cursor: undefined })) },
    { label: "Recently created", icon: "commit", onSelect: () => navigate(href({ sort: "created", cursor: undefined })) },
  ];

  return (
    <section aria-labelledby="prs-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-3)" }}>
      <Title>Pull requests · {repo()} · Myelin</Title>
      <nav aria-label="Breadcrumb" style={{ "font-size": "var(--fs-caption)", display: "flex", gap: "var(--space-1)" }}>
        <A href="/git/repos" style={{ color: "var(--text-muted)" }}>Repositories</A>
        <span aria-hidden="true">/</span>
        <A href={repoPath()} style={{ color: "var(--text-muted)" }}>{repo()}</A>
        <span aria-hidden="true">/</span>
        <span aria-current="page" style={{ color: "var(--text-muted)" }}>Pull requests</span>
      </nav>

      <div style={{ display: "flex", "align-items": "center", gap: "var(--space-3)", "flex-wrap": "wrap" }}>
        <h1 id="prs-heading" style={{ "font-size": "var(--fs-h1)", margin: "0", display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
          <Icon name="pull-request" /> Pull requests
        </h1>
        <div style={{ flex: "1" }} />
        <A href={`${repoPath()}/commits/main`} style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--text-muted)", "text-decoration": "none" }}>
          <Icon name="commit" /> Commits
        </A>
      </div>

      <ErrorBoundary
        fallback={() => (
          // System-blaming, scoped to the list, filters kept — never a raw err.message (ux-git #7).
          <div role="alert" data-testid="prs-error" style={errBox}>
            <Icon name="check-fail" title="Error" />
            <div>
              <strong style={{ display: "block", color: "var(--text-primary)" }}>We couldn't load pull requests</strong>
              <span style={{ color: "var(--text-muted)" }}>Something went wrong on our side. Your filters are kept — this is scoped to the list; the rest of the page is still live.</span>
              <div>
                <button type="button" onClick={() => location.reload()} style={retryBtn}>
                  <Icon name="cycle" /> Retry
                </button>
              </div>
            </div>
          </div>
        )}
      >
        <Suspense
          fallback={
            <Skeleton label="Loading pull requests…" data-testid="prs-loading" style={{ gap: "0" }}>
              <For each={[0, 1, 2, 3, 4]}>
                {() => (
                  <div style={{ display: "grid", "grid-template-columns": "auto 1fr auto", gap: "var(--space-3)", padding: "var(--space-3)", "border-block-end": "var(--hairline) solid var(--border)" }}>
                    <SkeletonBlock height="1.1rem" width="4rem" radius="var(--radius-pill)" />
                    <div style={{ display: "flex", "flex-direction": "column", gap: "var(--space-1)" }}>
                      <SkeletonBlock height="0.8rem" width="52%" />
                      <SkeletonBlock height="0.6rem" width="34%" />
                    </div>
                    <SkeletonBlock height="0.7rem" width="4.5rem" />
                  </div>
                )}
              </For>
            </Skeleton>
          }
        >
          <Show when={ready()} fallback={<NotAvailable kind="pull request list" status="missing" />}>
            <Show when={data()} keyed>
              {(result) => (
                <Show
                  when={!("restricted" in result)}
                  fallback={
                    // Do not reveal whether the repository contains PRs.
                    <div role="note" data-testid="prs-restricted" style={stateBox}>
                      <Icon name="gate" title="Restricted" />
                      <div>
                        <strong style={{ display: "block", color: "var(--text-primary)" }}>Pull requests are not available to you</strong>
                        <span style={{ color: "var(--text-muted)" }}>
                          You don't have access to pull requests in <code style={mono}>{repo()}</code>. If you think this is wrong, ask a repository admin for the <code style={mono}>view</code> permission.
                        </span>
                      </div>
                    </div>
                  }
                >
                  <PopulatedOrEmpty
                    page={result as PrListPage}
                    repo={repo()}
                    activeState={state()}
                    hrefFor={href}
                    sortItems={sortItems()}
                    sortLabel={sort() === "created" ? "Recently created" : "Recently updated"}
                  />
                </Show>
              )}
            </Show>
          </Show>
        </Suspense>
      </ErrorBoundary>
    </section>
  );
}

function PopulatedOrEmpty(props: {
  page: PrListPage;
  repo: string;
  activeState: string;
  hrefFor: (patch: { state?: string; sort?: string; cursor?: string }) => string;
  sortItems: MenuItemSpec[];
  sortLabel: string;
}) {
  const items = () => props.page.items;
  const counts = () => props.page.counts;
  const tabs = () => stateTabs(counts());
  const total = () => props.page.page.total ?? items().length;
  const offset = () => props.page.page.offset ?? 0;

  // The list has one tab stop; arrows and j/k move it, and Tab exits the list.
  const [active, setActive] = createSignal(0);
  const rowEls: (HTMLAnchorElement | undefined)[] = [];
  createEffect(() => {
    // Reset the index after pagination or filtering without clearing the assigned row refs.
    items();
    setActive(0);
  });
  const focusRow = (i: number) => {
    const len = items().length;
    if (len === 0) return;
    const n = ((i % len) + len) % len;
    setActive(n);
    rowEls[n]?.focus();
  };
  const onListKeyDown = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "j") {
      e.preventDefault();
      focusRow(active() + 1);
    } else if (e.key === "ArrowUp" || e.key === "k") {
      e.preventDefault();
      focusRow(active() - 1);
    } else if (e.key === "Home") {
      e.preventDefault();
      focusRow(0);
    } else if (e.key === "End") {
      e.preventDefault();
      focusRow(items().length - 1);
    }
  };

  const onTabsKeyDown = (e: KeyboardEvent, idx: number, count: number) => {
    // ARIA tablist roving: Left/Right move focus between tabs (each is a deep-linkable route).
    if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
      e.preventDefault();
      const n = e.key === "ArrowRight" ? idx + 1 : idx - 1;
      const wrapped = ((n % count) + count) % count;
      (document.getElementById(`prs-tab-${wrapped}`) as HTMLElement | null)?.focus();
    }
  };

  return (
    <>
      {/* Filter tabs (ARIA tablist — contains ONLY tabs) + sort Menu, side by side. */}
      <div style={barRow}>
        <div role="tablist" aria-label="Filter pull requests by state" style={{ display: "flex", "align-items": "center", gap: "var(--space-1)" }}>
          <For each={tabs()}>
            {(t, i) => {
              const selected = () => t.key === props.activeState;
              return (
                <A
                  id={`prs-tab-${i()}`}
                  role="tab"
                  aria-selected={selected() ? "true" : "false"}
                  tabindex={selected() ? 0 : -1}
                  href={props.hrefFor({ state: t.key, cursor: undefined })}
                  onKeyDown={(e) => onTabsKeyDown(e, i(), tabs().length)}
                  style={{
                    display: "inline-flex", "align-items": "center", gap: "var(--space-1)",
                    padding: "var(--space-2) var(--space-3)", "text-decoration": "none",
                    color: selected() ? "var(--text-primary)" : "var(--text-subtle)",
                    "border-block-end": selected() ? "2px solid var(--accent)" : "2px solid transparent",
                    "font-weight": selected() ? "var(--weight-medium)" : "var(--weight-regular)",
                  }}
                >
                  {t.label} <span style={{ "font-family": "var(--font-mono)", "font-size": "var(--fs-caption)", color: "var(--text-subtle)" }}>{t.count}</span>
                </A>
              );
            }}
          </For>
        </div>
        <div style={{ flex: "1" }} />
        <Menu label="Sort pull requests" placement="bottom-end" items={props.sortItems}
          triggerLabel={
            <span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>
              Sort: {props.sortLabel} <Icon name="chevron" />
            </span>
          }
        />
      </div>

      <Show
        when={items().length > 0}
        fallback={
          <Show
            when={isFilteredNoResults(items().length, counts())}
            fallback={
              // Empty (teaching) — next-action forward. Distinct from filtered-no-results.
              <div data-testid="prs-empty" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)", padding: "var(--space-4)", "max-width": "60ch" }}>
                <span style={{ color: "var(--text-muted)" }}><Icon name="pull-request" /></span>
                <h2 style={{ "font-size": "var(--fs-h3)", margin: "0" }}>No open pull requests</h2>
                <p style={{ color: "var(--text-muted)", margin: "0" }}>
                  Create one by pushing a branch, then opening a pull request from it into <code style={mono}>main</code>.
                </p>
                <pre style={{ ...codeBox }}>{`git switch -c my-change\ngit push -u origin my-change`}</pre>
                <p style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)", margin: "0" }}>
                  Then open it here or from the command palette (<kbd style={kbd}>⌘K → New pull request</kbd>).
                </p>
              </div>
            }
          >
            <div role="status" data-testid="prs-no-results" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)", padding: "var(--space-4)", "max-width": "56ch" }}>
              <h2 style={{ "font-size": "var(--fs-h3)", margin: "0" }}>No pull requests match this filter</h2>
              <p style={{ color: "var(--text-muted)", margin: "0" }}>
                No results for <strong>{tabLabel(props.activeState)}</strong>.{" "}
                <A href={props.hrefFor({ state: "all", cursor: undefined })} style={{ color: "var(--info)", "text-decoration": "underline" }}>Clear filters</A> to see all pull requests.
              </p>
            </div>
          </Show>
        }
      >
        <p class="visually-hidden-live" role="status" aria-live="polite" style={srOnly}>
          {items().length} pull requests. Sorted by {props.sortLabel.toLowerCase()}.
        </p>
        <ul
          role="list"
          aria-label={`Pull requests, ${items().length} items`}
          style={{ "list-style": "none", margin: "0", padding: "0" }}
        >
          <For each={items()}>
            {(row, i) => (
              <li>
                <PrRow
                  row={row}
                  repo={props.repo}
                  index={i()}
                  active={active() === i()}
                  setRef={(el) => (rowEls[i()] = el)}
                  onFocus={() => setActive(i())}
                  onKeyDown={onListKeyDown}
                />
              </li>
            )}
          </For>
        </ul>

        {/* Bidirectional cursor pager + position hint. */}
        <div style={{ display: "flex", "align-items": "center", gap: "var(--space-3)", padding: "var(--space-3) 0" }}>
          <Show
            when={props.page.page.prev_cursor}
            fallback={<span aria-disabled="true" style={{ ...pagerBtn, opacity: "0.55", cursor: "not-allowed" }}><Icon name="chevron" /> Newer</span>}
          >
            {(prev) => <A href={props.hrefFor({ cursor: prev() })} style={pagerBtn}><Icon name="chevron" /> Newer</A>}
          </Show>
          <Show when={props.page.page.next_cursor}>
            {(next) => <A href={props.hrefFor({ cursor: next() })} style={pagerBtn}>Older <Icon name="chevron" /></A>}
          </Show>
          <span style={{ "margin-inline-start": "auto", color: "var(--text-subtle)", "font-size": "var(--fs-caption)", "font-variant-numeric": "tabular-nums" }}>
            Showing {items().length === 0 ? 0 : offset() + 1}–{offset() + items().length} of {total()}
          </span>
        </div>
      </Show>
    </>
  );
}

function PrRow(props: {
  row: PrListRowVM;
  repo: string;
  index: number;
  active: boolean;
  setRef: (el: HTMLAnchorElement) => void;
  onFocus: () => void;
  onKeyDown: (event: KeyboardEvent) => void;
}) {
  const row = () => props.row;
  const marker = () => reviewMarker(row());
  return (
    <A
      ref={props.setRef}
      href={`${gitRepositoryPath(props.repo)}/prs/${row().number}`}
      tabindex={props.active ? 0 : -1}
      onFocus={props.onFocus}
      onKeyDown={props.onKeyDown}
      data-testid="pr-row"
      style={{
        display: "grid", "grid-template-columns": "auto 1fr auto", "align-items": "start",
        gap: "var(--space-3)", padding: "var(--space-3)", "text-decoration": "none",
        color: "inherit", "border-block-end": "var(--hairline) solid var(--border)",
      }}
    >
      <StatusPill kind="pr-state" state={row().pr_state} />
      <span style={{ "min-width": "0", display: "flex", "flex-direction": "column", gap: "var(--space-1)" }}>
        <span style={{ "font-weight": "var(--weight-medium)", color: "var(--text-primary)", "line-height": "var(--lh-tight)" }}>
          <span style={isTitleFallback(row()) ? { color: "var(--text-subtle)", "font-family": "var(--font-mono)" } : {}}>{prTitleText(row())}</span>
          <Show when={!isTitleFallback(row())}>
            {" "}<span style={{ color: "var(--text-subtle)", "font-family": "var(--font-mono)", "font-weight": "var(--weight-regular)", "font-size": "var(--fs-caption)" }}>#{row().number}</span>
          </Show>
        </span>
        <span style={{ display: "flex", "flex-wrap": "wrap", "align-items": "center", gap: "var(--space-1) var(--space-3)", color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>
          <span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)" }}>
            <code style={refCode}>{shortRef(row().head_ref)}</code>
            <span aria-hidden="true" style={{ color: "var(--text-subtle)" }}>→</span>
            <code style={refCode}>{shortRef(row().base_ref)}</code>
          </span>
          <span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)" }}>
            <Show when={row().author_is_agent} fallback={<Icon name="human" />}>
              <Icon name="agent" title="Agent" />
              <span style={{ "font-size": "9px", "letter-spacing": "0.04em", "text-transform": "uppercase", color: "var(--agent)", border: "var(--hairline) solid var(--agent)", "border-radius": "var(--radius-1)", padding: "0 var(--space-1)" }}>Agent</span>
            </Show>
            <span style={{ color: "var(--text-muted)" }}>{authorName(row().author)}</span>
          </span>
        </span>
      </span>
      <span style={{ display: "flex", "flex-direction": "column", "align-items": "flex-end", gap: "var(--space-1)", "white-space": "nowrap", "font-size": "var(--fs-caption)" }}>
        <StatusPill
          kind="check-verdict"
          verdict={row().checks_summary.verdict}
          passing={row().checks_summary.passing}
          failing={row().checks_summary.failing}
          total={row().checks_summary.total}
          merged={row().pr_state === "merged"}
        />
        <span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-2)", color: "var(--text-subtle)" }}>
          <Show
            when={marker()}
            fallback={<span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--text-muted)" }} title={`${row().reviews} reviews`}><Icon name="message" /> {row().reviews}</span>}
          >
            {(m) => <span style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--info)" }} title="Your review requested"><Icon name="message" /> {m()}</span>}
          </Show>
          <Show when={updatedLabel(row())}>{(u) => <span style={{ "font-variant-numeric": "tabular-nums" }}>{u()}</span>}</Show>
        </span>
      </span>
    </A>
  );
}

// ── small pure helpers + shared style objects ──
function shortRef(ref: string): string {
  return ref.replace(/^refs\/heads\//, "").replace(/^refs\/tags\//, "");
}
function authorName(pseudonym: string): string {
  return pseudonym.split("@")[0] ?? pseudonym;
}
function tabLabel(state: string): string {
  return state.charAt(0).toUpperCase() + state.slice(1);
}

const mono = { "font-family": "var(--font-mono)" } as const;
const refCode = { color: "var(--text-muted)", background: "var(--surface-raised)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", padding: "0 var(--space-1)", "font-family": "var(--font-mono)", "font-size": "var(--fs-caption)" } as const;
const barRow = { display: "flex", "align-items": "center", gap: "var(--space-1)", "border-block-end": "var(--hairline) solid var(--border)" } as const;
const stateBox = { display: "flex", "align-items": "flex-start", gap: "var(--space-2)", padding: "var(--space-3)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", background: "var(--surface-raised)" } as const;
const errBox = { ...stateBox, "border-color": "var(--danger)" } as const;
const codeBox = { background: "var(--surface-raised)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", padding: "var(--space-2) var(--space-3)", "font-family": "var(--font-mono)", "font-size": "var(--fs-code)", color: "var(--text-primary)", "white-space": "pre-wrap", margin: "0" } as const;
const kbd = { background: "var(--surface-raised)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", padding: "0 var(--space-1)", "font-family": "var(--font-mono)" } as const;
const pagerBtn = { display: "inline-flex", "align-items": "center", gap: "var(--space-1)", "text-decoration": "none", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", padding: "var(--space-1) var(--space-3)", color: "var(--text-muted)" } as const;
const retryBtn = { display: "inline-flex", "align-items": "center", gap: "var(--space-1)", "margin-block-start": "var(--space-2)", border: "var(--hairline) solid var(--border-strong)", "border-radius": "var(--radius-1)", padding: "var(--space-1) var(--space-2)", color: "var(--text-primary)", background: "none", cursor: "pointer" } as const;
const srOnly = { position: "absolute", width: "1px", height: "1px", overflow: "hidden", clip: "rect(0 0 0 0)", "white-space": "nowrap" } as const;

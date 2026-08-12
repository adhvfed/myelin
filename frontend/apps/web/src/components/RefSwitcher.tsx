// Searchable branch/tag picker. Callers provide links that preserve the current repository path.
import { For, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import { A } from "@solidjs/router";
import { Icon, Popover } from "@myelin/design-system";
import { getRefs } from "~/lib/api";
import { isFullGitRef } from "~/lib/git-read-input";
import {
  RefSwitcherController,
  REF_SWITCHER_SEARCH_DEBOUNCE_MS,
  friendlyRefName,
  refOptionHref,
  visibleRefGroups,
  type RefSwitcherSnapshot,
  type SwitchRefRow,
} from "~/lib/ref-switcher-state";

export interface RefSwitcherProps {
  repo: string;
  currentRef: string;
  /** Exact current ref when the caller knows its namespace. Ambiguous short route refs stay unset. */
  currentFullRef?: string;
  /** Build the target route for a chosen ref (preserving the current surface + path). */
  hrefFor: (ref: string) => string;
}

export function RefSwitcher(props: RefSwitcherProps) {
  const [filter, setFilter] = createSignal("");
  const [state, setState] = createSignal<RefSwitcherSnapshot>({
    query: "",
    pins: [],
    rows: [],
    nextCursor: null,
    loading: true,
    capped: false,
    error: null,
  });
  const currentFullRef = createMemo(() => {
    // A short route ref may name both a branch and a tag. Keep it in the visible trigger, but never
    // guess a namespace for server pins or option selection.
    if (isFullGitRef(props.currentFullRef)) return props.currentFullRef;
    return isFullGitRef(props.currentRef) ? props.currentRef : undefined;
  });
  const controller = new RefSwitcherController(getRefs, setState);
  const groups = createMemo(() => visibleRefGroups(state()));

  createEffect(() => {
    const search = { repo: props.repo, query: filter(), current: currentFullRef() };
    controller.prepare(search);
    const timer = setTimeout(() => void controller.search(search), REF_SWITCHER_SEARCH_DEBOUNCE_MS);
    onCleanup(() => clearTimeout(timer));
  });
  onCleanup(() => controller.dispose());

  return (
    <Popover
      label="Switch branch or tag"
      placement="bottom-start"
      triggerLabel={
        <span
          style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)" }}
          data-testid="ref-switcher-trigger"
        >
          <Icon name="branch" />
          <span style={{ "font-family": "var(--font-mono)" }}>{friendlyRefName(props.currentRef)}</span>
          <Icon name="chevron" />
        </span>
      }
    >
      <div
        style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)", "min-width": "16rem", padding: "var(--space-2)" }}
      >
        {/* The filter input wrapper — :focus-within ring via the .ref-filter class (app.css). */}
        <div class="ref-filter">
          <Icon name="search" />
          <input
            type="text"
            class="ref-filter-input"
            aria-label="Search branches and tags"
            placeholder="Search refs…"
            value={filter()}
            onInput={(e) => setFilter(e.currentTarget.value)}
            autofocus
          />
        </div>

        <RefGroup
          title="Pinned"
          rows={groups().pins}
          hrefFor={props.hrefFor}
          currentFullRef={currentFullRef()}
        />
        <RefGroup
          title="Branches"
          rows={groups().branches}
          hrefFor={props.hrefFor}
          currentFullRef={currentFullRef()}
        />
        <RefGroup
          title="Tags"
          rows={groups().tags}
          hrefFor={props.hrefFor}
          currentFullRef={currentFullRef()}
        />

        <Show when={!state().loading && groups().branches.length === 0 && groups().tags.length === 0}>
          <p style={{ color: "var(--text-muted)", "font-size": "var(--fs-caption)", margin: "var(--space-1) 0" }}>
            {filter() ? "No refs matched this server search." : "No refs."}
          </p>
        </Show>
        <Show when={state().loading}>
          <p aria-live="polite" style={{ color: "var(--text-muted)", "font-size": "var(--fs-caption)", margin: "var(--space-1) 0" }}>
            Loading refs…
          </p>
        </Show>
        <Show when={state().error}>
          {(message) => (
            <p role="alert" style={{ color: "var(--text-danger)", "font-size": "var(--fs-caption)", margin: "var(--space-1) 0" }}>
              {message()}
            </p>
          )}
        </Show>
        <Show when={state().nextCursor && !state().loading}>
          <button
            type="button"
            onClick={() => void controller.loadMore()}
            style={{
              padding: "var(--space-1) var(--space-2)",
              border: "var(--hairline) solid var(--border)",
              "border-radius": "var(--radius-1)",
              background: "var(--surface-raised)",
              color: "var(--text-primary)",
              cursor: "pointer",
            }}
          >
            Load more
          </button>
        </Show>
        <Show when={state().capped}>
          <p style={{ color: "var(--text-muted)", "font-size": "var(--fs-caption)", margin: "var(--space-1) 0" }}>
            Showing 300 server results. Refine the search to see other refs.
          </p>
        </Show>
      </div>
    </Popover>
  );
}

function RefGroup(props: {
  title: string;
  rows: SwitchRefRow[];
  currentFullRef?: string;
  hrefFor: (ref: string) => string;
}) {
  return (
    <Show when={props.rows.length > 0}>
      <div role="group" aria-label={props.title}>
        <p style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)", margin: "0 0 var(--space-1)", "text-transform": "uppercase", "letter-spacing": "0.04em" }}>
          {props.title}
        </p>
        <ul style={{ "list-style": "none", margin: "0", padding: "0", display: "flex", "flex-direction": "column" }}>
          <For each={props.rows}>
            {(r) => {
              const isDefault = () => r.isDefault;
              const isCurrent = () => props.currentFullRef === r.fullName;
              return (
                <li>
                  <A
                    href={refOptionHref(r, props.hrefFor)}
                    aria-current={isCurrent() ? "true" : undefined}
                    style={{
                      display: "flex",
                      "align-items": "center",
                      gap: "var(--space-2)",
                      padding: "var(--space-1) var(--space-2)",
                      "border-radius": "var(--radius-1)",
                      color: "var(--text-primary)",
                      background: isCurrent() ? "var(--surface-hover)" : "transparent",
                    }}
                  >
                    {/* A check glyph marks the current selection — NEVER colour alone. */}
                    <span aria-hidden="true" style={{ width: "1rem" }}>
                      <Show when={isCurrent()}>
                        <Icon name="approve" size={14} />
                      </Show>
                    </span>
                    <span style={{ "font-family": "var(--font-mono)", flex: "1" }}>{r.name}</span>
                    <Show when={isDefault()}>
                      <span style={{ "font-size": "var(--fs-caption)", color: "var(--text-subtle)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-pill)", padding: "0 var(--space-1)" }}>
                        default
                      </span>
                    </Show>
                  </A>
                </li>
              );
            }}
          </For>
        </ul>
      </div>
    </Show>
  );
}

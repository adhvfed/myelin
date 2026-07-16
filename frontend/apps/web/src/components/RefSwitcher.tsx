// The branch/tag switcher (R3.4 / G-1) — a scoped in-context picker (NOT the ⌘K palette): a Popover
// trigger showing the current ref, unfurling a filter input + two groups (Branches / Tags). Selecting
// a ref rewrites the current route's `[ref]` (the caller supplies `hrefFor`) and preserves the path.
// The filter-input wrapper carries a `:focus-within` focus ring (the gate fix — never a bare
// `outline:none`). Options are real links (keyboard-reachable, axe-clean). Semantic tokens only.
import { For, Show, createSignal, createMemo } from "solid-js";
import { A, createAsync } from "@solidjs/router";
import { Icon, Popover } from "@myelin/design-system";
import { getRefs, type RefRow } from "~/lib/api";

export interface RefSwitcherProps {
  repo: string;
  currentRef: string;
  /** Build the target route for a chosen ref (preserving the current surface + path). */
  hrefFor: (ref: string) => string;
}

export function RefSwitcher(props: RefSwitcherProps) {
  const refs = createAsync(async () => getRefs(props.repo));
  const [filter, setFilter] = createSignal("");

  const match = (rows: RefRow[] | undefined): RefRow[] => {
    const q = filter().toLowerCase();
    return (rows ?? []).filter((r) => !q || r.name.toLowerCase().includes(q));
  };
  const branches = createMemo(() => match(refs()?.branches));
  const tags = createMemo(() => match(refs()?.tags));

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
          <span style={{ "font-family": "var(--font-mono)" }}>{props.currentRef}</span>
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
            aria-label="Filter branches and tags"
            placeholder="Filter…"
            value={filter()}
            onInput={(e) => setFilter(e.currentTarget.value)}
            autofocus
          />
        </div>

        <RefGroup title="Branches" rows={branches()} defaultRef={refs()?.default_branch} hrefFor={props.hrefFor} current={props.currentRef} />
        <RefGroup title="Tags" rows={tags()} hrefFor={props.hrefFor} current={props.currentRef} />

        <Show when={branches().length === 0 && tags().length === 0}>
          <p style={{ color: "var(--text-muted)", "font-size": "var(--fs-caption)", margin: "var(--space-1) 0" }}>
            No matching refs.
          </p>
        </Show>
      </div>
    </Popover>
  );
}

function RefGroup(props: {
  title: string;
  rows: RefRow[];
  defaultRef?: string;
  current: string;
  hrefFor: (ref: string) => string;
}) {
  return (
    <Show when={props.rows.length > 0}>
      <div role="group" aria-label={props.title}>
        <p style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)", margin: "0 0 var(--space-1)", "text-transform": "uppercase", "letter-spacing": "0.04em" }}>
          {props.title} <span aria-hidden="true">·</span> {props.rows.length}
        </p>
        <ul style={{ "list-style": "none", margin: "0", padding: "0", display: "flex", "flex-direction": "column" }}>
          <For each={props.rows}>
            {(r) => {
              const isDefault = () => props.defaultRef === r.name || r.is_default;
              const isCurrent = () => props.current === r.name;
              return (
                <li>
                  <A
                    href={props.hrefFor(r.name)}
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

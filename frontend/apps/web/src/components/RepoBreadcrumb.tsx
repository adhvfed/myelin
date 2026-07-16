// The ref-carrying breadcrumb (R3.4 / findings 13, 6): Repositories / {repo} / {ref-pill} / seg / seg…
// The repo, the ref, and EVERY path segment are individual links (ref → tree root at ref; each segment
// → tree at that sub-path). The last segment is the current node (`aria-current`, not a link). The ref
// is NEVER reset to a hardcoded 'main' — it is the ref the surface was reached with. Mono runs are
// bidi-isolated for RTL. Semantic tokens only.
import { For, Show } from "solid-js";
import { A } from "@solidjs/router";
import { Icon } from "@myelin/design-system";

export interface RepoBreadcrumbProps {
  repo: string;
  refName: string;
  /** The repo-relative path ("" at the tree root / repo home). */
  path?: string;
  /** `blob` marks the last segment as a file (still aria-current, never a link). */
  kind?: "tree" | "blob";
}

const sep = (
  <span aria-hidden="true" style={{ color: "var(--text-subtle)", margin: "0 var(--space-1)" }}>/</span>
);

const mono = { "font-family": "var(--font-mono)", "unicode-bidi": "isolate", direction: "ltr" } as const;

export function RepoBreadcrumb(props: RepoBreadcrumbProps) {
  const segs = () => (props.path ?? "").split("/").filter((s) => s.length > 0);
  const r = () => encodeURIComponent(props.refName);
  const encPath = (parts: string[]) => parts.map(encodeURIComponent).join("/");

  return (
    <nav aria-label="Breadcrumb" style={{ "font-size": "var(--fs-caption)", display: "flex", "align-items": "center", "flex-wrap": "wrap", gap: "var(--space-1)" }}>
      <A href="/git/repos" style={{ color: "var(--text-muted)" }}>Repositories</A>
      {sep}
      <A href={`/git/repos/${props.repo}`} style={{ color: "var(--text-muted)", ...mono }}>{props.repo}</A>
      {sep}
      {/* The ref pill — a link back to the tree root at this ref. */}
      <A
        href={`/git/repos/${props.repo}/tree/${r()}`}
        style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--text-primary)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-pill)", padding: "0 var(--space-2)", ...mono }}
      >
        <Icon name="branch" size={12} /> {props.refName}
      </A>
      <For each={segs()}>
        {(segment, i) => {
          const isLast = () => i() === segs().length - 1;
          const upto = () => segs().slice(0, i() + 1);
          return (
            <>
              {sep}
              <Show
                when={!isLast()}
                fallback={
                  <span aria-current="page" style={{ color: "var(--text-primary)", ...mono }}>{segment}</span>
                }
              >
                <A href={`/git/repos/${props.repo}/tree/${r()}/${encPath(upto())}`} style={{ color: "var(--text-muted)", ...mono }}>{segment}</A>
              </Show>
            </>
          );
        }}
      </For>
    </nav>
  );
}

// PrHeader — the shared PR header + tabs segment across the three PR routes (Overview · Files changed
// · Checks · Commits), per the 03-pr-diff NOTES §1 (a shared layout segment). Active tab = the R1
// `--surface-hover` fill + brighter text. The diff (G-7) and overview (G-6) both render this so the
// tab strip and identity strip never diverge. Status is TEXT (StatusPill); semantic tokens only.
import { Show } from "solid-js";
import { A } from "@solidjs/router";
import { Icon, StatusPill } from "@myelin/design-system";
import { fmtDate } from "~/lib/format";
import type { PrVM } from "~/lib/api";

export type PrTab = "overview" | "diff" | "checks" | "commits";

function tabStyle(active: boolean) {
  return {
    padding: "var(--space-2) var(--space-3)",
    "text-decoration": "none",
    color: active ? "var(--text-primary)" : "var(--text-muted)",
    background: active ? "var(--surface-hover)" : "transparent",
    "border-radius": "var(--radius-1) var(--radius-1) 0 0",
    "font-size": "var(--fs-caption)",
  } as const;
}

export function PrHeader(props: {
  pr: PrVM;
  repo: string;
  active: PrTab;
  commitsCount?: number | null;
  filesCount?: number | null;
}) {
  const title = () => props.pr.title ?? `#${props.pr.number}`;
  const base = () => `/git/repos/${props.repo}/prs/${props.pr.number}`;
  return (
    <header style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
      <div style={{ display: "flex", "align-items": "center", gap: "var(--space-2)", "flex-wrap": "wrap" }}>
        <StatusPill kind="pr-state" state={props.pr.pr_state} />
        <h1 id="pr-heading" style={{ "font-size": "var(--fs-h1)", margin: "0" }}>
          {title()} <span style={{ color: "var(--text-subtle)", "font-weight": "400" }}>#{props.pr.number}</span>
        </h1>
      </div>
      <p style={{ color: "var(--text-muted)", margin: "0", "font-size": "var(--fs-caption)" }}>
        <code style={{ "font-family": "var(--font-mono)" }}>{props.pr.head_ref}</code>
        {" → "}
        <code style={{ "font-family": "var(--font-mono)" }}>{props.pr.base_ref}</code>
        {" · by "}{props.pr.author}
        <Show when={props.pr.author_is_agent}><span> · <Icon name="agent" /> agent</span></Show>
        <Show when={props.pr.created_at}>{" · opened "}{fmtDate(props.pr.created_at as number)}</Show>
      </p>
      <nav aria-label="Pull request sections" style={{ display: "flex", gap: "var(--space-1)", "border-block-end": "var(--hairline) solid var(--border)" }}>
        <A aria-current={props.active === "overview" ? "page" : undefined} href={base()} style={tabStyle(props.active === "overview")}>Overview</A>
        <A aria-current={props.active === "diff" ? "page" : undefined} href={`${base()}/diff`} style={tabStyle(props.active === "diff")}>
          Files changed
          <Show when={props.filesCount != null}>{" "}<span aria-label={`${props.filesCount} files changed`}>({props.filesCount})</span></Show>
        </A>
        <A aria-current={props.active === "checks" ? "page" : undefined} href={`${base()}/checks`} style={tabStyle(props.active === "checks")}>Checks</A>
        <A aria-current={props.active === "commits" ? "page" : undefined} href={`${base()}/commits`} style={tabStyle(props.active === "commits")}>
          Commits
          <Show when={props.commitsCount != null}>{" "}<span aria-label={`${props.commitsCount} commits`}>({props.commitsCount}{props.pr.commits_count_capped ? "+" : ""})</span></Show>
        </A>
      </nav>
    </header>
  );
}

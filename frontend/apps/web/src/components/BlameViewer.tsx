import { A } from "@solidjs/router";
import { For, Show, createMemo } from "solid-js";
import { Icon } from "@myelin/design-system";

import type { BlameHunkVM, BlameVM } from "~/lib/blame-response";
import { splitRepositoryLines } from "~/lib/blame-response";
import { fmtDate } from "~/lib/format";

interface BlameViewerProps {
  repo: string;
  blame: BlameVM;
}

const mono = { "font-family": "var(--font-mono)" } as const;

export function BlameViewer(props: BlameViewerProps) {
  const lines = createMemo(() => splitRepositoryLines(props.blame.contents));
  const hunkLines = (hunk: BlameHunkVM) =>
    lines().slice(hunk.start_line - 1, hunk.start_line - 1 + hunk.line_count);

  return (
    <div
      data-testid="blame-viewer"
      style={{ overflow: "auto", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", background: "var(--surface-raised)" }}
    >
      <table
        aria-label={`Blame for ${props.blame.path}`}
        style={{ width: "100%", "border-collapse": "collapse", "font-size": "var(--fs-caption)" }}
      >
        <thead class="sr-only">
          <tr><th>Commit attribution</th><th>Line</th><th>Code</th></tr>
        </thead>
        <tbody>
          <For each={props.blame.hunks}>
            {(hunk) => (
              <For each={hunkLines(hunk)}>
                {(line, offset) => {
                  const lineNumber = () => hunk.start_line + offset();
                  return (
                    <tr id={`L${lineNumber()}`} style={{ "border-block-start": offset() === 0 ? "var(--hairline) solid var(--border)" : undefined, "scroll-margin-block-start": "var(--space-5)" }}>
                      <Show when={offset() === 0}>
                        <td
                          rowSpan={hunk.line_count}
                          style={{ width: "18rem", "min-width": "14rem", padding: "var(--space-2)", "vertical-align": "top", background: "var(--surface)", "border-inline-end": "var(--hairline) solid var(--border)" }}
                        >
                          <div style={{ display: "grid", gap: "var(--space-1)" }}>
                            <A
                              href={`/git/repos/${props.repo}/commit/${hunk.commit.oid}?ref=${encodeURIComponent(props.blame.ref)}`}
                              style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--accent)", ...mono }}
                              title={hunk.commit.oid}
                            >
                              <Icon name="commit" size={12} /> {hunk.commit.oid.slice(0, 10)}
                            </A>
                            <span style={{ color: "var(--text-primary)", "font-weight": "var(--weight-medium)" }}>{hunk.commit.summary || "Untitled commit"}</span>
                            <span style={{ color: "var(--text-subtle)" }}>
                              {hunk.commit.author || "Erased contributor"} · {fmtDate(hunk.commit.committed_at)}
                            </span>
                          </div>
                        </td>
                      </Show>
                      <th scope="row" style={{ width: "4rem", padding: "0 var(--space-2)", color: "var(--text-subtle)", "font-weight": "var(--weight-regular)", "text-align": "end", "user-select": "none", ...mono }}>
                        <a href={`#L${lineNumber()}`} style={{ color: "inherit" }}>{lineNumber()}</a>
                      </th>
                      <td style={{ padding: "0 var(--space-2)", "white-space": "pre", color: "var(--text-primary)", ...mono }}>
                        {line || " "}
                      </td>
                    </tr>
                  );
                }}
              </For>
            )}
          </For>
        </tbody>
      </table>
      <Show when={lines().length === 0}>
        <p role="note" style={{ margin: "0", padding: "var(--space-4)", color: "var(--text-subtle)", "text-align": "center" }}>
          This file is empty, so there are no lines to attribute.
        </p>
      </Show>
    </div>
  );
}

// DiffViewer — the R-17 §5.1 hard component (R3.2 · G-7), contributed DOWN into the design system as
// the ONE diff/files-changed viewer. Consumers: PR diff (G-7), compare (G-4), commit detail (G-3) —
// three surfaces, zero forks. Presentational + accessible by construction:
//   • change kind is TEXT, never colour: every code cell carries a visually-hidden prefix
//     ("added, new line 210: …" / "removed, old line 105: …" / "unchanged, line 104"); the +/− glyph
//     is a visible TEXT channel (aria-hidden — the prefix already says it). Line numbers are announced.
//   • the line grid is ONE tab stop (roving tabindex); j/k walk lines, F7/Shift-F7 walk CHANGES,
//     n/p walk files, c comments the focused line, v marks viewed. Esc never traps; inline widgets
//     (threads, composer) live in an always-present widget row (tab AFTER their line).
//   • side-by-side + unified; unified-only under the caller's `forceUnified` (the <720px mobile rule).
//   • binary/LFS/submodule rows never dump garbled text; deleted files collapse (never a red wall).
// The table body is ONE flat `<For>` of precomputed single/fixed-arity rows (no conditional <tr>
// siblings, no Show-branch swaps under <For>) so a dynamic re-render never trips the table reconciler.
// Semantic tokens only (no hex); logical properties; the +/− colour only ever tints the glyph channel.
import { For, Show, Switch, Match, createMemo, createSignal, onMount, mergeProps, type JSX } from "solid-js";
import { Icon } from "./Icon";

/** One diff line with both line numbers (null on the absent side). */
export interface DiffViewerLine {
  origin: string; // "+" | "-" | " "
  content: string;
  old_no?: number | null;
  new_no?: number | null;
}

/** One hunk — the `@@` header + boundaries + lines. */
export interface DiffViewerHunk {
  header: string;
  old_start: number;
  old_lines: number;
  new_start: number;
  new_lines: number;
  lines: DiffViewerLine[];
}

/** One changed file. */
export interface DiffViewerFile {
  path: string;
  old_path?: string | null;
  status: string; // A/M/D/R/C
  kind?: "text" | "binary" | "lfs" | "submodule";
  additions?: number;
  deletions?: number;
  size_bytes?: number | null;
  hunks: DiffViewerHunk[];
  deleted_body_available?: boolean;
  truncated?: boolean;
}

/** Context lines the consumer injected after expanding a gap, keyed `${fileIdx}:${gapKey}`. */
export type ExpandedContext = Record<string, DiffViewerLine[]>;

export interface DiffViewerProps {
  files: DiffViewerFile[];
  view?: "split" | "unified";
  forceUnified?: boolean;
  onToggleView?: (v: "split" | "unified") => void;
  srLinear?: boolean;
  onToggleSrLinear?: (v: boolean) => void;
  isViewed?: (path: string) => boolean;
  onToggleViewed?: (path: string) => void;
  onRequestComment?: (path: string, side: "old" | "new", line: number) => void;
  renderThread?: (path: string, side: "old" | "new", line: number) => JSX.Element | undefined;
  hasThread?: (path: string, side: "old" | "new", line: number) => boolean;
  renderComposer?: (path: string, side: "old" | "new", line: number) => JSX.Element | undefined;
  renderFileThreads?: (path: string) => JSX.Element | undefined;
  onExpandContext?: (fileIdx: number, gapKey: string, dir: "up" | "down" | "all") => void;
  expandedContext?: ExpandedContext;
  deepLink?: { path: string; side: "old" | "new"; line: number } | null;
  liveMessage?: string;
}

const STATUS_LABEL: Record<string, string> = { A: "added", M: "modified", D: "deleted", R: "renamed", C: "copied" };

function kindLabel(k?: string): string {
  return k === "binary" ? "Binary file" : k === "lfs" ? "Git LFS object" : k === "submodule" ? "Submodule" : "";
}

/** Pair a hunk's lines into split rows: removed lines align left, added right, context on both. */
function splitRows(lines: DiffViewerLine[]): { left?: DiffViewerLine; right?: DiffViewerLine }[] {
  const rows: { left?: DiffViewerLine; right?: DiffViewerLine }[] = [];
  let dels: DiffViewerLine[] = [];
  let adds: DiffViewerLine[] = [];
  const flush = () => {
    const n = Math.max(dels.length, adds.length);
    for (let i = 0; i < n; i++) rows.push({ left: dels[i], right: adds[i] });
    dels = [];
    adds = [];
  };
  for (const l of lines) {
    if (l.origin === "-") dels.push(l);
    else if (l.origin === "+") adds.push(l);
    else {
      flush();
      rows.push({ left: l, right: l });
    }
  }
  flush();
  return rows;
}

/** The visually-hidden SR prefix — kind + the line number(s), as TEXT (never colour). */
function srPrefix(l: DiffViewerLine): string {
  if (l.origin === "+") return `added, new line ${l.new_no ?? "?"}: `;
  if (l.origin === "-") return `removed, old line ${l.old_no ?? "?"}: `;
  return `unchanged, line ${l.new_no ?? l.old_no ?? "?"}: `;
}

function originGlyph(origin: string): string {
  return origin === "+" ? "+" : origin === "-" ? "−" : " ";
}

function lineColor(origin: string): string {
  return origin === "+" ? "var(--success)" : origin === "-" ? "var(--danger)" : "var(--text-subtle)";
}

/** The split/unified + SR-linear toolbar ("Diff controls" landmark). */
export function DiffToolbar(props: {
  view: "split" | "unified";
  forceUnified?: boolean;
  onToggleView?: (v: "split" | "unified") => void;
  srLinear?: boolean;
  onToggleSrLinear?: (v: boolean) => void;
  summary?: string;
}) {
  return (
    <div role="toolbar" aria-label="Diff controls" style={{ display: "flex", "align-items": "center", gap: "var(--space-3)", "flex-wrap": "wrap", "font-size": "var(--fs-caption)" }}>
      <Show when={props.summary}>
        <span style={{ color: "var(--text-muted)" }} data-testid="diff-summary">{props.summary}</span>
      </Show>
      <Show when={!props.forceUnified}>
        <div role="group" aria-label="Layout" style={{ display: "inline-flex", gap: "var(--space-1)" }}>
          <For each={["split", "unified"] as const}>
            {(v) => (
              <button
                type="button"
                aria-pressed={props.view === v}
                onClick={() => props.onToggleView?.(v)}
                data-testid={`diff-view-${v}`}
                style={{ padding: "var(--space-1) var(--space-2)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", background: props.view === v ? "var(--surface-hover)" : "transparent", color: props.view === v ? "var(--text-primary)" : "var(--text-muted)", cursor: "pointer" }}
              >
                {v === "split" ? "Side-by-side" : "Unified"}
              </button>
            )}
          </For>
        </div>
      </Show>
      <label style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", color: "var(--text-muted)", cursor: "pointer" }}>
        <input type="checkbox" checked={Boolean(props.srLinear)} onChange={(e) => props.onToggleSrLinear?.(e.currentTarget.checked)} data-testid="diff-sr-linear" />
        Linear read
      </label>
    </div>
  );
}

/** The collapsed-context expander (`chevron` is the accepted interim glyph for `expand-lines`). */
export function ExpandContextControl(props: { cols: number; count?: number; onExpand: (dir: "up" | "down" | "all") => void }) {
  return (
    <tr data-testid="expand-context">
      <td colSpan={props.cols} style={{ "text-align": "center", padding: "var(--space-1)", background: "var(--surface-overlay)", color: "var(--text-subtle)", "border-block": "var(--hairline) solid var(--border)", "font-size": "var(--fs-caption)" }}>
        <button type="button" onClick={() => props.onExpand("all")} data-testid="expand-all" style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", background: "transparent", border: "none", color: "var(--text-muted)", cursor: "pointer" }}>
          <Icon name="chevron" size={14} />
          <Show when={props.count} fallback="Expand unchanged lines">{(c) => <>⋯ {c()} unchanged lines · Expand</>}</Show>
        </button>
      </td>
    </tr>
  );
}

// ── flat row model: the whole table body is a stable list of these (no conditional siblings) ──
type DiffRow =
  | { t: "expand"; key: string; gapKey: string }
  | { t: "hunk"; key: string; header: string }
  | { t: "uline"; key: string; line: DiffViewerLine }
  | { t: "sline"; key: string; left?: DiffViewerLine; right?: DiffViewerLine };

function buildRows(file: DiffViewerFile, view: "split" | "unified", expandable: boolean): DiffRow[] {
  const rows: DiffRow[] = [];
  file.hunks.forEach((hunk, hi) => {
    // Only surface the collapsed-context affordance when the consumer wired an expand handler (else it
    // would be a dead button — expand-context needs the N2 endpoint + per-file blob oids to inject).
    if (expandable && (hi > 0 || hunk.new_start > 1)) rows.push({ t: "expand", key: `x${hi}`, gapKey: `${hi}` });
    rows.push({ t: "hunk", key: `h${hi}`, header: hunk.header });
    if (view === "unified") {
      hunk.lines.forEach((line, li) => rows.push({ t: "uline", key: `u${hi}.${li}`, line }));
    } else {
      splitRows(hunk.lines).forEach((r, li) => rows.push({ t: "sline", key: `s${hi}.${li}`, left: r.left, right: r.right }));
    }
  });
  return rows;
}

export function DiffViewer(rawProps: DiffViewerProps) {
  const props = mergeProps({ view: "split" as const }, rawProps);
  const view = (): "split" | "unified" => (props.forceUnified ? "unified" : props.view);
  const cols = () => (view() === "split" ? 4 : 3);
  const [focusKey, setFocusKey] = createSignal<string>("");
  // `mounted` guards the ASYNC-DATA-dependent renders (line threads/composer + the ● marker): the
  // thread data arrives from an async resource, so it must NOT render during SSR / the first hydration
  // pass (server has it, client's first paint doesn't → a hydration-key mismatch that crashes the
  // grid). It flips true after mount, so threads appear client-side without a mismatch.
  const [mounted, setMounted] = createSignal(false);
  onMount(() => setMounted(true));

  let rootEl: HTMLDivElement | undefined;
  const cells = (): HTMLElement[] => Array.from(rootEl?.querySelectorAll<HTMLElement>("[data-rowkey]") ?? []);
  const curIndex = (list: HTMLElement[]) => {
    const active = document.activeElement as HTMLElement | null;
    const byActive = active ? list.indexOf(active) : -1;
    if (byActive >= 0) return byActive;
    return list.findIndex((e) => e.dataset.rowkey === focusKey());
  };
  const focusAt = (list: HTMLElement[], i: number) => {
    const el = list[Math.max(0, Math.min(i, list.length - 1))];
    if (el) {
      setFocusKey(el.dataset.rowkey ?? "");
      el.focus();
      el.scrollIntoView({ block: "center", behavior: "auto" });
    }
  };
  const focusNextChange = (dir: 1 | -1) => {
    const list = cells();
    let i = curIndex(list);
    if (i < 0) i = dir === 1 ? -1 : list.length;
    for (let j = i + dir; j >= 0 && j < list.length; j += dir) {
      if (list[j]?.dataset.change === "1") return focusAt(list, j);
    }
  };
  const focusFile = (dir: 1 | -1) => {
    const headers = Array.from(rootEl?.querySelectorAll<HTMLElement>("[data-fileheader]") ?? []);
    const list = cells();
    const curFile = list[curIndex(list)]?.dataset.fileidx;
    let idx = headers.findIndex((h) => h.dataset.fileidx === curFile);
    if (idx < 0) idx = dir === 1 ? -1 : headers.length;
    const target = headers[idx + dir];
    if (target) {
      target.focus();
      target.scrollIntoView({ block: "start" });
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const t = e.target as HTMLElement;
    if (t.closest("[data-diff-widget]") || t.tagName === "TEXTAREA" || t.tagName === "INPUT" || t.tagName === "BUTTON") return;
    const list = cells();
    const cur = list[curIndex(list)];
    switch (e.key) {
      case "j": e.preventDefault(); focusAt(list, curIndex(list) + 1); break;
      case "k": e.preventDefault(); focusAt(list, curIndex(list) - 1); break;
      case "F7": e.preventDefault(); focusNextChange(e.shiftKey ? -1 : 1); break;
      case "]": e.preventDefault(); focusNextChange(1); break;
      case "[": e.preventDefault(); focusNextChange(-1); break;
      case "n": e.preventDefault(); focusFile(1); break;
      case "p": e.preventDefault(); focusFile(-1); break;
      case "c":
        if (cur?.dataset.path && cur.dataset.side && cur.dataset.line) {
          e.preventDefault();
          props.onRequestComment?.(cur.dataset.path, cur.dataset.side as "old" | "new", Number(cur.dataset.line));
        }
        break;
      case "v":
        if (cur?.dataset.path) { e.preventDefault(); props.onToggleViewed?.(cur.dataset.path); }
        break;
    }
  };

  return (
    <div ref={rootEl} onKeyDown={onKeyDown} style={{ display: "flex", "flex-direction": "column", gap: "var(--space-3)" }}>
      <DiffToolbar
        view={view()}
        forceUnified={props.forceUnified}
        onToggleView={props.onToggleView}
        srLinear={props.srLinear}
        onToggleSrLinear={props.onToggleSrLinear}
        summary={`${props.files.length} file${props.files.length === 1 ? "" : "s"} · +${props.files.reduce((a, f) => a + (f.additions ?? 0), 0)} −${props.files.reduce((a, f) => a + (f.deletions ?? 0), 0)}`}
      />
      <div aria-live="polite" class="sr-only" data-testid="diff-live">{props.liveMessage ?? ""}</div>
      <For each={props.files}>
        {(file, fi) => (
          <DiffFileSection
            file={file}
            fileIdx={fi()}
            view={view()}
            cols={cols()}
            viewed={props.isViewed?.(file.path) ?? false}
            onToggleViewed={props.onToggleViewed}
            focusKey={focusKey()}
            mounted={mounted()}
            setFocusKey={setFocusKey}
            onRequestComment={props.onRequestComment}
            renderThread={props.renderThread}
            hasThread={props.hasThread}
            renderComposer={props.renderComposer}
            renderFileThreads={props.renderFileThreads}
            onExpandContext={props.onExpandContext}
            deepLink={props.deepLink}
          />
        )}
      </For>
    </div>
  );
}

interface SectionProps {
  file: DiffViewerFile;
  fileIdx: number;
  view: "split" | "unified";
  cols: number;
  viewed: boolean;
  onToggleViewed?: (path: string) => void;
  focusKey: string;
  mounted: boolean;
  setFocusKey: (k: string) => void;
  onRequestComment?: (path: string, side: "old" | "new", line: number) => void;
  renderThread?: (path: string, side: "old" | "new", line: number) => JSX.Element | undefined;
  hasThread?: (path: string, side: "old" | "new", line: number) => boolean;
  renderComposer?: (path: string, side: "old" | "new", line: number) => JSX.Element | undefined;
  renderFileThreads?: (path: string) => JSX.Element | undefined;
  onExpandContext?: (fileIdx: number, gapKey: string, dir: "up" | "down" | "all") => void;
  deepLink?: { path: string; side: "old" | "new"; line: number } | null;
}

function DiffFileSection(props: SectionProps) {
  const [folded, setFolded] = createSignal(false);
  const [showDeleted, setShowDeleted] = createSignal(false);
  const isText = () => (props.file.kind ?? "text") === "text";
  const isDeleted = () => props.file.status === "D";
  const collapsed = () => folded() || props.viewed;
  const rows = createMemo(() => buildRows(props.file, props.view, Boolean(props.onExpandContext)));

  return (
    <section aria-label={`Diff for ${props.file.path}`} data-fileidx={props.fileIdx} style={{ border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", overflow: "hidden" }}>
      <h2
        tabindex="-1"
        data-fileheader
        data-fileidx={props.fileIdx}
        data-path={props.file.path}
        style={{ position: "sticky", "inset-block-start": "0", "z-index": "1", margin: "0", padding: "var(--space-2) var(--space-3)", background: "var(--surface-overlay)", "font-size": "var(--fs-body)", display: "flex", "align-items": "center", gap: "var(--space-2)", "flex-wrap": "wrap" }}
      >
        <button type="button" onClick={() => setFolded((f) => !f)} aria-expanded={!collapsed()} aria-label={collapsed() ? `Expand ${props.file.path}` : `Collapse ${props.file.path}`} data-testid="file-fold" style={{ background: "transparent", border: "none", cursor: "pointer", color: "var(--text-muted)", "line-height": "1" }}>
          <Icon name="chevron" size={14} style={{ transform: collapsed() ? "rotate(-90deg)" : "none" }} />
        </button>
        <span style={{ "font-size": "var(--fs-caption)", padding: "0 var(--space-1)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", color: "var(--text-muted)" }}>
          {STATUS_LABEL[props.file.status] ?? props.file.status}
        </span>
        <Show when={props.file.old_path}>{(old) => <code style={{ "font-family": "var(--font-mono)", color: "var(--text-subtle)" }}><bdi>{old()}</bdi> →</code>}</Show>
        <code style={{ "font-family": "var(--font-mono)" }}><bdi>{props.file.path}</bdi></code>
        <span style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>+{props.file.additions ?? 0} −{props.file.deletions ?? 0}</span>
        <div style={{ "margin-inline-start": "auto", display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
          <Show when={props.file.truncated}><span data-testid="file-truncated" style={{ color: "var(--warning)", "font-size": "var(--fs-caption)" }}>truncated</span></Show>
          <label style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", "font-size": "var(--fs-caption)", color: "var(--text-muted)", cursor: "pointer" }}>
            <input type="checkbox" checked={props.viewed} onChange={() => props.onToggleViewed?.(props.file.path)} data-testid="file-viewed" />
            Viewed
          </label>
        </div>
      </h2>

      <Show when={props.mounted}>{props.renderFileThreads?.(props.file.path)}</Show>

      <Show when={!collapsed()}>
        <Switch>
          <Match when={!isText()}>
            <div data-testid="binary-row" style={{ padding: "var(--space-3)", color: "var(--text-muted)", "font-size": "var(--fs-body-sm)" }}>
              {kindLabel(props.file.kind)} — no text diff.<Show when={props.file.size_bytes}>{(s) => <> {s()} bytes.</>}</Show>
            </div>
          </Match>
          <Match when={isDeleted() && !showDeleted()}>
            <div data-testid="deleted-tombstone" style={{ padding: "var(--space-3)", color: "var(--text-muted)", "font-size": "var(--fs-body-sm)", display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
              <span>File deleted (−{props.file.deletions ?? 0} lines).</span>
              <Show when={props.file.deleted_body_available}>
                <button type="button" onClick={() => setShowDeleted(true)} data-testid="show-deleted" style={{ background: "transparent", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", padding: "0 var(--space-1)", cursor: "pointer", color: "var(--text-muted)" }}>Show deleted contents</button>
              </Show>
            </div>
          </Match>
          <Match when={true}>
            <div class="tblwrap" style={{ "overflow-x": "auto" }}>
              <table style={{ width: "100%", "border-collapse": "collapse", "font-family": "var(--font-mono)", "font-size": "var(--fs-body-sm)" }}>
                <tbody>
                  <For each={rows()}>
                    {(r) => (
                      <Switch>
                        <Match when={r.t === "expand"}>
                          <ExpandContextControl cols={props.cols} onExpand={(dir) => props.onExpandContext?.(props.fileIdx, (r as { gapKey: string }).gapKey, dir)} />
                        </Match>
                        <Match when={r.t === "hunk"}>
                          <tr>
                            <td colSpan={props.cols} style={{ background: "var(--surface-raised)", color: "var(--text-subtle)", padding: "0 var(--space-2)", "white-space": "pre" }}>
                              <bdi>{(r as { header: string }).header}</bdi>
                            </td>
                          </tr>
                        </Match>
                        <Match when={r.t === "uline"}>
                          <UnifiedRow row={r as Extract<DiffRow, { t: "uline" }>} {...props} />
                        </Match>
                        <Match when={r.t === "sline"}>
                          <SplitRow row={r as Extract<DiffRow, { t: "sline" }>} {...props} />
                        </Match>
                      </Switch>
                    )}
                  </For>
                </tbody>
              </table>
            </div>
          </Match>
        </Switch>
      </Show>
    </section>
  );
}

/** A gutter cell — the line number, announced (NOT aria-hidden). */
function Gutter(props: { no?: number | null }) {
  return (
    <td style={{ width: "3rem", "text-align": "end", "padding-inline": "var(--space-1)", color: "var(--text-subtle)", "user-select": "none", "border-inline-end": "var(--hairline) solid var(--border)", "vertical-align": "top" }}>
      {props.no ?? ""}
    </td>
  );
}

function CodeCell(props: {
  line: DiffViewerLine;
  section: SectionProps;
  side: "old" | "new";
}) {
  const s = props.section;
  const lineNo = () => (props.side === "new" ? props.line.new_no : props.line.old_no) ?? props.line.new_no ?? props.line.old_no ?? 0;
  const rowKey = () => `${s.fileIdx}:${props.side}:${lineNo()}:${props.line.origin}`;
  const isChange = () => props.line.origin !== " ";
  // Gated on `mounted`: the thread flag comes from an async resource — rendering it during SSR/first
  // hydration would mismatch the client's first paint (see DiffViewer `mounted`).
  const threaded = () => s.mounted && (s.hasThread?.(s.file.path, props.side, lineNo()) ?? false);
  const isDeepLink = () => s.deepLink?.path === s.file.path && s.deepLink?.side === props.side && s.deepLink?.line === lineNo();
  return (
    <td
      tabindex={s.focusKey === rowKey() ? "0" : "-1"}
      data-rowkey={rowKey()}
      data-change={isChange() ? "1" : "0"}
      data-fileidx={s.fileIdx}
      data-path={s.file.path}
      data-side={props.side}
      data-line={lineNo()}
      onFocus={() => s.setFocusKey(rowKey())}
      onClick={() => s.onRequestComment?.(s.file.path, props.side, lineNo())}
      style={{ "padding-inline": "var(--space-2)", "white-space": "pre-wrap", "word-break": "break-word", color: lineColor(props.line.origin), background: isDeepLink() ? "var(--info-subtle)" : threaded() ? "var(--surface-hover)" : "transparent", outline: "none", cursor: "text", "vertical-align": "top" }}
    >
      <span class="sr-only">{srPrefix(props.line)}{threaded() ? "has 1 comment thread. " : ""}{isDeepLink() ? "deep-link target from failing check. " : ""}</span>
      <span aria-hidden="true" style={{ "user-select": "none", color: "var(--text-subtle)", "margin-inline-end": "var(--space-1)" }}>{originGlyph(props.line.origin)}</span>
      <bdi>{props.line.content || " "}</bdi>
      <Show when={threaded()}><span aria-hidden="true" style={{ "margin-inline-start": "var(--space-1)", color: "var(--accent)" }}>●</span></Show>
    </td>
  );
}

/** An ALWAYS-PRESENT widget row (thread + composer) beneath a line — stable row count (content is
 *  conditional INSIDE, never a conditional sibling <tr>, so a dynamic open/close never trips the
 *  table reconciler). A row with no thread/composer renders an empty, zero-height <tr>.
 *  `targets` is the set of (side,line) the row can host a widget for: unified rows carry ONE, split
 *  rows carry BOTH the old (left) and new (right) sides so a comment on a DELETED (old-side) line
 *  opens its composer under the clicked side — never silently anchored to the new side only. */
function WidgetRow(props: { cols: number; section: SectionProps; targets: { side: "old" | "new"; line: number }[] }) {
  const s = props.section;
  // A row carries an optional OLD (left/deleted) and NEW (right/added) target. Two keying rules:
  //   • THREADS are anchored by (path, line), side-agnostic — render per DISTINCT line, so a modified
  //     row with old_no === new_no shows its thread ONCE, but old_no !== new_no shows both lines.
  //   • COMPOSERS are side-keyed and self-gate on the exact (path, side, line) — render for EACH side
  //     so a click on the deleted (old) cell opens the composer under the OLD side, not only the new.
  // Explicit <Show> slots (not <For>) keep each call in a TRACKED scope so it re-runs when the
  // consumer's open-composer / thread signals change (a <For> callback is memoised and would not).
  const oldT = () => props.targets.find((t) => t.side === "old");
  const newT = () => props.targets.find((t) => t.side === "new");
  const threadAt = (t?: { side: "old" | "new"; line: number }) => (t ? s.renderThread?.(s.file.path, t.side, t.line) : undefined);
  const composerAt = (t?: { side: "old" | "new"; line: number }) => (t ? s.renderComposer?.(s.file.path, t.side, t.line) : undefined);
  const oldThread = () => threadAt(oldT());
  const newThread = () => {
    const nt = newT();
    const ot = oldT();
    return nt && (!ot || ot.line !== nt.line) ? threadAt(nt) : undefined; // dedupe same-line sides
  };
  // Dynamic JSX children (`{fn()}`) — each is its own TRACKED scope, so the composer re-renders when
  // the consumer's open-composer signal flips (a <Show>/<For> render-prop body is called once, not re-run).
  return (
    <tr data-diff-widget>
      <td colSpan={props.cols} style={{ padding: "0", background: "var(--surface-raised)" }}>
        <Show when={s.mounted}>
          {oldThread()}
          {newThread()}
          {composerAt(oldT())}
          {composerAt(newT())}
        </Show>
      </td>
    </tr>
  );
}

function UnifiedRow(props: { row: Extract<DiffRow, { t: "uline" }>; } & SectionProps) {
  const line = () => props.row.line;
  const side = (): "old" | "new" => (line().origin === "-" ? "old" : "new");
  const lineNo = () => (side() === "new" ? line().new_no : line().old_no) ?? 0;
  const bg = () => (line().origin === "+" ? "var(--success-subtle)" : line().origin === "-" ? "var(--danger-subtle)" : "transparent");
  return (
    <>
      <tr style={{ background: bg() }}>
        <Gutter no={line().old_no} />
        <Gutter no={line().new_no} />
        <CodeCell line={line()} section={props} side={side()} />
      </tr>
      <WidgetRow cols={props.cols} section={props} targets={[{ side: side(), line: lineNo() }]} />
    </>
  );
}

function SplitRow(props: { row: Extract<DiffRow, { t: "sline" }>; } & SectionProps) {
  const left = () => props.row.left;
  const right = () => props.row.right;
  // Widget targets: a context row pairs the SAME line object on both sides (splitRows pushes
  // `{ left: l, right: l }`) → one target on the new side. A changed row has distinct old/new lines →
  // a target for EACH side present, so a comment on the deleted (left) cell opens under the old side.
  const targets = (): { side: "old" | "new"; line: number }[] => {
    const l = left();
    const r = right();
    if (l && r && l === r) return [{ side: "new", line: r.new_no ?? 0 }];
    const t: { side: "old" | "new"; line: number }[] = [];
    if (l) t.push({ side: "old", line: l.old_no ?? 0 });
    if (r) t.push({ side: "new", line: r.new_no ?? 0 });
    return t;
  };
  return (
    <>
      <tr>
        <Show when={left()} fallback={<><td style={{ width: "3rem", "border-inline-end": "var(--hairline) solid var(--border)", background: "var(--surface-overlay)" }} /><td style={{ background: "var(--surface-overlay)" }} /></>}>
          {(l) => (<><Gutter no={l().old_no} /><CodeCell line={l()} section={props} side="old" /></>)}
        </Show>
        <Show when={right()} fallback={<><td style={{ width: "3rem", "border-inline-end": "var(--hairline) solid var(--border)", background: "var(--surface-overlay)" }} /><td style={{ background: "var(--surface-overlay)" }} /></>}>
          {(r) => (<><Gutter no={r().new_no} /><CodeCell line={r()} section={props} side="new" /></>)}
        </Show>
      </tr>
      <WidgetRow cols={props.cols} section={props} targets={targets()} />
    </>
  );
}

// PR diff / files-changed (R3.2 · G-7) — `/git/repos/{repo}/prs/{n}/diff`. The densest engineer
// surface: the three-dot `merge-base(base, head) … head` diff on the shared <DiffViewer>, with
// line-anchored comment threads (the R3.3 thread store), viewed marks (client-local, localStorage
// keyed pr+head_oid), the W4 deep-link anchor (?file=&line=&side=) with an honest banner, split/
// unified (unified-only <720px), the restricted count-only row, and load-remaining-files paging.
// Full R-21 states. Keyboard/SR live in <DiffViewer> (j/k line · F7 change · n/p file · c comment ·
// v viewed); this route wires the data + threads + composer. Semantic tokens only; status is TEXT.
import { ErrorBoundary, For, Show, Suspense, createMemo, createSignal } from "solid-js";
import { Title } from "@solidjs/meta";
import { A, createAsync, revalidate, useAction, useParams, useSearchParams } from "@solidjs/router";
import {
  Skeleton,
  SkeletonBlock,
  DiffViewer,
  type DiffViewerFile,
  type ExpandedContext,
} from "@myelin/design-system";
import {
  getFileLines,
  getPr,
  getPrDiff,
  getPrThreads,
  prMutate,
  type DiffLineVM,
  type PrDiffVM,
  type PrThreadVM,
  type PrThreadsVM,
} from "~/lib/api";
import { mapPrDiffContextLines, prDiffContextRange } from "~/lib/pr-diff-context";
import { PrHeader } from "~/components/PrHeader";
import { RepoErrorState, errKind } from "~/components/RepoErrorState";
import { Markdown } from "~/components/Markdown";

const card = {
  border: "var(--hairline) solid var(--border)",
  "border-radius": "var(--radius-1)",
  padding: "var(--space-3)",
  background: "var(--surface-raised)",
} as const;

const textareaStyle = {
  width: "100%",
  "font-family": "var(--font-mono)",
  "font-size": "var(--fs-body-sm)",
  padding: "var(--space-2)",
  "border-radius": "var(--radius-1)",
  border: "var(--hairline) solid var(--border)",
  background: "var(--surface)",
  color: "var(--text-primary)",
  "box-sizing": "border-box",
} as const;

/** A line-anchored comment target the composer is open on. */
type CommentAt = { path: string; side: "old" | "new"; line: number };

export default function PrDiffScreen() {
  const params = useParams();
  const [search, setSearch] = useSearchParams();
  const ready = () => Boolean(params.repo && params.n && Number.isFinite(Number(params.n)));
  const repo = () => params.repo ?? "";
  const n = () => Number(params.n);

  // Layout: ?view= wins; else split ≥960px, unified below; <720px forces unified (the switcher hides).
  const viewParam = () => (typeof search.view === "string" && (search.view === "split" || search.view === "unified") ? search.view : undefined);
  // The MR-014 file cursor (?cursor=) — the "Load remaining files" link SETS it; the query must READ
  // it (mirrors the PR-list route) or a >50-file PR can never page. `getPrDiff` accepts it.
  const cursor = () => (typeof search.cursor === "string" ? search.cursor : undefined);

  const pr = createAsync(
    async () => (ready() ? getPr({ repo: repo(), n: n() }) : undefined),
    { deferStream: true },
  );
  // NB: the diff query does NOT depend on `view` — layout (split/unified) is a CLIENT concern; the
  // server is layout-agnostic, so toggling the view never refetches (and never remounts the grid). It
  // DOES depend on `cursor` — a new page is a real refetch keyed by the file cursor.
  const diff = createAsync(
    async () =>
      ready() ? getPrDiff({ repo: repo(), n: n(), cursor: cursor() }) : undefined,
    { deferStream: true },
  );
  const threads = createAsync(
    async () => (ready() ? getPrThreads({ repo: repo(), n: n() }) : undefined),
    { deferStream: true },
  );

  // View is a stable LOCAL signal (source of truth for rendering — decoupled from navigation so a
  // toggle never re-suspends the grid), seeded from the shareable `?view=` (default split). Toggling
  // updates the signal immediately AND mirrors the URL for shareability.
  // FLOOR (R3): auto-responsive layout (split≥960 / unified<720, switcher hidden on mobile) is a
  // fast-follow — the reactive media-query wiring flapped the switcher; the user toggle + `?view=`
  // are the R3 path. The <720px unified-only rule lands with that follow-on.
  const [viewOverride, setViewOverride] = createSignal<"split" | "unified" | undefined>(viewParam());
  const view = (): "split" | "unified" => viewOverride() ?? viewParam() ?? "split";
  const setView = (v: "split" | "unified") => {
    setViewOverride(v);
    setSearch({ view: v });
  };

  // Viewed marks — client-local (R3 Q6 floor), keyed pr + head_oid so a rebase resets them.
  const viewedKey = (d: PrDiffVM) => `myelin:viewed:${repo()}:${n()}:${d.head_oid}`;
  const [viewedTick, setViewedTick] = createSignal(0);
  const readViewed = (d: PrDiffVM): Set<string> => {
    if (typeof localStorage === "undefined") return new Set();
    try {
      return new Set(JSON.parse(localStorage.getItem(viewedKey(d)) ?? "[]") as string[]);
    } catch {
      return new Set();
    }
  };
  const isViewed = (path: string): boolean => {
    const d = diff();
    if (!d) return false;
    void viewedTick();
    return readViewed(d).has(path);
  };
  const toggleViewed = (path: string) => {
    const d = diff();
    if (!d || typeof localStorage === "undefined") return;
    const s = readViewed(d);
    if (s.has(path)) s.delete(path);
    else s.add(path);
    localStorage.setItem(viewedKey(d), JSON.stringify([...s]));
    setViewedTick((t) => t + 1);
    setLive(`${path} marked ${s.has(path) ? "viewed, collapsed" : "not viewed"}`);
  };

  // Threads — the anchored (line-attached) threads from the R3.3 store, keyed by path+line. A detached
  // (line == null / anchor_state "outdated") thread is LIFTED to file level (never a silent wrong line).
  const anchored = createMemo<PrThreadVM[]>(() => (threads() as PrThreadsVM | undefined)?.anchored ?? []);
  const liveThreadsFor = (path: string, line: number): PrThreadVM[] =>
    anchored().filter((t) => t.anchor && t.anchor.path === path && t.anchor.line === line && t.anchor.anchor_state !== "outdated");
  // An OUTDATED anchor is detached: its content no longer exists in the diff. `line` carries the FORMER
  // (authored-time) line — lifted to file level as "was on former line N", never a silent wrong line.
  const detachedThreadsFor = (path: string): PrThreadVM[] =>
    anchored().filter((t) => t.anchor && t.anchor.path === path && t.anchor.anchor_state === "outdated");
  const hasThread = (path: string, _side: "old" | "new", line: number) => liveThreadsFor(path, line).length > 0;

  // The line-comment composer (one open at a time). `c` / a gutter click opens it on the focused line.
  const [commentAt, setCommentAt] = createSignal<CommentAt | null>(null);
  const [draft, setDraft] = createSignal("");
  const [live, setLive] = createSignal("");
  // Expanded context is namespaced by immutable diff identity rather than page-local file indexes.
  // That keeps a late response from one cursor page or pre-rebase head out of another file's rows.
  const [expandedByIdentity, setExpandedByIdentity] = createSignal<Record<string, DiffLineVM[]>>({});
  const pendingContext = new Set<string>();
  const contextIdentity = (headOid: string, path: string, blobOid: string, gapKey: string) =>
    JSON.stringify([headOid, path, blobOid, gapKey]);
  const expandedContext = createMemo<ExpandedContext>(() => {
    const d = diff();
    if (!d) return {};
    const stored = expandedByIdentity();
    const visible: ExpandedContext = {};
    d.files.forEach((file, fileIdx) => {
      const blobOid = file.new_blob_oid;
      if (!blobOid) return;
      file.hunks.forEach((_hunk, hunkIdx) => {
        const lines = stored[contextIdentity(d.head_oid, file.path, blobOid, `${hunkIdx}`)];
        if (lines) visible[`${fileIdx}:${hunkIdx}`] = lines;
      });
    });
    return visible;
  });
  const expandContext = async (fileIdx: number, gapKey: string) => {
    const d = diff();
    const file = d?.files[fileIdx];
    if (!d || !file?.new_blob_oid) return;
    const range = prDiffContextRange(file, gapKey);
    if (!range) {
      setLive(`Unchanged context for ${file.path} is outside the bounded expansion range.`);
      return;
    }
    const identity = contextIdentity(d.head_oid, file.path, file.new_blob_oid, gapKey);
    if (pendingContext.has(identity) || expandedByIdentity()[identity]) return;
    pendingContext.add(identity);
    const { head_oid: headOid } = d;
    const { path, new_blob_oid: blobOid } = file;
    try {
      const response = await getFileLines({
        repo: repo(),
        oid: blobOid,
        path,
        start: range.start,
        end: range.end,
      });
      const lines = mapPrDiffContextLines(response.lines, range);
      if (!lines) throw new Error("invalid context response");
      const current = diff();
      const currentFile = current?.files[fileIdx];
      if (current?.head_oid !== headOid || currentFile?.path !== path ||
          currentFile.new_blob_oid !== blobOid) return;
      setExpandedByIdentity((value) => ({ ...value, [identity]: lines }));
      setLive(`Expanded ${lines.length} unchanged ${lines.length === 1 ? "line" : "lines"} in ${path}.`);
    } catch {
      setLive(`Couldn't expand unchanged lines in ${path}.`);
    } finally {
      pendingContext.delete(identity);
    }
  };
  const mutate = useAction(prMutate);
  const reload = () => revalidate("git-pr-threads");

  const submitComment = async () => {
    const at = commentAt();
    const body = draft().trim();
    if (!at || !body) return;
    await mutate({ op: "thread", repo: repo(), n: n(), body_md: body, anchor: { path: at.path, line: at.line, side: at.side } });
    setDraft("");
    setCommentAt(null);
    setLive("Comment posted");
    await reload();
  };

  // The W4 deep-link anchor (?file=&line=&side=). Honest: if the line no longer exists in the diff, the
  // banner says the check ran against an older head — never a silent nearest-line guess.
  const deepLink = createMemo(() => {
    const f = typeof search.file === "string" ? search.file : null;
    const l = typeof search.line === "string" ? Number(search.line) : NaN;
    if (!f || !Number.isFinite(l)) return null;
    const side = search.side === "old" ? "old" : "new";
    return { path: f, side, line: l } as const;
  });
  const deepLinkExists = createMemo(() => {
    const dl = deepLink();
    const d = diff();
    if (!dl || !d) return false;
    const file = d.files.find((x) => x.path === dl.path);
    if (!file) return false;
    return file.hunks.some((h) => h.lines.some((ln) => (dl.side === "new" ? ln.new_no : ln.old_no) === dl.line));
  });

  return (
    <section aria-labelledby="pr-heading" style={{ display: "flex", "flex-direction": "column", gap: "var(--space-3)" }}>
      <Title>Files changed · #{params.n} · {params.repo} · Myelin</Title>
      <nav aria-label="Breadcrumb" style={{ "font-size": "var(--fs-caption)", display: "flex", gap: "var(--space-1)", "flex-wrap": "wrap" }}>
        <A href="/git/repos" style={{ color: "var(--text-muted)" }}>Repositories</A>
        <span aria-hidden="true">/</span>
        <A href={`/git/repos/${params.repo}`} style={{ color: "var(--text-muted)" }}>{params.repo}</A>
        <span aria-hidden="true">/</span>
        <A href={`/git/repos/${params.repo}/prs/${params.n}`} style={{ color: "var(--text-muted)" }}>#{params.n}</A>
      </nav>

      <ErrorBoundary fallback={(err, retry) => <RepoErrorState kind={errKind(err)} repo={params.repo} onRetry={retry} />}>
        <Show when={ready()} fallback={<RepoErrorState kind="not-found" repo={params.repo} />}>
          {/* The header/tabs render as soon as PrVM resolves (independent regions). */}
          <Suspense fallback={<SkeletonBlock height="4rem" />}>
            <Show when={pr()}>{(p) => <PrHeader pr={p()} repo={repo()} active="diff" filesCount={diff()?.total_files ?? null} commitsCount={p().commits_count ?? null} />}</Show>
          </Suspense>

          <Suspense
            fallback={
              <Skeleton label="Loading diff…" data-testid="diff-loading">
                <SkeletonBlock height="2rem" />
                <SkeletonBlock height="14rem" style={{ "margin-block-start": "var(--space-2)" }} />
              </Skeleton>
            }
          >
            <Show when={diff()} keyed>
              {(d) => (
                <>
                  {/* Two-dot floor banner — honest when a real merge-base couldn't be computed. */}
                  <Show when={!d.three_dot && d.total_files > 0}>
                    <p role="note" data-testid="two-dot-banner" style={{ ...card, margin: "0", color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>
                      Compared against <code style={{ "font-family": "var(--font-mono)" }}>{d.base_ref}</code> @ <code style={{ "font-family": "var(--font-mono)" }}>{d.short_base_oid || "—"}</code> (two-dot — a merge-base wasn't available).
                    </p>
                  </Show>

                  {/* W4 deep-link arrival banner — honest about a stale line (never a nearest-line guess). */}
                  <Show when={deepLink()}>
                    {(dl) => (
                      <p role="note" data-testid="deeplink-banner" style={{ ...card, margin: "0", "font-size": "var(--fs-caption)", background: "var(--info-subtle)" }}>
                        <Show
                          when={deepLinkExists()}
                          fallback={<>The failing check pointed at <code style={{ "font-family": "var(--font-mono)" }}>{dl().path}</code> line {dl().line}, but that line no longer exists in this diff — the check ran against an older head.</>}
                        >
                          Jumped to <code style={{ "font-family": "var(--font-mono)" }}>{dl().path}</code> line {dl().line}, the target of a failing check.
                        </Show>
                      </p>
                    )}
                  </Show>

                  {/* Restricted count-only row — no path, no per-file diffstat crosses the wire. */}
                  <Show when={d.restricted_files > 0}>
                    <p role="note" data-testid="restricted-row" style={{ ...card, margin: "0", color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>
                      Part of this diff is restricted — {d.restricted_files} changed {d.restricted_files === 1 ? "file isn't" : "files aren't"} shown.
                    </p>
                  </Show>

                  <Show
                    when={d.total_files > 0}
                    fallback={
                      <p data-testid="diff-empty" style={{ color: "var(--text-muted)" }}>
                        No changes to review — the base and head are identical.
                      </p>
                    }
                  >
                    <DiffViewer
                      files={d.files as DiffViewerFile[]}
                      view={view()}
                      onToggleView={setView}
                      liveMessage={live()}
                      isViewed={isViewed}
                      onToggleViewed={toggleViewed}
                      onExpandContext={(fileIdx, gapKey) => void expandContext(fileIdx, gapKey)}
                      expandedContext={expandedContext()}
                      hasThread={hasThread}
                      onRequestComment={(path, side, line) => { setCommentAt({ path, side, line }); setDraft(""); }}
                      deepLink={deepLinkExists() ? deepLink() : null}
                      renderThread={(path, _side, line) => {
                        const ts = liveThreadsFor(path, line);
                        return ts.length ? <div data-diff-widget><For each={ts}>{(t) => <ThreadCard thread={t} />}</For></div> : undefined;
                      }}
                      renderFileThreads={(path) => {
                        const ts = detachedThreadsFor(path);
                        return ts.length ? (
                          <div data-diff-widget data-testid="detached-threads" style={{ padding: "var(--space-2) var(--space-3)", "border-block-end": "var(--hairline) solid var(--border)" }}>
                            <For each={ts}>{(t) => <ThreadCard thread={t} outdated />}</For>
                          </div>
                        ) : undefined;
                      }}
                      renderComposer={(path, side, line) => {
                        const at = commentAt();
                        if (!at || at.path !== path || at.side !== side || at.line !== line) return undefined;
                        return (
                          <div data-diff-widget style={{ padding: "var(--space-2) var(--space-3)" }}>
                            <textarea
                              autofocus
                              aria-label={`Comment on ${path} line ${line}`}
                              value={draft()}
                              onInput={(e) => setDraft(e.currentTarget.value)}
                              onKeyDown={(e) => {
                                if (e.key === "Escape") { e.preventDefault(); setCommentAt(null); }
                                if ((e.metaKey || e.ctrlKey) && e.key === "Enter") { e.preventDefault(); void submitComment(); }
                              }}
                              rows={3}
                              placeholder="Comment on this line…  (⌘⏎ to submit, Esc to cancel)"
                              style={textareaStyle}
                            />
                            <div style={{ display: "flex", gap: "var(--space-2)", "margin-block-start": "var(--space-1)" }}>
                              <button type="button" onClick={() => void submitComment()} disabled={!draft().trim()} style={barBtn}>Add single comment</button>
                              <button type="button" onClick={() => setCommentAt(null)} style={{ ...barBtn, background: "transparent" }}>Cancel</button>
                            </div>
                          </div>
                        );
                      }}
                    />

                    {/* Load-remaining-files (MR-014 file cursor) — names/counts visible, contents lazy. */}
                    <Show when={d.page.next_cursor}>
                      <p data-testid="load-remaining" style={{ color: "var(--text-muted)", "font-size": "var(--fs-caption)" }}>
                        {d.total_files - d.files.length} more {d.total_files - d.files.length === 1 ? "file wasn't" : "files weren't"} rendered.{" "}
                        <A href={`/git/repos/${repo()}/prs/${n()}/diff?cursor=${d.page.next_cursor}${viewParam() ? `&view=${viewParam()}` : ""}`} style={{ color: "var(--text-primary)" }}>Load remaining files</A>
                      </p>
                    </Show>
                  </Show>
                </>
              )}
            </Show>
          </Suspense>
        </Show>
      </ErrorBoundary>
    </section>
  );
}

const barBtn = {
  display: "inline-flex",
  "align-items": "center",
  gap: "var(--space-1)",
  padding: "var(--space-1) var(--space-3)",
  border: "var(--hairline) solid var(--border)",
  "border-radius": "var(--radius-1)",
  background: "var(--surface-hover)",
  color: "var(--text-primary)",
  cursor: "pointer",
  "font-size": "var(--fs-caption)",
} as const;

/** A compact anchored-thread card (the diff's inline face of the R3.3 thread store). An `outdated`
 *  thread carries the honest detach pill — "Outdated — was on former line N" — never a wrong line. */
function ThreadCard(props: { thread: PrThreadVM; outdated?: boolean }) {
  return (
    <div style={{ ...card, "margin-block": "var(--space-1)" }} data-testid="line-thread">
      <Show when={props.outdated && props.thread.anchor}>
        {(a) => (
          <span data-testid="outdated-pill" style={{ "font-size": "var(--fs-caption)", color: "var(--warning)", display: "inline-flex", "align-items": "center", gap: "var(--space-1)" }}>
            Outdated — was on former line {a().line ?? "?"}
          </span>
        )}
      </Show>
      <For each={props.thread.comments}>
        {(c) => (
          <div style={{ display: "flex", "flex-direction": "column", gap: "var(--space-1)", "padding-block": "var(--space-1)" }}>
            <span style={{ "font-size": "var(--fs-caption)", color: "var(--text-muted)" }}>{c.author.display}</span>
            <Show when={c.state === "visible" && c.body_md} fallback={<span style={{ color: "var(--text-muted)", "font-style": "italic" }}>Comment removed</span>}>
              <Markdown source={c.body_md ?? ""} />
            </Show>
          </div>
        )}
      </For>
    </div>
  );
}

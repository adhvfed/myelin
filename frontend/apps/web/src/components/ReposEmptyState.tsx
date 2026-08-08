// The FRESH-TENANT onboarding empty state (R3.5, dissolves ux-ux-firstrun #2 "empty teaches").
// Onboarding-forward: teaches push-to-create (no "New repository" button — the push IS the create),
// tells you what happens next, and carries a LIVE "waiting for your first push" affordance that flips
// the list in place when the repo.created firehose frame lands (manual Refresh is the always-present
// fallback). Plus the compact, dismissable first-run checklist. Semantic tokens only.
//
// Live channel (OQ-4): subscribes the UNIFIED tenant firehose (`/api/git/events` → the edge
// `/v1/t/{tenant}/events` SSE) and listens for the typed `repo.created`/`repo.pushed` events — NOT a
// second channel, and these are not inbox items. On receipt it revalidates the repos query so the
// list re-renders in place (no reload). FLOOR: the dev-edge fixture holds the stream open but does
// not emit these frames, so the live flip is demonstrable only against the real edge; the manual
// Refresh is the fallback everywhere.
import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { revalidate } from "@solidjs/router";
import { Icon } from "@myelin/design-system";
import { getRepos } from "~/lib/api";

/** A copy-to-clipboard button paired with a labelled `<pre>` command (SR label required). */
function CopyCmd(props: { cmd: string; label: string; testid?: string }) {
  const [copied, setCopied] = createSignal(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(props.cmd);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard denied (permissions / insecure context) — the command stays visible to copy by hand.
    }
  };
  return (
    <div style={{ display: "flex", "align-items": "stretch", gap: "var(--space-2)", "flex-wrap": "wrap" }}>
      <pre
        data-testid={props.testid}
        style={{
          flex: "1",
          "min-width": "16rem",
          margin: "0",
          "font-family": "var(--font-mono)",
          "font-size": "var(--fs-body-sm)",
          background: "var(--surface)",
          border: "var(--hairline) solid var(--border)",
          "border-radius": "var(--radius-1)",
          padding: "var(--space-2) var(--space-3)",
          "white-space": "pre-wrap",
          "overflow-x": "auto",
        }}
      >
        {props.cmd}
      </pre>
      <button
        type="button"
        onClick={copy}
        aria-label={`Copy: ${props.label}`}
        style={{
          display: "inline-flex",
          "align-items": "center",
          gap: "var(--space-1)",
          padding: "var(--space-2) var(--space-3)",
          border: "var(--hairline) solid var(--border-strong)",
          "border-radius": "var(--radius-1)",
          background: "var(--surface)",
          color: "var(--text-primary)",
          cursor: "pointer",
          "font-weight": "500",
        }}
      >
        <Icon name="file" /> {copied() ? "Copied" : "Copy"}
      </button>
    </div>
  );
}

const CHECKLIST = [
  "Push your first repository",
  "Open a pull request",
  "A check reports on the PR",
  "Merge to your default branch",
] as const;

const DISMISS_KEY = "myelin.firstRunChecklist.dismissed";

export function ReposEmptyState(props: { tenant: string }) {
  const remoteCmd = () => `git remote add myelin https://git.eu.myelin.dev/${props.tenant}/<name>.git`;
  const pushCmd = "git push -u myelin main";

  const refresh = () => void revalidate(getRepos.key);
  const [liveStatus, setLiveStatus] = createSignal<"connecting" | "connected" | "unavailable">(
    "connecting",
  );

  // The checklist is dismissable (OQ-2: KEEP with dismiss); persisted client-side.
  const [dismissed, setDismissed] = createSignal(false);
  onMount(() => {
    try {
      if (localStorage.getItem(DISMISS_KEY) === "1") setDismissed(true);
    } catch {
      /* storage unavailable — the checklist simply stays shown */
    }
  });
  const dismiss = () => {
    setDismissed(true);
    try {
      localStorage.setItem(DISMISS_KEY, "1");
    } catch {
      /* ignore */
    }
  };

  // Live subscription to the unified firehose — flip the list in place on repo.created/pushed.
  onMount(() => {
    let es: EventSource | undefined;
    try {
      es = new EventSource("/api/git/events");
      es.onopen = () => setLiveStatus("connected");
      const onRepo = () => refresh();
      es.addEventListener("repo.created", onRepo);
      es.addEventListener("repo.pushed", onRepo);
      // A transport failure can mean the bounded edge stream shed us after an event gap. Refresh the
      // authoritative snapshot once, then close rather than silently staying stale or reconnect-looping.
      es.onerror = () => {
        setLiveStatus("unavailable");
        refresh();
        es?.close();
      };
      onCleanup(() => es?.close());
    } catch {
      setLiveStatus("unavailable");
    }
  });

  return (
    <div style={{ display: "grid", "grid-template-columns": "minmax(0, 1fr) minmax(0, 18rem)", gap: "var(--space-4)", "align-items": "start" }} data-testid="repos-empty">
      {/* LEFT — get your first repository in */}
      <section
        aria-labelledby="get-code-in"
        style={{ border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-2)", background: "var(--surface-raised)", padding: "var(--space-4)", display: "flex", "flex-direction": "column", gap: "var(--space-4)" }}
      >
        <div style={{ display: "flex", "flex-direction": "column", gap: "var(--space-1)" }}>
          <h2 id="get-code-in" style={{ "font-size": "var(--fs-h2)", margin: "0" }}>Get your first repository in</h2>
          <p style={{ margin: "0", color: "var(--text-muted)" }}>
            No repositories in <strong style={{ color: "var(--text-primary)" }}>{props.tenant}</strong> yet — push one to create it.
          </p>
        </div>

        <ol style={{ "list-style": "none", margin: "0", padding: "0", display: "flex", "flex-direction": "column", gap: "var(--space-4)", "counter-reset": "step" }}>
          <li style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
            <span style={{ "font-weight": "600" }}>1 · Point a local repo at Myelin</span>
            <CopyCmd cmd={remoteCmd()} label="git remote add" testid="cmd-remote" />
            <span style={{ color: "var(--text-muted)", "font-size": "var(--fs-body-sm)" }}>
              Swap <code style={{ "font-family": "var(--font-mono)" }}>&lt;name&gt;</code> for your repository name — that's what it will be called.
            </span>
          </li>
          <li style={{ display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
            <span style={{ "font-weight": "600" }}>2 · Push your default branch</span>
            <CopyCmd cmd={pushCmd} label="git push" testid="cmd-push" />
            <span style={{ color: "var(--text-muted)", "font-size": "var(--fs-body-sm)" }}>
              The repository is created on this first push. There's no "New repository" button — the push is the create.
            </span>
          </li>
          <li style={{ display: "flex", "flex-direction": "column", gap: "var(--space-1)" }}>
            <span style={{ "font-weight": "600" }}>3 · What happens next</span>
            <ul style={{ margin: "0", "padding-inline-start": "var(--space-4)", color: "var(--text-muted)", "font-size": "var(--fs-body-sm)", display: "flex", "flex-direction": "column", gap: "var(--space-1)" }}>
              <li>Your repository appears in this list.</li>
              <li>Open a pull request to propose changes.</li>
              <li>Checks report on the PR once a CI provider is connected.</li>
            </ul>
          </li>
        </ol>

        {/* Live "waiting for your first push" — a NEUTRAL live dot (NOT the reserved CI verdict ring).
            The dot is aria-hidden; the text carries the meaning; the region is polite. */}
        <div
          data-testid="waiting-first-push"
          role="status"
          style={{ display: "flex", "align-items": "center", gap: "var(--space-2)", "flex-wrap": "wrap", "border-block-start": "var(--hairline) solid var(--border)", "padding-block-start": "var(--space-3)" }}
        >
          <span
            aria-hidden="true"
            style={{
              width: "0.5rem",
              height: "0.5rem",
              "border-radius": "var(--radius-pill)",
              background:
                liveStatus() === "connected"
                  ? "var(--accent)"
                  : liveStatus() === "unavailable"
                    ? "var(--warning)"
                    : "var(--text-subtle)",
              flex: "none",
            }}
          />
          <span style={{ "font-weight": "500" }}>
            {liveStatus() === "connected"
              ? "Waiting for your first push…"
              : liveStatus() === "unavailable"
                ? "Live updates are unavailable."
                : "Connecting to live updates…"}
          </span>
          <button
            type="button"
            data-testid="repos-refresh"
            onClick={refresh}
            style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", padding: "var(--space-1) var(--space-2)", border: "var(--hairline) solid var(--border-strong)", "border-radius": "var(--radius-1)", background: "var(--surface)", color: "var(--text-primary)", cursor: "pointer", "font-size": "var(--fs-caption)" }}
          >
            <Icon name="search" /> Refresh
          </button>
          <span style={{ flex: "1 1 100%", color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>
            {liveStatus() === "connected"
              ? "This list will update when your push lands. Refresh remains available at any time."
              : liveStatus() === "unavailable"
                ? "Use Refresh to check whether your push has landed."
                : "Refresh is available while the live connection starts."}
          </span>
        </div>
      </section>

      {/* RIGHT — the compact, dismissable first-run checklist (OQ-2 KEEP with dismiss). */}
      <Show when={!dismissed()}>
        <aside
          aria-labelledby="first-run"
          data-testid="first-run-checklist"
          style={{ border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-2)", background: "var(--surface-raised)", padding: "var(--space-4)", display: "flex", "flex-direction": "column", gap: "var(--space-3)" }}
        >
          <div style={{ display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
            <h2 id="first-run" style={{ "font-size": "var(--fs-h3)", margin: "0", flex: "1" }}>First run</h2>
            <button
              type="button"
              data-testid="checklist-dismiss"
              onClick={dismiss}
              aria-label="Dismiss the first-run checklist"
              style={{ display: "inline-flex", "align-items": "center", padding: "var(--space-1)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", background: "var(--surface)", color: "var(--text-muted)", cursor: "pointer" }}
            >
              <Icon name="close" />
            </button>
          </div>
          <ol style={{ "list-style": "none", margin: "0", padding: "0", display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
            <For each={CHECKLIST}>
              {(step, i) => {
                // The first step is the current one ("In progress"); deferred into JSX scope so the
                // reactive index is read within a tracked context (solid/reactivity).
                const current = () => i() === 0;
                return (
                  <li style={{ display: "flex", "align-items": "center", gap: "var(--space-2)" }}>
                    {/* Current step = a filled accent SQUARE marker (§3.1 small-affordance carve-out,
                        carries the visible "In progress" label); todo = a subtle hollow marker. */}
                    <span
                      aria-hidden="true"
                      style={{
                        width: "0.75rem",
                        height: "0.75rem",
                        flex: "none",
                        "border-radius": "2px",
                        background: current() ? "var(--accent)" : "transparent",
                        border: current() ? "none" : "var(--hairline) solid var(--border-strong)",
                      }}
                    />
                    <span style={{ flex: "1", color: current() ? "var(--text-primary)" : "var(--text-muted)", "font-size": "var(--fs-body-sm)", "font-weight": current() ? "500" : "400" }}>
                      {step}
                    </span>
                    <Show when={current()}>
                      <span style={{ color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>In progress</span>
                    </Show>
                  </li>
                );
              }}
            </For>
          </ol>
          <p style={{ margin: "0", color: "var(--text-subtle)", "font-size": "var(--fs-caption)" }}>
            Disappears once you've merged your first PR.
          </p>
        </aside>
      </Show>
    </div>
  );
}

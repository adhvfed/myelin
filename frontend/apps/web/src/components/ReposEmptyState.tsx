import { createSignal, onCleanup, onMount } from "solid-js";
import { revalidate } from "@solidjs/router";
import { Icon } from "@myelin/design-system";
import { getRepos } from "~/lib/api";

function CopyCommand(props: { command: string; label: string; testid: string }) {
  const [copied, setCopied] = createSignal(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(props.command);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // The visible command remains selectable when clipboard access is unavailable.
    }
  };
  return (
    <div style={{ display: "flex", "align-items": "stretch", gap: "var(--space-2)", "flex-wrap": "wrap" }}>
      <pre data-testid={props.testid} style={{ flex: "1", "min-width": "16rem", margin: "0", "font-family": "var(--font-mono)", "font-size": "var(--fs-body-sm)", background: "var(--surface)", border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-1)", padding: "var(--space-2) var(--space-3)", "white-space": "pre-wrap", "overflow-x": "auto" }}>
        {props.command}
      </pre>
      <button type="button" onClick={copy} aria-label={`Copy: ${props.label}`} style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", padding: "var(--space-2) var(--space-3)", border: "var(--hairline) solid var(--border-strong)", "border-radius": "var(--radius-1)", background: "var(--surface)", color: "var(--text-primary)", cursor: "pointer" }}>
        <Icon name="file" /> {copied() ? "Copied" : "Copy"}
      </button>
    </div>
  );
}

export function ReposEmptyState(props: { tenant: string; onCreate: () => void }) {
  const remote = () => `git remote add myelin https://git.eu.myelin.dev/${props.tenant}/<name>.git`;
  const [liveStatus, setLiveStatus] = createSignal<"connecting" | "connected" | "unavailable">("connecting");
  const refresh = () => void revalidate(getRepos.key);

  onMount(() => {
    try {
      const events = new EventSource("/api/git/events");
      events.onopen = () => setLiveStatus("connected");
      const onRepo = () => refresh();
      events.addEventListener("repo.created", onRepo);
      events.addEventListener("repo.pushed", onRepo);
      events.onerror = () => {
        setLiveStatus("unavailable");
        refresh();
        events.close();
      };
      onCleanup(() => events.close());
    } catch {
      setLiveStatus("unavailable");
    }
  });

  return (
    <section data-testid="repos-empty" aria-labelledby="repos-empty-heading" style={{ border: "var(--hairline) solid var(--border)", "border-radius": "var(--radius-2)", background: "var(--surface-raised)", padding: "var(--space-4)", display: "flex", "flex-direction": "column", gap: "var(--space-4)", "max-width": "56rem" }}>
      <div style={{ display: "flex", "align-items": "start", gap: "var(--space-3)", "flex-wrap": "wrap" }}>
        <div style={{ flex: "1 1 20rem" }}>
          <h2 id="repos-empty-heading" style={{ "font-size": "var(--fs-h2)", margin: "0 0 var(--space-1)" }}>Create your first repository</h2>
          <p style={{ margin: "0", color: "var(--text-muted)" }}>Start empty in <strong style={{ color: "var(--text-primary)" }}>{props.tenant}</strong>, or push an existing local repository.</p>
        </div>
        <button type="button" onClick={() => props.onCreate()} style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", padding: "var(--space-2) var(--space-3)", border: "none", "border-radius": "var(--radius-1)", background: "var(--accent)", color: "var(--on-accent)", cursor: "pointer" }}>
          <Icon name="repo" /> Create repository
        </button>
      </div>

      <div style={{ display: "flex", "flex-direction": "column", gap: "var(--space-3)" }}>
        <h3 style={{ "font-size": "var(--fs-h3)", margin: "0" }}>Push an existing repository</h3>
        <CopyCommand command={remote()} label="git remote add" testid="cmd-remote" />
        <CopyCommand command="git push -u myelin main" label="git push" testid="cmd-push" />
      </div>

      <div data-testid="waiting-first-push" role="status" style={{ display: "flex", "align-items": "center", gap: "var(--space-2)", "flex-wrap": "wrap", "border-block-start": "var(--hairline) solid var(--border)", "padding-block-start": "var(--space-3)" }}>
        <span aria-hidden="true" style={{ width: "0.5rem", height: "0.5rem", "border-radius": "var(--radius-pill)", background: liveStatus() === "connected" ? "var(--accent)" : liveStatus() === "unavailable" ? "var(--warning)" : "var(--text-subtle)" }} />
        <span>{liveStatus() === "connected" ? "Waiting for a repository update…" : liveStatus() === "unavailable" ? "Live updates are unavailable." : "Connecting to live updates…"}</span>
        <button type="button" data-testid="repos-refresh" onClick={refresh} style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", padding: "var(--space-1) var(--space-2)", border: "var(--hairline) solid var(--border-strong)", "border-radius": "var(--radius-1)", background: "var(--surface)", color: "var(--text-primary)", cursor: "pointer" }}>
          <Icon name="cycle" /> Refresh
        </button>
      </div>
    </section>
  );
}

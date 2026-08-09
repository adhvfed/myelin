import { createSignal, onCleanup, onMount } from "solid-js";
import { revalidate } from "@solidjs/router";
import { Icon } from "@myelin/design-system";
import { getRepos } from "~/lib/api";

export function ReposEmptyState(props: { tenant: string; onCreate: () => void }) {
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
          <p style={{ margin: "0", color: "var(--text-muted)" }}>Create a home in <strong style={{ color: "var(--text-primary)" }}>{props.tenant}</strong> for your first new or existing codebase.</p>
        </div>
        <button type="button" onClick={() => props.onCreate()} style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", padding: "var(--space-2) var(--space-3)", border: "none", "border-radius": "var(--radius-1)", background: "var(--accent)", color: "var(--on-accent)", cursor: "pointer" }}>
          <Icon name="repo" /> Create repository
        </button>
      </div>

      <ol style={{ margin: "0", padding: "0 0 0 var(--space-4)", color: "var(--text-muted)", display: "flex", "flex-direction": "column", gap: "var(--space-2)" }}>
        <li>Create the repository so Myelin can assign its exact tenant, region, and Edge URL.</li>
        <li>The repository page will guide browser sign-in and configure Git without a pasted API key.</li>
        <li>Push your first commit. Include <code>.myelin/ci.toml</code> to start the first CI run.</li>
      </ol>

      <div data-testid="waiting-first-push" role="status" style={{ display: "flex", "align-items": "center", gap: "var(--space-2)", "flex-wrap": "wrap", "border-block-start": "var(--hairline) solid var(--border)", "padding-block-start": "var(--space-3)" }}>
        <span aria-hidden="true" style={{ width: "0.5rem", height: "0.5rem", "border-radius": "var(--radius-pill)", background: liveStatus() === "connected" ? "var(--accent)" : liveStatus() === "unavailable" ? "var(--warning)" : "var(--text-subtle)" }} />
        <span>{liveStatus() === "connected" ? "Waiting for repository creation…" : liveStatus() === "unavailable" ? "Live updates are unavailable." : "Connecting to live updates…"}</span>
        <button type="button" data-testid="repos-refresh" onClick={refresh} style={{ display: "inline-flex", "align-items": "center", gap: "var(--space-1)", padding: "var(--space-1) var(--space-2)", border: "var(--hairline) solid var(--border-strong)", "border-radius": "var(--radius-1)", background: "var(--surface)", color: "var(--text-primary)", cursor: "pointer" }}>
          <Icon name="cycle" /> Refresh
        </button>
      </div>
    </section>
  );
}

import { createResource, createSignal, Show, Suspense, type JSX } from "solid-js";
import { Dialog, Icon } from "@myelin/design-system";
import { coreInfo, renderMarkdown } from "./bridge";

// Sample content passed through the shared Rust renderer.
const SAMPLE = "**hello, shared myelin-content**";

export function App(): JSX.Element {
  // Both fire on mount, each invoking a Tauri command backed by a Myelin crate.
  const [render] = createResource(() => renderMarkdown(SAMPLE));
  const [info] = createResource(coreInfo);
  const [open, setOpen] = createSignal(false);

  return (
    <main
      style={{
        font: "var(--font-body, 14px/1.5 system-ui)",
        color: "var(--text-primary, #e6e6e6)",
        background: "var(--surface-base, #111)",
        "min-height": "100vh",
        padding: "2rem",
        display: "flex",
        "flex-direction": "column",
        gap: "1rem",
        "align-items": "flex-start",
      }}
    >
      <h1 style={{ "font-size": "var(--type-title, 1.25rem)", margin: "0" }}>
        Myelin — desktop shell
      </h1>
      <p style={{ color: "var(--text-secondary, #999)", margin: "0" }}>
        Tauri 2 wrapping a Solid app; the Rust side reuses the Myelin crates.
      </p>

      <Suspense fallback={<p>Calling the shared Rust core…</p>}>
        <Show when={render()} fallback={<p>(no render result)</p>}>
          {(r) => (
            <p style={{ display: "flex", "align-items": "center", gap: "0.5rem", margin: "0" }}>
              <Icon
                name={r().roundTrips ? "check-pass" : "check-fail"}
                title={r().roundTrips ? "round-trip passed" : "round-trip failed"}
              />
              <span>
                myelin-content round-trip {r().roundTrips ? "PASS" : "FAIL"}:{" "}
                <code>{r().output}</code>
              </span>
            </p>
          )}
        </Show>
      </Suspense>

      <button type="button" onClick={() => setOpen(true)}>
        Shared core info
      </button>

      <Dialog
        open={open()}
        onClose={() => setOpen(false)}
        title="Shared Rust core"
        description="Facts read straight from the linked Myelin crates."
      >
        <Suspense fallback={<p>Loading…</p>}>
          <Show when={info()}>
            {(i) => (
              <ul>
                <li>
                  myelin-content corpus round-trip: {i().contentCorpusPassed}/
                  {i().contentCorpusTotal}
                </li>
                <li>myelin-client default timeout: {i().clientTimeoutMs} ms</li>
              </ul>
            )}
          </Show>
        </Suspense>
      </Dialog>
    </main>
  );
}

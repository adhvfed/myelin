import { createMemo, createSignal, For, Show } from "solid-js";
import { Dialog, Icon, type IconName } from "@myelin/design-system";

export interface Command {
  id: string;
  label: string;
  hint?: string;
  icon?: IconName;
  run: () => void;
}

export interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
  commands: Command[];
  onSearch?: (query: string) => void;
}

export function CommandPalette(props: CommandPaletteProps) {
  const [queryText, setQueryText] = createSignal("");
  const [active, setActive] = createSignal(0);
  let input: HTMLInputElement | undefined;

  const matches = createMemo<Command[]>(() => {
    const q = queryText().trim().toLowerCase();
    const list = props.commands;
    if (!q) return list;
    const commands = list.filter((c) => c.label.toLowerCase().includes(q));
    if (props.onSearch) {
      const search = queryText().trim();
      commands.push({
        id: "search:code",
        label: `Search code for “${search}”`,
        icon: "search",
        run: () => props.onSearch?.(search),
      });
    }
    return commands;
  });

  const clampActive = (n: number) => {
    const len = matches().length;
    if (len === 0) return 0;
    return ((n % len) + len) % len;
  };

  const runActive = () => {
    const m = matches();
    const cmd = m[active()];
    if (!cmd) return;
    props.onClose();
    setQueryText("");
    cmd.run();
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((n) => clampActive(n + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((n) => clampActive(n - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      runActive();
    }
    // Escape is owned by the Dialog primitive (focus-trap + return-focus on close).
  };

  return (
    <Dialog
      open={props.open}
      onClose={() => {
        setQueryText("");
        props.onClose();
      }}
      title="Command palette"
      size="md"
      initialFocus={() => input}
    >
      <div style={{ display: "flex", "flex-direction": "column", gap: "var(--space-3)" }}>
        <div
          style={{
            display: "flex",
            "align-items": "center",
            gap: "var(--space-2)",
            padding: "var(--space-2) var(--space-3)",
            border: "var(--hairline) solid var(--border)",
            "border-radius": "var(--radius-1)",
            background: "var(--surface)",
          }}
        >
          <Icon name="search" />
          <input
            ref={input}
            type="text"
            value={queryText()}
            onInput={(e) => {
              setQueryText(e.currentTarget.value);
              setActive(0);
            }}
            onKeyDown={onKeyDown}
            placeholder="Search or run a command…"
            role="combobox"
            aria-expanded="true"
            aria-controls="cmdk-listbox"
            aria-activedescendant={
              matches().length ? `cmdk-opt-${active()}` : undefined
            }
            aria-label="Search or run a command"
            // No inline `outline: none` — the shared zero-specificity `:focus-visible` ring
            // (tokens.css) must reach the autofocused palette input (DESIGN-MANUAL §6 / must-ship #5).
            style={{
              flex: "1",
              border: "none",
              background: "transparent",
              color: "var(--text-primary)",
              font: "inherit",
            }}
          />
        </div>

        <Show
          when={matches().length > 0}
          fallback={
            <p style={{ color: "var(--text-muted)", margin: "0", padding: "var(--space-3)" }}>
              No matching commands.
            </p>
          }
        >
          <ul
            id="cmdk-listbox"
            role="listbox"
            aria-label="Commands"
            style={{ "list-style": "none", margin: "0", padding: "0", "max-height": "16rem", "overflow-y": "auto" }}
          >
            <For each={matches()}>
              {(cmd, i) => (
                <li
                  id={`cmdk-opt-${i()}`}
                  role="option"
                  aria-selected={i() === active()}
                  onClick={() => {
                    setActive(i());
                    runActive();
                  }}
                  // The primary keyboard surface is the combobox input (↑↓/Enter via
                  // aria-activedescendant). This mirror keeps the pointer-clickable option also
                  // key-operable, satisfying the a11y bar without a second focus stop.
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      setActive(i());
                      runActive();
                    }
                  }}
                  onMouseEnter={() => setActive(i())}
                  style={{
                    display: "flex",
                    "align-items": "center",
                    gap: "var(--space-2)",
                    padding: "var(--space-2) var(--space-3)",
                    "border-radius": "var(--radius-1)",
                    cursor: "pointer",
                    background: i() === active() ? "var(--surface-hover)" : "transparent",
                  }}
                >
                  <Show when={cmd.icon}>{(name) => <Icon name={name()} />}</Show>
                  <span style={{ flex: "1" }}>{cmd.label}</span>
                  <Show when={cmd.hint}>
                    {(hint) => (
                      <kbd
                        style={{
                          "font-family": "var(--font-mono)",
                          "font-size": "var(--fs-caption)",
                          color: "var(--text-subtle)",
                        }}
                      >
                        {hint()}
                      </kbd>
                    )}
                  </Show>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </div>
    </Dialog>
  );
}

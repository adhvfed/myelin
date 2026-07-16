// Menu (Dropdown) — the inline-flow anchored action list (overlays.md §5): row actions,
// block-convert, the identity menu. Non-modal; reuses the substrate (portal, Escape + outside-click
// dismiss via createOverlay) and adds the APG menu-button behaviour: roving tabindex, ↑/↓/Home/End
// navigation, type-ahead, Enter/Space activation, return-focus to the trigger. Items the viewer
// can't run are simply ABSENT (overlays.md §5: "omit unpermitted verbs", never grey-tease).

import {
  For,
  Show,
  createSignal,
  createEffect,
  createUniqueId,
  mergeProps,
  splitProps,
  type JSX,
} from "solid-js";
import { OverlayPortal } from "./primitives/OverlayPortal";
import { createOverlay } from "./primitives/createOverlay";
import { computePosition, type Placement } from "./primitives/position";
import { Icon } from "../Icon";
import type { IconName } from "../icon-names";

export interface MenuItemSpec {
  label: string;
  onSelect: () => void;
  icon?: IconName;
  /** Mono keyboard hint shown right-aligned. */
  kbd?: string;
  disabled?: boolean;
}

export interface MenuProps {
  /** Accessible name for the menu. */
  label: string;
  items: MenuItemSpec[];
  triggerLabel: JSX.Element;
  placement?: Placement;
}

export function Menu(props: MenuProps): JSX.Element {
  const merged = mergeProps({ placement: "bottom-start" as Placement }, props);
  const [local] = splitProps(merged, ["label", "items", "triggerLabel", "placement"]);

  const [open, setOpen] = createSignal(false);
  const [active, setActive] = createSignal(0);
  const [pos, setPos] = createSignal({ left: 0, top: 0, maxBlockSize: 0 });
  let trigger: HTMLButtonElement | undefined;
  let menu: HTMLDivElement | undefined;
  const itemEls: (HTMLButtonElement | undefined)[] = [];
  const menuId = createUniqueId();

  let typeahead = "";
  let typeaheadTimer: ReturnType<typeof setTimeout> | undefined;

  const enabledIndexes = () =>
    local.items.map((it, i) => (it.disabled ? -1 : i)).filter((i) => i !== -1);

  const close = (returnFocus: boolean) => {
    setOpen(false);
    if (returnFocus) trigger?.focus();
  };

  createOverlay({
    isOpen: open,
    onDismiss: () => close(true), // Escape / outside-pointer → close + return focus to trigger
    contentRef: () => menu,
    triggerRef: () => trigger,
    modal: false,
    autoFocus: false, // we drive roving focus to the active item ourselves
    restoreFocus: false,
  });

  const openMenu = (start: "first" | "last") => {
    const enabled = enabledIndexes();
    if (enabled.length === 0) {
      setActive(-1);
    } else {
      setActive((start === "first" ? enabled[0] : enabled[enabled.length - 1]) ?? -1);
    }
    setOpen(true);
  };

  // Position + move focus to the active item once the menu mounts / active changes.
  createEffect(() => {
    if (!open() || !trigger || !menu) return;
    const p = computePosition(trigger, menu, local.placement);
    setPos({ left: p.left, top: p.top, maxBlockSize: p.maxBlockSize });
  });
  createEffect(() => {
    if (!open()) return;
    const i = active();
    if (i >= 0) itemEls[i]?.focus();
    else menu?.focus();
  });

  const moveActive = (dir: 1 | -1) => {
    const enabled = enabledIndexes();
    if (enabled.length === 0) return;
    const cur = enabled.indexOf(active());
    const next = cur === -1 ? 0 : (cur + dir + enabled.length) % enabled.length;
    setActive(enabled[next] ?? -1);
  };

  const onTypeahead = (ch: string) => {
    if (typeaheadTimer) clearTimeout(typeaheadTimer);
    typeahead += ch.toLowerCase();
    typeaheadTimer = setTimeout(() => (typeahead = ""), 500);
    const enabled = enabledIndexes();
    const match = enabled.find((i) => local.items[i]?.label.toLowerCase().startsWith(typeahead));
    if (match !== undefined) setActive(match);
  };

  const onTriggerKeydown = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      openMenu("first");
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      openMenu("last");
    }
  };

  const onMenuKeydown = (e: KeyboardEvent) => {
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        moveActive(1);
        break;
      case "ArrowUp":
        e.preventDefault();
        moveActive(-1);
        break;
      case "Home":
        e.preventDefault();
        setActive(enabledIndexes()[0] ?? -1);
        break;
      case "End": {
        e.preventDefault();
        const en = enabledIndexes();
        setActive(en[en.length - 1] ?? -1);
        break;
      }
      case "Enter":
      case " ": {
        e.preventDefault();
        const sel = local.items[active()];
        if (sel && !sel.disabled) {
          sel.onSelect();
          close(true);
        }
        break;
      }
      case "Tab":
        // APG: Tab closes the menu; we return focus to the trigger for a predictable landing.
        e.preventDefault();
        close(true);
        break;
      default:
        // Type-ahead for single printable characters.
        if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
          e.preventDefault();
          onTypeahead(e.key);
        }
    }
  };

  return (
    <>
      <button
        ref={trigger}
        type="button"
        aria-haspopup="menu"
        aria-expanded={open()}
        aria-controls={open() ? menuId : undefined}
        onClick={() => (open() ? close(true) : openMenu("first"))}
        onKeyDown={onTriggerKeydown}
        style={{
          display: "inline-flex",
          "align-items": "center",
          gap: "var(--space-1)",
          height: "var(--control-h)",
          padding: "0 var(--space-3)",
          background: "var(--surface-raised)",
          color: "var(--text-primary)",
          border: "var(--hairline) solid var(--border)",
          "border-radius": "var(--radius-1)",
          cursor: "pointer",
        }}
      >
        {local.triggerLabel}
        <Icon name="chevron" size={14} />
      </button>

      <Show when={open()}>
        <OverlayPortal>
          <div
            ref={menu}
            id={menuId}
            role="menu"
            aria-label={local.label}
            tabindex={-1}
            onKeyDown={onMenuKeydown}
            style={{
              position: "fixed",
              left: `${pos().left}px`,
              top: `${pos().top}px`,
              "z-index": "var(--z-popover)",
              "min-inline-size": "12rem",
              // Apply the positioner's viewport clamp so a long menu SCROLLS instead of overflowing
              // off-screen (fe-ds finding 2 — otherwise roved-to items render invisibly below the fold).
              "max-block-size": pos().maxBlockSize > 0 ? `${pos().maxBlockSize}px` : undefined,
              "overflow-y": "auto",
              background: "var(--surface-overlay)",
              border: "var(--hairline) solid var(--border)",
              "border-radius": "var(--radius-1)",
              "box-shadow": "var(--shadow-popover)",
              padding: "var(--space-1)",
              "font-family": "var(--font-sans)",
              "font-size": "var(--fs-body)",
              transition: "opacity var(--dur-micro) var(--ease-enter)",
            }}
          >
            <Show
              when={local.items.length > 0}
              fallback={
                <p style={{ margin: "0", padding: "var(--space-2)", color: "var(--text-subtle)" }}>
                  No actions available
                </p>
              }
            >
              <For each={local.items}>
                {(item, i) => (
                  <button
                    ref={(el) => (itemEls[i()] = el)}
                    type="button"
                    role="menuitem"
                    tabindex={active() === i() ? 0 : -1}
                    // aria-disabled ONLY — never the native `disabled` attribute, which drops the item
                    // from the a11y tree entirely so SR users can't perceive the action exists and the
                    // announced item count diverges from the visual list (APG, fe-ds finding 4). The
                    // enabledIndexes() roving-skip + the onClick early-return already prevent activation.
                    aria-disabled={item.disabled}
                    onClick={() => {
                      if (item.disabled) return;
                      item.onSelect();
                      close(true);
                    }}
                    onPointerEnter={() => !item.disabled && setActive(i())}
                    style={{
                      display: "flex",
                      "align-items": "center",
                      gap: "var(--space-2)",
                      width: "100%",
                      height: "var(--row-h)",
                      padding: "0 var(--space-2)",
                      background: active() === i() ? "var(--surface-hover)" : "transparent",
                      color: item.disabled ? "var(--text-subtle)" : "var(--text-primary)",
                      border: "none",
                      "border-radius": "var(--radius-1)",
                      "text-align": "start",
                      cursor: item.disabled ? "default" : "pointer",
                    }}
                  >
                    <Show when={item.icon}>
                      {(name) => <Icon name={name()} size={14} />}
                    </Show>
                    <span style={{ flex: "1" }}>{item.label}</span>
                    <Show when={item.kbd}>
                      <kbd
                        style={{
                          color: "var(--text-subtle)",
                          "font-family": "var(--font-mono)",
                          "font-size": "var(--fs-caption)",
                        }}
                      >
                        {item.kbd}
                      </kbd>
                    </Show>
                  </button>
                )}
              </For>
            </Show>
          </div>
        </OverlayPortal>
      </Show>
    </>
  );
}

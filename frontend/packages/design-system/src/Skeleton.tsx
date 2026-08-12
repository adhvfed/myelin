// Loading skeleton with `aria-busy` and a debounced, page-shared polite announcement. Callers can
// use row props or provide a custom layout that matches the loaded content.

import { For, Show, onMount, onCleanup, mergeProps, type JSX } from "solid-js";

// Module-level state combines concurrent skeletons and suppresses announcements for very short loads.
const DEBOUNCE_MS = 150;

let liveRegion: HTMLElement | undefined;
let activeCount = 0;
let announceTimer: ReturnType<typeof setTimeout> | undefined;
let loadingCommitted = false;

function ensureLiveRegion(): HTMLElement | undefined {
  if (typeof document === "undefined") return undefined;
  if (liveRegion && document.body.contains(liveRegion)) return liveRegion;
  const el = document.createElement("div");
  el.setAttribute("aria-live", "polite");
  el.setAttribute("aria-atomic", "true");
  el.setAttribute("role", "status");
  el.setAttribute("data-myelin-skeleton-live", "");
  // Visually hidden but available to AT — inlined so it works without any app stylesheet.
  Object.assign(el.style, {
    position: "absolute",
    width: "1px",
    height: "1px",
    padding: "0",
    margin: "-1px",
    overflow: "hidden",
    clip: "rect(0, 0, 0, 0)",
    whiteSpace: "nowrap",
    border: "0",
  } as Partial<CSSStyleDeclaration>);
  document.body.appendChild(el);
  liveRegion = el;
  return el;
}

/** A skeleton appeared — schedule the polite "loading" announcement (debounced). */
function registerLoading(message: string): void {
  activeCount += 1;
  if (activeCount > 1) return; // one announcement covers all concurrent skeletons
  const region = ensureLiveRegion();
  if (!region) return;
  clearTimeout(announceTimer);
  announceTimer = setTimeout(() => {
    region.textContent = message;
    loadingCommitted = true;
  }, DEBOUNCE_MS);
}

/** A skeleton went away — when the last one clears, announce "loaded" (only if we ever said loading). */
function registerLoaded(message: string): void {
  activeCount = Math.max(0, activeCount - 1);
  if (activeCount > 0) return; // still loading elsewhere
  clearTimeout(announceTimer);
  if (!loadingCommitted) return; // sub-threshold load never announced start → stay silent
  const region = ensureLiveRegion();
  if (!region) return;
  announceTimer = setTimeout(() => {
    region.textContent = message;
    loadingCommitted = false;
  }, DEBOUNCE_MS);
}

// --------------------------------------------------------------------------------------------------
// SkeletonBlock — a single shimmer-free placeholder bar/box. Compose these into a bespoke structure,
// or let <Skeleton rows> lay out a default stack of them.
// --------------------------------------------------------------------------------------------------
export interface SkeletonBlockProps {
  /** Block height (a spacing/size token or length). Default 1rem. */
  height?: string;
  /** Block inline-size. Default 100%. */
  width?: string;
  radius?: string;
  style?: JSX.CSSProperties;
}

export function SkeletonBlock(props: SkeletonBlockProps): JSX.Element {
  return (
    <div
      aria-hidden="true"
      style={{
        height: props.height ?? "1rem",
        width: props.width ?? "100%",
        "border-radius": props.radius ?? "var(--radius-1)",
        background: "var(--surface-hover)",
        ...props.style,
      }}
    />
  );
}

export interface SkeletonProps {
  /** Number of default placeholder rows (ignored when `children` is provided). Default 3. */
  rows?: number;
  /** Height of each default row. Default 2.5rem. */
  rowHeight?: string;
  /** Gap between rows. Default var(--space-2). */
  gap?: string;
  /** The polite message announced while loading. Default "Loading…". */
  label?: string;
  /** The polite message announced once loading completes. Default "Loaded". */
  loadedLabel?: string;
  /** A bespoke structure-matching layout of <SkeletonBlock>s (overrides the default rows). */
  children?: JSX.Element;
  "data-testid"?: string;
  style?: JSX.CSSProperties;
}

export function Skeleton(props: SkeletonProps): JSX.Element {
  const merged = mergeProps(
    { rows: 3, rowHeight: "2.5rem", gap: "var(--space-2)", label: "Loading…", loadedLabel: "Loaded" },
    props,
  );

  // onMount never runs on the server, so the live-region DOM work stays client-only (SSR-safe).
  onMount(() => {
    registerLoading(merged.label);
    onCleanup(() => registerLoaded(merged.loadedLabel));
  });

  return (
    <div
      aria-busy="true"
      data-testid={props["data-testid"] ?? "skeleton"}
      style={{
        display: "flex",
        "flex-direction": "column",
        gap: merged.gap,
        ...merged.style,
      }}
    >
      <Show
        when={props.children}
        fallback={
          <For each={Array.from({ length: merged.rows })}>
            {() => <SkeletonBlock height={merged.rowHeight} />}
          </For>
        }
      >
        {props.children}
      </Show>
    </div>
  );
}

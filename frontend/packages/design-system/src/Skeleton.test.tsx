// Skeleton rendering and debounced live-region behavior.
import { render, screen } from "@solidjs/testing-library";
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createSignal, Show } from "solid-js";
import { Skeleton, SkeletonBlock } from "./Skeleton";

describe("Skeleton", () => {
  it("sets aria-busy on its container and renders the requested placeholder rows", () => {
    render(() => <Skeleton rows={4} data-testid="sk" />);
    const el = screen.getByTestId("sk");
    expect(el.getAttribute("aria-busy")).toBe("true");
    // rows are shimmer-free aria-hidden blocks (no spinner).
    expect(el.querySelectorAll('[aria-hidden="true"]')).toHaveLength(4);
    expect(el.textContent).toBe(""); // no "Loading…" text baked into the visual layer
  });

  it("renders a bespoke structure-matching layout when given children", () => {
    render(() => (
      <Skeleton data-testid="sk">
        <SkeletonBlock height="2rem" width="10rem" />
        <SkeletonBlock height="8rem" />
      </Skeleton>
    ));
    expect(screen.getByTestId("sk").querySelectorAll('[aria-hidden="true"]')).toHaveLength(2);
  });
});

describe("Skeleton live-region announcements (one debounced polite region)", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    vi.useRealTimers();
    document.querySelectorAll("[data-myelin-skeleton-live]").forEach((n) => n.remove());
  });

  it("announces a debounced polite 'Loading…' then 'Loaded' through a single shared region", () => {
    const [loading, setLoading] = createSignal(true);
    render(() => (
      <Show when={loading()} fallback={<p>done</p>}>
        <Skeleton rows={2} label="Loading commits…" loadedLabel="Commits loaded" />
      </Show>
    ));

    // Exactly one live region, polite, created lazily.
    const regions = document.querySelectorAll("[data-myelin-skeleton-live]");
    expect(regions).toHaveLength(1);
    const region = regions[0] as HTMLElement;
    expect(region.getAttribute("aria-live")).toBe("polite");
    expect(region.textContent).toBe(""); // debounced — nothing announced yet

    vi.advanceTimersByTime(200); // past the debounce window
    expect(region.textContent).toBe("Loading commits…");

    setLoading(false); // content arrived → skeleton unmounts
    vi.advanceTimersByTime(200);
    expect(region.textContent).toBe("Commits loaded");
    // Still exactly one shared region (not one-per-skeleton).
    expect(document.querySelectorAll("[data-myelin-skeleton-live]")).toHaveLength(1);
  });

  it("stays silent for a sub-threshold load (mount + unmount within the debounce window)", () => {
    const [loading, setLoading] = createSignal(true);
    render(() => (
      <Show when={loading()} fallback={<p>done</p>}>
        <Skeleton rows={1} />
      </Show>
    ));
    setLoading(false); // resolves before the 150ms debounce fires
    vi.advanceTimersByTime(400);
    const region = document.querySelector("[data-myelin-skeleton-live]");
    // Never announced "Loading…", so it must not announce "Loaded" either — no flash-of-announcement.
    expect(region?.textContent ?? "").toBe("");
  });
});

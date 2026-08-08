import { Show, createSignal, onCleanup, onMount } from "solid-js";
import { Icon } from "@myelin/design-system";
import { getCiLog } from "~/lib/api";
import { consumeCiLiveStream, type CiLiveEvent } from "~/lib/ci-live-stream";

const LIVE_WINDOW_BYTES = 256 * 1024;
const MAX_RANGE_BYTES = 256 * 1024;
const RECONNECT_DELAY_MS = 250;
const MAX_RECONNECT_DELAY_MS = 5_000;

type LiveStatus = "loading" | "connecting" | "live" | "resyncing" | "complete" | "unavailable";

function decodeBase64(value: string): Uint8Array {
  const decoded = atob(value);
  return Uint8Array.from(decoded, (character) => character.charCodeAt(0));
}

function waitForReconnect(signal: AbortSignal, failures: number): Promise<void> {
  return new Promise((resolve) => {
    const delay = Math.min(
      MAX_RECONNECT_DELAY_MS,
      RECONNECT_DELAY_MS * 2 ** Math.min(failures, 5),
    );
    const timer = window.setTimeout(resolve, delay);
    signal.addEventListener("abort", () => {
      window.clearTimeout(timer);
      resolve();
    }, { once: true });
  });
}

export function CiLiveLog(props: { run: string; job: string }) {
  const [status, setStatus] = createSignal<LiveStatus>("loading");
  const [windowStart, setWindowStart] = createSignal(0);
  const [windowEnd, setWindowEnd] = createSignal(0);
  const [bytes, setBytes] = createSignal<Uint8Array<ArrayBufferLike>>(new Uint8Array());
  const [resyncs, setResyncs] = createSignal(0);

  const replaceWindow = (start: number, end: number, next: Uint8Array) => {
    const bounded = next.byteLength > LIVE_WINDOW_BYTES
      ? next.slice(next.byteLength - LIVE_WINDOW_BYTES)
      : next;
    setBytes(bounded);
    setWindowEnd(end);
    setWindowStart(end - bounded.byteLength);
    if (start > end - bounded.byteLength) throw new Error("CI_LIVE_INVALID_WINDOW");
  };

  const appendWindow = (start: number, end: number, next: Uint8Array) => {
    if (start !== windowEnd() || end - start !== next.byteLength) {
      throw new Error("CI_LIVE_NONCONTIGUOUS_ARCHIVE");
    }
    const current = bytes();
    const joined = new Uint8Array(current.byteLength + next.byteLength);
    joined.set(current);
    joined.set(next, current.byteLength);
    replaceWindow(windowStart(), end, joined);
  };

  const readSnapshot = async () => {
    // Discover the old head, then accept the exact bounded range ending at that head even if the
    // writer advances meanwhile. The fresh SSE checkpoint catches up [old_head, current_head), so
    // taking a coherent older snapshot never requires the producer to become quiescent.
    const head = await getCiLog({
      run: props.run,
      job: props.job,
      start: Number.MAX_SAFE_INTEGER,
      limit: 1,
    });
    const start = Math.max(0, head.total_end - LIVE_WINDOW_BYTES);
    const range = await getCiLog({
      run: props.run,
      job: props.job,
      start,
      limit: LIVE_WINDOW_BYTES,
    });
    if (range.byte_start !== start || range.byte_end < head.total_end ||
        range.total_end < head.total_end) throw new Error("CI_LIVE_SNAPSHOT_REGRESSED");
    replaceWindow(range.byte_start, range.byte_end, decodeBase64(range.data));
  };

  const readThrough = async (target: number) => {
    if (target < windowEnd()) return;
    while (windowEnd() < target) {
      const start = windowEnd();
      const limit = Math.min(MAX_RANGE_BYTES, target - start);
      const range = await getCiLog({ run: props.run, job: props.job, start, limit });
      if (range.byte_start !== start || range.byte_end <= start || range.byte_end > target ||
          range.total_end < target) throw new Error("CI_LIVE_ARCHIVE_GAP");
      appendWindow(start, range.byte_end, decodeBase64(range.data));
    }
  };

  onMount(() => {
    const controller = new AbortController();
    let cursor: string | undefined;
    let failures = 0;

    const accept = async (event: CiLiveEvent) => {
      if (event.kind === "ready") {
        if (cursor !== undefined) throw new Error("CI_LIVE_UNEXPECTED_CHECKPOINT");
        await readThrough(event.byte_end);
        cursor = event.cursor;
        setStatus("live");
        return;
      }
      if (event.kind === "appended") {
        if (cursor !== undefined && BigInt(event.cursor) <= BigInt(cursor)) {
          throw new Error("CI_LIVE_NONMONOTONE_CURSOR");
        }
        if (event.byte_start > windowEnd()) await readThrough(event.byte_start);
        if (event.byte_start < windowEnd() && event.byte_end > windowEnd()) {
          throw new Error("CI_LIVE_OVERLAPPING_POINTER");
        }
        if (event.byte_end > windowEnd()) await readThrough(event.byte_end);
        cursor = event.cursor;
        setStatus("live");
        return;
      }
      await readThrough(event.byte_end);
      if (event.cursor !== undefined) {
        if (cursor !== undefined && event.cursor !== cursor) {
          throw new Error("CI_LIVE_TERMINAL_CURSOR_DRIFT");
        }
        cursor = event.cursor;
      }
      setStatus("complete");
    };

    const run = async () => {
      while (!controller.signal.aborted) {
        try {
          await readSnapshot();
          failures = 0;
          break;
        } catch {
          setStatus("unavailable");
          failures += 1;
          await waitForReconnect(controller.signal, failures);
        }
      }
      if (controller.signal.aborted) return;
      while (!controller.signal.aborted && status() !== "complete") {
        setStatus("connecting");
        const headers = new Headers({ accept: "text/event-stream" });
        if (cursor !== undefined) headers.set("last-event-id", cursor);
        try {
          const response = await fetch(
            `/api/ci/runs/${encodeURIComponent(props.run)}/jobs/${encodeURIComponent(props.job)}/log/live`,
            { headers, signal: controller.signal },
          );
          if (response.status === 401) {
            await response.body?.cancel().catch(() => undefined);
            window.location.assign("/login");
            return;
          }
          if (response.status === 409) {
            await response.body?.cancel().catch(() => undefined);
            setStatus("resyncing");
            await readSnapshot();
            cursor = undefined;
            setResyncs((value) => value + 1);
            failures = 0;
            continue;
          }
          if (response.status === 400 || response.status === 404) {
            await response.body?.cancel().catch(() => undefined);
            setStatus("unavailable");
            return;
          }
          if (!response.ok) {
            await response.body?.cancel().catch(() => undefined);
            throw new Error(`CI_LIVE_HTTP_${response.status}`);
          }
          setStatus("live");
          await consumeCiLiveStream(response, { run: props.run, job: props.job }, accept);
          failures = 0;
          if (status() === "complete") return;
        } catch {
          if (controller.signal.aborted) return;
          setStatus("unavailable");
          failures += 1;
        }
        await waitForReconnect(controller.signal, failures);
      }
    };
    void run();
    onCleanup(() => controller.abort());
  });

  const label = () => {
    switch (status()) {
      case "loading": return "Loading durable log snapshot…";
      case "connecting": return "Connecting to durable live output…";
      case "live": return "Live · durable bytes";
      case "resyncing": return "Cursor expired · reloading durable archive…";
      case "complete": return "Complete · durable output";
      case "unavailable": return "Live connection interrupted · reconnecting…";
    }
  };
  const text = () => {
    const value = bytes();
    return value.byteLength === 0
      ? "No durable output yet."
      : new TextDecoder("utf-8", { fatal: false }).decode(value);
  };

  return (
    <section aria-labelledby="ci-live-log-heading" class="ci-live-log">
      <div class="ci-log-heading">
        <div>
          <p class="ci-eyebrow">Bounded recent window</p>
          <h3 id="ci-live-log-heading">Live output</h3>
        </div>
        <span class="ci-live-label" data-status={status()}>
          <Icon name={status() === "complete" ? "check-pass" : "check-pending"} />
          {status() === "complete" ? "Complete" : "Live"}
        </span>
      </div>
      <p class="ci-live-state" role="status" aria-live="polite" data-testid="ci-live-state">
        {label()}
      </p>
      <Show when={resyncs() > 0}>
        <p class="ci-live-resynced" data-testid="ci-live-resynced">
          Reloaded the durable archive after the live cursor expired.
        </p>
      </Show>
      <p class="ci-log-range">
        Recent bytes {windowStart()}–{windowEnd()}
      </p>
      <textarea
        readOnly
        aria-label="Live job output"
        data-testid="ci-live-log"
        value={text()}
      />
    </section>
  );
}

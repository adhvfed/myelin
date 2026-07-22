import type { DiffLineVM } from "./api";
import type { FileLine } from "./file-lines";

export const MAX_EXPANDED_CONTEXT_LINES = 1_000;

export interface PrDiffContextRange {
  start: number;
  end: number;
  oldLineOffset: number;
}

/**
 * Resolve the unchanged new-side gap immediately before one hunk. `gapKey` is the hunk index
 * emitted by DiffViewer. The range excludes both surrounding hunks and refuses an unbounded gap;
 * callers must never turn one click into arbitrarily many file-lines requests.
 */
export function prDiffContextRange(
  file: {
    hunks: Array<{
      old_start: number;
      old_lines?: number;
      new_start: number;
      new_lines: number;
    }>;
  },
  gapKey: string,
): PrDiffContextRange | null {
  if (!/^(0|[1-9]\d*)$/.test(gapKey)) return null;
  const index = Number(gapKey);
  if (!Number.isSafeInteger(index) || index < 0 || index >= file.hunks.length) return null;
  const next = file.hunks[index];
  const previous = index > 0 ? file.hunks[index - 1] : undefined;
  if (!next) return null;

  const start = previous ? previous.new_start + previous.new_lines : 1;
  const end = next.new_start - 1;
  const count = end - start + 1;
  const oldLineOffset = next.old_start - next.new_start;
  if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) ||
      !Number.isSafeInteger(oldLineOffset) || start < 1 || end < start ||
      count > MAX_EXPANDED_CONTEXT_LINES) return null;
  return { start, end, oldLineOffset };
}

/** New-side blob lines are unchanged in a collapsed gap, so both gutters can be reconstructed. */
export function mapPrDiffContextLines(
  lines: readonly FileLine[],
  range: PrDiffContextRange,
): DiffLineVM[] | null {
  const expectedLength = range.end - range.start + 1;
  if (lines.length !== expectedLength) return null;
  const mapped: DiffLineVM[] = [];
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (!line || line.new_no !== range.start + index) return null;
    const oldNo = line.new_no + range.oldLineOffset;
    if (!Number.isSafeInteger(oldNo) || oldNo < 1) return null;
    mapped.push({ origin: " ", content: line.content, old_no: oldNo, new_no: line.new_no });
  }
  return mapped;
}

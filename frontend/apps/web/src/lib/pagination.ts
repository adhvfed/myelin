export interface CursorPage {
  page: { next_cursor: string | null };
}

export function nextCursorForPages<T extends CursorPage>(
  first: T | null | undefined,
  loaded: T[],
): string | null {
  if (loaded.length > 0) return loaded.at(-1)!.page.next_cursor;
  return first?.page.next_cursor ?? null;
}

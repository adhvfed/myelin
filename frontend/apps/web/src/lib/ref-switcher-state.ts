import type { GitRefsInput } from "./git-read-input";
import type { PinnedRefRow, RefsVM } from "./api";

export const REF_SWITCHER_PAGE_LIMIT = 100;
export const REF_SWITCHER_ROW_CAP = 300;
export const REF_SWITCHER_SEARCH_DEBOUNCE_MS = 200;

export interface SwitchRefRow {
  kind: "branch" | "tag";
  fullName: string;
  name: string;
  oid: string;
  isDefault: boolean;
}

export interface RefSwitcherSnapshot {
  query: string;
  pins: SwitchRefRow[];
  rows: SwitchRefRow[];
  nextCursor: string | null;
  loading: boolean;
  capped: boolean;
  error: string | null;
}

export interface RefSearch {
  repo: string;
  query: string;
  current?: string;
}

export type RefFetcher = (input: GitRefsInput) => Promise<RefsVM>;

function switchRow(
  kind: "branch" | "tag",
  row: { name: string; oid: string; is_default?: boolean },
): SwitchRefRow {
  return {
    kind,
    fullName: `${kind === "branch" ? "refs/heads/" : "refs/tags/"}${row.name}`,
    name: row.name,
    oid: row.oid,
    isDefault: row.is_default === true,
  };
}

function switchPin(row: PinnedRefRow): SwitchRefRow {
  return {
    kind: row.kind,
    fullName: row.full_name,
    name: row.name,
    oid: row.oid,
    isDefault: row.is_default,
  };
}

function rowsOf(refs: RefsVM): SwitchRefRow[] {
  return [
    ...refs.branches.map((row) => switchRow("branch", row)),
    ...refs.tags.map((row) => switchRow("tag", row)),
  ];
}

function requestOf(search: RefSearch, cursor?: string): GitRefsInput {
  return {
    repo: search.repo,
    limit: REF_SWITCHER_PAGE_LIMIT,
    ...(search.query ? { q: search.query } : {}),
    ...(search.current ? { current: search.current } : {}),
    ...(cursor ? { cursor } : {}),
  };
}

export function visibleRefGroups(snapshot: RefSwitcherSnapshot): {
  pins: SwitchRefRow[];
  branches: SwitchRefRow[];
  tags: SwitchRefRow[];
} {
  const pinned = new Set(snapshot.pins.map((row) => row.fullName));
  const rows = snapshot.rows.filter((row) => !pinned.has(row.fullName));
  return {
    pins: snapshot.pins,
    branches: rows.filter((row) => row.kind === "branch"),
    tags: rows.filter((row) => row.kind === "tag"),
  };
}

export class RefSwitcherController {
  private generation = 0;
  private searchInput: RefSearch | null = null;
  private state: RefSwitcherSnapshot = {
    query: "",
    pins: [],
    rows: [],
    nextCursor: null,
    loading: false,
    capped: false,
    error: null,
  };

  constructor(
    private readonly fetchRefs: RefFetcher,
    private readonly changed: (snapshot: RefSwitcherSnapshot) => void,
  ) {}

  snapshot(): RefSwitcherSnapshot {
    return { ...this.state, pins: [...this.state.pins], rows: [...this.state.rows] };
  }

  private publish(): void {
    this.changed(this.snapshot());
  }

  prepare(search: RefSearch): void {
    this.generation += 1;
    this.searchInput = { ...search };
    this.state = {
      query: search.query,
      pins: [],
      rows: [],
      nextCursor: null,
      loading: true,
      capped: false,
      error: null,
    };
    this.publish();
  }

  private apply(refs: RefsVM, append: boolean): void {
    const byName = new Map<string, SwitchRefRow>();
    if (append) {
      for (const row of this.state.rows) byName.set(row.fullName, row);
    }
    for (const row of rowsOf(refs)) byName.set(row.fullName, row);
    const allRows = [...byName.values()];
    const overflow = allRows.length > REF_SWITCHER_ROW_CAP;
    const rows = allRows.slice(0, REF_SWITCHER_ROW_CAP);
    const capped = overflow || (rows.length === REF_SWITCHER_ROW_CAP && refs.page.next_cursor !== null);
    this.state = {
      ...this.state,
      pins: refs.pinned.map(switchPin),
      rows,
      nextCursor: capped ? null : refs.page.next_cursor,
      loading: false,
      capped,
      error: null,
    };
    this.publish();
  }

  async search(search: RefSearch): Promise<void> {
    const generation = ++this.generation;
    this.searchInput = { ...search };
    this.state = {
      query: search.query,
      pins: [],
      rows: [],
      nextCursor: null,
      loading: true,
      capped: false,
      error: null,
    };
    this.publish();
    try {
      const refs = await this.fetchRefs(requestOf(search));
      if (generation !== this.generation) return;
      this.apply(refs, false);
    } catch {
      if (generation !== this.generation) return;
      this.state = { ...this.state, loading: false, error: "Unable to load refs." };
      this.publish();
    }
  }

  async loadMore(): Promise<void> {
    const search = this.searchInput;
    const cursor = this.state.nextCursor;
    if (!search || !cursor || this.state.loading || this.state.capped) return;
    const generation = this.generation;
    this.state = { ...this.state, loading: true, error: null };
    this.publish();
    try {
      const refs = await this.fetchRefs(requestOf(search, cursor));
      if (generation !== this.generation) return;
      this.apply(refs, true);
    } catch {
      if (generation !== this.generation) return;
      this.state = { ...this.state, loading: false, error: "Unable to load more refs." };
      this.publish();
    }
  }

  dispose(): void {
    this.generation += 1;
    this.searchInput = null;
  }
}

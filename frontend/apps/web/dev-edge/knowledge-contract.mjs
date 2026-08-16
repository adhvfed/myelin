// Stateful Knowledge contract fixture. It mirrors the production page/block JSON and optimistic
// version boundary; storage is deliberately in-memory because this file is only the browser double.
const ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
function encode(value, length) { let current = BigInt(value); let out = ""; for (let i = 0; i < length; i += 1) { out = ALPHABET[Number(current & 31n)] + out; current >>= 5n; } return out; }
function ulid(sequence) { return encode(1_760_000_000_000n + BigInt(sequence), 10) + encode(sequence, 16); }
function exact(value, keys) { return value && typeof value === "object" && !Array.isArray(value) && Object.keys(value).length === keys.length && Object.keys(value).every((key) => keys.includes(key)); }
function clean(value, max, empty = false) { return typeof value === "string" && (empty || value.length > 0) && Buffer.byteLength(value, "utf8") <= max && !value.includes("\0"); }
const TYPES = ["paragraph", "heading", "bullet_list", "ordered_list", "task_list", "blockquote", "code_block", "callout", "divider"];
function operationId(value) { return typeof value === "string" && /^[!-~]{1,128}$/.test(value); }

function blocksFor(template) {
  if (template === "blank") return [["paragraph", ""]];
  if (template === "product-spec") return [["heading", "Problem"], ["paragraph", "What user or organisational problem are we solving?"], ["heading", "Outcomes"], ["bullet_list", "Describe the measurable change this work should create."], ["heading", "Approach"], ["paragraph", "Explain the smallest coherent approach and the alternatives considered."], ["heading", "Risks"], ["bullet_list", "Name failure modes, privacy implications, and how we will observe them."]];
  if (template === "runbook") return [["heading", "When to use this runbook"], ["paragraph", "Describe the signals and impact that should trigger this response."], ["heading", "Response"], ["ordered_list", "Confirm the alert and establish an incident lead."], ["ordered_list", "Stabilise the service before changing multiple variables."], ["heading", "Recovery checks"], ["task_list", "User-visible health has recovered and remains stable."], ["task_list", "Follow-up work has an owner and is linked to the incident."]];
  return null;
}

function summary(row) { return { id: row.id, ref: `myelin://acme/knowledge/page/${row.id}`, space: "engineering", parent_page_id: null, title: row.title, title_state: "active", visibility: row.visibility, version: row.version, can_edit: true, created_at: row.created_at, updated_at: row.updated_at }; }
function document(row) { return { ...summary(row), blocks: row.blocks.map((block) => ({ ...block, references: block.references ?? [], state: "active", is_you: true })) }; }

export function parseKnowledgeQuery(raw) {
  const values = new URLSearchParams(raw);
  if ([...values.keys()].some((key) => !["limit", "cursor"].includes(key)) || [...values.keys()].some((key) => values.getAll(key).length !== 1)) return null;
  const rawLimit = values.get("limit"); const limit = rawLimit === null ? 50 : Number(rawLimit);
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 100 || (rawLimit !== null && String(limit) !== rawLimit)) return null;
  const cursor = values.get("cursor") ?? undefined;
  if (cursor && !/^[0-9A-HJKMNP-TV-Z]{26}$/.test(cursor)) return null;
  return { limit, cursor };
}

export class KnowledgeFixtures {
  constructor() { this.reset(); }
  reset({ empty = false } = {}) {
    this.sequence = 40; this.nonces = new Map();
    this.pages = empty ? [] : [
      { id: ulid(1), title: "Engineering principles", visibility: "team", version: 3, created_at: 1_760_000_000, updated_at: 1_760_003_000, blocks: [{ id: ulid(11), type: "heading", markdown: "Build for understanding" }, { id: ulid(12), type: "paragraph", markdown: "The product and its code should make the important relationships visible. Prefer cohesive systems, explicit ownership, and evidence over ceremony." }, { id: ulid(13), type: "callout", markdown: "Quality is a product feature. Slow down when a seam deserves a proper design." }] },
      { id: ulid(2), title: "EU release runbook", visibility: "team", version: 2, created_at: 1_760_000_100, updated_at: 1_760_002_000, blocks: [{ id: ulid(21), type: "heading", markdown: "Release gate" }, { id: ulid(22), type: "task_list", markdown: "CI, privacy checks, and rollback evidence are green." }] },
    ];
  }
  list({ cursor, limit }) { const ordered = [...this.pages].sort((a, b) => b.id.localeCompare(a.id)); const eligible = cursor ? ordered.filter((row) => row.id < cursor) : ordered; const items = eligible.slice(0, limit); return { items: items.map(summary), page: { next_cursor: eligible.length > items.length ? items.at(-1)?.id ?? null : null, limit } }; }
  get(id) { const row = this.pages.find((page) => page.id === id); return row ? document(row) : null; }
  bump(id) { const row = this.pages.find((page) => page.id === id); if (!row) return false; row.version += 1; row.updated_at += 1; return true; }
  create(body, idempotencyKey) {
    if (!exact(body, ["title", "template", "visibility"]) || !clean(body.title, 512) || body.title.trim() !== body.title || !["private", "team"].includes(body.visibility) || !operationId(idempotencyKey)) return { status: 400 };
    const template = blocksFor(body.template); if (!template) return { status: 400 };
    const existingId = this.nonces.get(idempotencyKey); if (existingId) return { status: 200, json: { page: this.get(existingId), created: false, durable: true } };
    const id = ulid(++this.sequence); const timestamp = 1_760_010_000 + this.sequence;
    const row = { id, title: body.title, visibility: body.visibility, version: 1, created_at: timestamp, updated_at: timestamp, blocks: template.map(([type, markdown]) => ({ id: ulid(++this.sequence), type, markdown })) };
    this.pages.push(row); this.nonces.set(idempotencyKey, id);
    return { status: 201, json: { page: document(row), created: true, durable: true } };
  }
  save(id, body) {
    const row = this.pages.find((page) => page.id === id); if (!row) return { status: 404 };
    if (!exact(body, ["expected_version", "title", "visibility", "blocks"]) || !Number.isSafeInteger(body.expected_version) || !clean(body.title, 512) || body.title.trim() !== body.title || !["private", "team"].includes(body.visibility) || !Array.isArray(body.blocks) || body.blocks.length < 1 || body.blocks.length > 500) return { status: 400 };
    if (body.expected_version !== row.version) return { status: 409 };
    const next = body.blocks.map((block) => {
      if (!block || typeof block !== "object" || Array.isArray(block) ||
          Object.keys(block).some((key) => !["id", "type", "markdown", "references", "state"].includes(key)) ||
          !["type", "markdown", "state"].every((key) => Object.hasOwn(block, key)) ||
          (block.id !== undefined && !/^[0-9A-HJKMNP-TV-Z]{26}$/.test(block.id)) ||
          !TYPES.includes(block.type) || !clean(block.markdown, 64 * 1024, true) ||
          !Array.isArray(block.references) || block.references.length > 32 ||
          block.references.some((reference) => typeof reference !== "string" || !reference.startsWith("myelin://")) ||
          [...block.markdown].filter((character) => character === "\uFFFC").length !== block.references.length ||
          block.state !== "active") return null;
      return { id: block.id ?? ulid(++this.sequence), type: block.type, markdown: block.markdown, references: block.references };
    });
    if (next.some((block) => block === null)) return { status: 400 };
    row.title = body.title; row.visibility = body.visibility; row.blocks = next; row.version += 1; row.updated_at += 1;
    return { status: 200, json: { page: document(row), version: row.version, durable: true } };
  }
}

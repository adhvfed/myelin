// Stateful Chat contract fixture for the browser harness. Shapes and bounds mirror chat_http.rs;
// authentication remains the dev Edge's responsibility.
const ULID_ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const AUTHOR = "chat-author:0123456789abcdef0123456789abcdef";
const OTHER_AUTHOR = "chat-author:fedcba9876543210fedcba9876543210";
const TENANT = "acme";
const PROJECT_ID = "20aee030-c7fa-4757-8243-700faf528690";

function encodeBase32(value, length) {
  let current = BigInt(value);
  let output = "";
  for (let index = 0; index < length; index += 1) {
    output = ULID_ALPHABET[Number(current & 31n)] + output;
    current >>= 5n;
  }
  return output;
}

function fixtureUlid(sequence) {
  return encodeBase32(1_750_000_000_000n + BigInt(sequence), 10) + encodeBase32(sequence, 16);
}

function exactObject(value, keys) {
  return value !== null && typeof value === "object" && !Array.isArray(value) &&
    Object.keys(value).length === keys.length && Object.keys(value).every((key) => keys.includes(key));
}

function cleanLabel(value) {
  return typeof value === "string" && value.length > 0 && value.length <= 255 &&
    value.trim() === value && !/\p{Cc}/u.test(value);
}

function canonicalProjectId(value) {
  return typeof value === "string" &&
    /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(value);
}

function cleanMessage(value) {
  return typeof value === "string" && value.trim().length > 0 &&
    Buffer.byteLength(value, "utf8") <= 32 * 1024 &&
    ![...value].some((character) => {
      const point = character.codePointAt(0);
      return point === 0 || point === 0x7f || (point < 0x20 && character !== "\n" && character !== "\t");
    });
}

function storableArtifactRef(value) {
  return typeof value === "string" && Buffer.byteLength(value, "utf8") <= 1024 &&
    /^myelin:\/\/[^/\s]+\/[^/\s]+\/[^/\s]+\/[^/#\s]+(?:#\S+)?$/.test(value);
}

function validIdempotencyKey(value) {
  return typeof value === "string" && value.length > 0 && value.length <= 128 &&
    [...value].every((character) => {
      const point = character.codePointAt(0);
      return point >= 0x21 && point <= 0x7e;
    });
}

function conversationJson(row) {
  return {
    id: row.id,
    ref: `myelin://${TENANT}/chat/channel/${row.id}`,
    project_id: row.project_id,
    channel: row.channel,
    topic: row.topic,
    linked_ref: null,
    pinned_canvas: null,
  };
}

function messageJson(row) {
  return {
    id: row.id,
    author: row.author,
    author_kind: row.author_kind,
    is_you: row.author === AUTHOR,
    content: row.content,
    nodes: row.nodes ?? [],
    edited: false,
    state: "active",
    created_at: row.created_at,
  };
}

export class ChatFixtures {
  constructor() {
    this.reset();
  }

  reset({ empty = false } = {}) {
    this.sequence = 20;
    this.conversations = empty ? [] : [
      { id: fixtureUlid(1), project_id: PROJECT_ID, channel: "engineering", topic: "release readiness" },
      { id: fixtureUlid(2), project_id: PROJECT_ID, channel: "engineering", topic: "agent operations" },
      { id: fixtureUlid(3), project_id: PROJECT_ID, channel: "product", topic: "customer feedback" },
    ];
    this.messages = new Map();
    if (!empty) {
      this.messages.set(fixtureUlid(1), [
        {
          id: fixtureUlid(10),
          author: OTHER_AUTHOR,
          author_kind: "human",
          content: "The canary is healthy. Track the rollout in \uFFFC so the decision stays with the work.",
          nodes: [{ kind: "artifact_ref", ref: `myelin://${TENANT}/issue/issue/MYL-204` }],
          created_at: 1_750_000_000,
          client_nonce: "seed-1",
        },
        {
          id: fixtureUlid(11),
          author: AUTHOR,
          author_kind: "human",
          content: "Great. Let’s hold at 25% until the EU latency window closes, then continue.",
          created_at: 1_750_000_060,
          client_nonce: "seed-2",
        },
        {
          id: fixtureUlid(12),
          author: OTHER_AUTHOR,
          author_kind: "agent",
          content: "I’m watching error rate and p95 latency. Both remain inside the release gate.",
          created_at: 1_750_000_120,
          client_nonce: "seed-3",
        },
      ]);
    }
  }

  listConversations({ cursor, limit }) {
    const ordered = [...this.conversations].sort((left, right) => right.id.localeCompare(left.id));
    const eligible = cursor ? ordered.filter((row) => row.id < cursor) : ordered;
    const items = eligible.slice(0, limit);
    return {
      items: items.map(conversationJson),
      page: {
        next_cursor: eligible.length > items.length ? items.at(-1)?.id ?? null : null,
        limit,
      },
    };
  }

  createConversation(body, clientNonce) {
    if (!exactObject(body, ["project_id", "channel", "topic"]) ||
        !canonicalProjectId(body.project_id) ||
        !cleanLabel(body.channel) || !cleanLabel(body.topic) ||
        !validIdempotencyKey(clientNonce)) return { status: 400 };
    const retry = this.conversations.find((row) => row.client_nonce === clientNonce);
    if (retry) {
      if (retry.project_id !== body.project_id || retry.channel !== body.channel ||
          retry.topic !== body.topic) return { status: 409 };
      return {
        status: 200,
        json: { conversation: conversationJson(retry), durable: true },
      };
    }
    const existing = this.conversations.find((row) => row.project_id === body.project_id &&
      row.channel === body.channel && row.topic === body.topic);
    if (existing) {
      return {
        status: 200,
        json: { conversation: conversationJson(existing), durable: true },
      };
    }
    const row = {
      id: fixtureUlid(++this.sequence),
      project_id: body.project_id,
      channel: body.channel,
      topic: body.topic,
      client_nonce: clientNonce,
    };
    this.conversations.push(row);
    this.messages.set(row.id, []);
    return {
      status: 201,
      json: { conversation: conversationJson(row), durable: true },
    };
  }

  listMessages(conversationId, { before, limit }) {
    const conversation = this.conversations.find((row) => row.id === conversationId);
    if (!conversation) return null;
    const ordered = [...(this.messages.get(conversationId) ?? [])]
      .sort((left, right) => left.id.localeCompare(right.id));
    const eligible = before ? ordered.filter((row) => row.id < before) : ordered;
    const items = eligible.slice(Math.max(0, eligible.length - limit));
    return {
      conversation: conversationJson(conversation),
      items: items.map(messageJson),
      page: {
        next_cursor: eligible.length > items.length ? items[0]?.id ?? null : null,
        limit,
      },
    };
  }

  postMessage(conversationId, body, idempotencyKey) {
    const bodyShape = exactObject(body, ["content"]) || exactObject(body, ["content", "references"]);
    const references = body?.references ?? [];
    if (!bodyShape || !cleanMessage(body.content) || !Array.isArray(references) ||
        references.length > 32 || !references.every(storableArtifactRef) ||
        [...body.content].filter((character) => character === "\uFFFC").length !== references.length ||
        !validIdempotencyKey(idempotencyKey)) return { status: 400 };
    if (!this.conversations.some((row) => row.id === conversationId)) return { status: 404 };
    const rows = this.messages.get(conversationId) ?? [];
    const existing = rows.find((row) => row.client_nonce === idempotencyKey);
    if (existing) return { status: 201, json: { message_id: existing.id, durable: true } };
    const row = {
      id: fixtureUlid(++this.sequence),
      author: AUTHOR,
      author_kind: "human",
      content: body.content,
      nodes: references.map((ref) => ({ kind: "artifact_ref", ref })),
      created_at: 1_750_001_000 + this.sequence,
      client_nonce: idempotencyKey,
    };
    rows.push(row);
    this.messages.set(conversationId, rows);
    return { status: 201, json: { message_id: row.id, durable: true } };
  }
}

export function parseChatQuery(raw, cursorName) {
  const values = new URLSearchParams(raw);
  if ([...values.keys()].some((key) => !["limit", cursorName].includes(key)) ||
      [...values.keys()].some((key) => values.getAll(key).length !== 1)) return null;
  const rawLimit = values.get("limit");
  const limit = rawLimit === null ? 50 : Number(rawLimit);
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 100 ||
      (rawLimit !== null && String(limit) !== rawLimit)) return null;
  const cursor = values.get(cursorName) ?? undefined;
  if (cursor !== undefined && !/^[0-9A-HJKMNP-TV-Z]{26}$/.test(cursor)) return null;
  return { limit, ...(cursor ? { [cursorName]: cursor } : {}) };
}

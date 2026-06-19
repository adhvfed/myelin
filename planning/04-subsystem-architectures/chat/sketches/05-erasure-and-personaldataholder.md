# Sketch 05 — Erasure mechanism + `PersonalDataHolder` (Chat is the hardest holder)

> Exploration note. Resolves Phase-2 Chat §9.5 / Phase-1 §7. Chat is "the most PII-dense holder — free-
> text bodies *about other people*" and "the stress test for the holder spine" (Phase-2 Chat §8.5).
> Grounds every choice in the platform's already-decided erasure triad (Bus §4.8, Storage §5.1).

---

## The problem, precisely

Chat message bodies are **pervasive, unstructured free-text personal data, often about *other* people**,
replicated into many derived stores. GDPR Art. 17 erasure of a user must remove *their* personal data,
but their messages are part of *others'* conversations (Phase-1 §7). The platform's decided answer is the
**references-not-payloads + crypto-shred + tombstone triad** (EI-04 §1; Bus §4.8; Storage §5.1) — *delete
the identity, not the fact*. Chat's job is to apply that triad correctly across **the hardest holder
surface in the platform**, and to enumerate exactly which Chat stores hold what.

The crucial honesty: **chat bodies are NOT references-not-payloads** — a message body is *itself* the
personal data (unlike an event envelope that points elsewhere). So chat **leans hard on crypto-shred**
(per-subject DEK key destruction), not just pseudonym indirection. This is the GD-4 "free-text /
chat-body / agent-memory = per-subject DEK" rule (Storage §5.1) — chat is the canonical case for it.

---

## The two erasure subjects (kept distinct)

GDPR erasure of a person P touches chat in **two different roles**, with different mechanisms:

### Role 1 — P is the *author* of messages (their own content)

- **Mechanism: crypto-shred the body + tombstone the record.** Message bodies are **envelope-encrypted
  with a per-subject DEK** (the author's key; Storage §5.1 GD-4). `erase(P)` = **destroy P's DEK** →
  every body P authored becomes unrecoverable ciphertext **in the hot store, the cold tier, AND backups**
  simultaneously (the crypto-shred property; Boneh & Lipton 1996; NIST SP 800-88r1) — *without rewriting
  the immutable per-conversation log*. The message *record* survives as a **tombstone** ("message
  deleted") so the conversation's structure/ordering/causality stays intact for the other participants
  and for audit (Phase-1 §7; Bus §4.8). This is "delete the content, keep the fact-of-a-message."
- **Why per-subject DEK, not per-tenant:** a per-tenant key would force erasing P to destroy *everyone's*
  bodies. Per-subject DEK is exactly GD-4's "free-text/chat-body = per-subject DEK" granularity rule.

### Role 2 — P is *mentioned / referenced* in *others'* messages

- **Mechanism: structured-node neutralisation, made tractable by ADR-05.** A `mention(Principal)` is a
  **structured node with a stable opaque `principal_id`** (ADR-05; Phase-1 §2.4) — *not* free text. The
  mention already points at P's **pseudonymous principal id**, never inline PII. So erasing P needs **no
  message mutation in the common case**: Id's pseudonym-map shred (identity §11) makes the id
  unresolvable, and the mention **renders to a tombstone** (`[erased user]`) on next render — the same
  references-not-payloads lever Refs §4.6 and Notif §3.9 use. The mention being *structured* is the whole
  reason this is tractable; free-text "@alice" in a body would not be (Phase-1 §7). 
- **The residual hard case (named honestly):** P's name typed into the *free-text body* of someone else's
  message ("I talked to Alice Smith about X"). This is **not** a structured node and **cannot** be
  surgically neutralised without content analysis. The honest posture (Phase-1 §7): it is covered by the
  *author's* crypto-shred only if the author is also erased — otherwise it falls under **retention +
  access-control + the documented lawful-basis limit**, the same residual EI-04 §1 names for free text.
  We do **not** pretend free-text third-party mentions are perfectly erasable; we name the floor. (This is
  the analogue of GD-1's git-history residual, scoped to chat free text.)

---

## The cascade: erasure must reach every Chat-owned derived store (the holder enumeration)

Erasing P must cascade to **all** Chat-held stores (Phase-1 §7; Phase-2 Chat §7.8). The `PersonalDataHolder`
auto-registration (substrate §3.4) makes "we forgot store X" structurally impossible — every store the
harness opens is registered. Chat's holder enumeration:

| Chat store | Holds | Erasure mechanism |
|---|---|---|
| **Durable message log** (Sketch 02) | bodies (per-subject-DEK encrypted), mention nodes (pseudonymous), tombstones | crypto-shred P's DEK (author role) + pseudonym-shred (mention role); tombstone the record |
| **Object/cold tier + backups** | sealed message segments | crypto-shred (key destruction reaches cold + backups for free — the whole point) |
| **Unfurl projection cache** (Sketch 04) | short-TTL projection snapshots (may hold a name in a title) | **purge cache entries** containing P; they re-resolve live → tombstone (and we store *no* durable unfurl snapshot, Sketch 04 — so this surface is small by design) |
| **Read-state store** (Sketch 03) | P's last-read markers (P's own data) | delete P's markers (Valkey + PG record) |
| **Membership / channel metadata** (PG) | P's channel memberships, prefs, pins/bookmarks/drafts (drafts are PII — Phase-1 §2.7) | delete P's rows; **drafts crypto-shred** (P-authored free text) |
| **Search index** (Search-owned, but Chat must trigger) | indexed message terms | Search **purges + reindexes** (incl. embeddings) on the erasure event (Search holder; T-5 erasure-reaches-search) |
| **Refs edges** ("discussed in #channel") | pseudonymous `origin_actor` | Refs pseudonym-shred (Refs §4.6) — no Chat action beyond the event |
| **Notif inbox items** | chat-originated items referencing P | Notif references-not-payloads → tombstone (Notif §3.9) |

Chat **emits the erasure trigger** (`identity.human.erased` consumed → cascade) and **implements
`locate/export/rectify/restrict/erase`** over its own stores; the cross-store cascade rides the DSR
orchestrator fan-out (GDPR §4) + the bus, never a Chat-private backdoor.

---

## `PersonalDataHolder` shape (illustrative)

```rust
impl PersonalDataHolder for Chat {
  fn locate(subject)  -> messages authored-by | mentioning subject; channel memberships; read-state;
                         drafts/pins/bookmarks; unfurl-cache entries naming subject;
  fn export(subject)  -> subject's messages (decrypted with their DEK), mentions OF them, DMs, reactions,
                         memberships — the Art. 15/20 DSR bundle (refs resolved via owners);
  fn rectify(subject) -> profile rectification is Id's; chat stores no rectifiable profile copy (refs);
  fn restrict(subject)-> stop indexing/agent-use/notification/new-routing for the restricted subject
                         (the platform restriction flag — README §5 obligation);
  fn erase(subject)   -> crypto-shred subject's per-subject DEK (authored bodies + drafts) → unrecoverable
                         in hot/cold/backups; tombstone the records; pseudonym-shred handles mentions;
                         purge unfurl-cache + read-state rows; cascade to Search/Refs/Notif via the bus.
}
```

- **Live unfurls + ephemeral caches favoured over durable snapshots** *precisely* so a later-erased third
  party isn't frozen in a card (Phase-1 §7; Sketch 04) — the erasure design and the unfurl design are the
  same decision viewed twice.
- **Per-channel/per-org retention** (auto-delete after N days) is a *bulk* erasure path that must purge
  from all derived stores too (Phase-1 §7); it rides the same cascade. Retention is tightest-policy-wins +
  legal-hold-aware (GD-2; GDPR §5) — Chat owns the *policy hook*, GDPR owns the engine.
- **Audit of edits/deletes/agent-processing** is the tamper-evident log (GDPR §6), distinct from chat
  history; "an agent reading a channel is processing personal data" and is audited (Phase-1 §7; lawful-
  basis/provenance on every message, esp. agent-authored — Art. 30 RoPA).

---

## What this sketch hands forward (decided directions)

- **Crypto-shred per-subject DEK for bodies + drafts** (GD-4; the chat-specific lean, because bodies *are*
  the PII, not references) + **tombstone the record** (keep the fact, delete the content).
- **Structured `mention(Principal)` neutralisation** via pseudonym-shred (free because the node is
  structured + pseudonymous) — the ADR-05 payoff.
- **Named floor (honesty):** free-text third-party names in *others'* un-erased bodies are **not**
  surgically erasable — covered by retention + access-control + a documented lawful-basis limit (the
  chat analogue of GD-1's residual). Stated as a floor, not as solved. → P4 + LEGAL (GD-1/GD-2 family).
- **Holder auto-registration enumerates every Chat store** so the cascade can't miss one; erasure reaches
  Search/Refs/Notif via the bus + DSR fan-out, never a backdoor.
- Owed drills: **erasure-reaches-every-Chat-holder** (incl. cold tier + backups via crypto-shred) and
  **erased-mention-renders-tombstone** (T-5 erasure family).

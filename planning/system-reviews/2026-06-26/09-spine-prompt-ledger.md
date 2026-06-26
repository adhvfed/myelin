# Make-It-Real Ledger — Phase 2: the spine

Date: 2026-06-26. Status: PLAN (Phase 2 = right-size the work into prompts). This decomposes the **spine**
(roadmap E0.1–E0.9) into batch-runner-ready, ~400k–700k-execution prompts. The subsystem tracks (Git → Actions →
chat → issues → docs) are decomposed after the spine; each pulls its own Tier-2 components + API + UI + CLI/MCP
surface in the same style.

## Conventions

- **ID:** `MR-NNN` (Make-It-Real). A new series so it never collides with the planning-only M7 band (P-522–546),
  which this *executes*.
- **Type:** **NET-NEW** (body authored fresh) · **REUSE P-NNN** (execute the existing M7 prompt body in
  `planning/07-prompts/by-system/production-readiness.md`, reconciled to current code).
- **Anti-duplication, binding on every prompt (step 1):** grep `planning/07-prompts/` + the crates + the
  design-manual spec for an existing implementation; **cross-check the ledger's claim against the actual
  commit/files** (`git log --grep`, `git show --stat`) — the claim says what to *reuse*, the files say what's
  *built*, the gap is what to build. Extend/reconcile, never fork.
- **Sizing:** each prompt targets ~400k–700k execution tokens (canon/specs read + code read + impl + verify),
  **never above 700k.** The size note flags split/merge risk; flagged splits are confirmed at authoring time.
- **Gate:** the batch runner runs these one at a time, cargo+frontend gate between each, halt on red.
- **Frontend canon (binding on every UI prompt's CANON DOCS):** the stack is **SolidJS + SolidStart + Tauri 2**
  with **hand-built primitives** — *not* React. Every UI/frontend prompt body MUST cite, and the agent MUST read
  IN FULL FIRST: `08-frontend-foundation.md` (stack), `10-frontend-component-patterns.md` (component behaviour),
  and the relevant `design-planning/08-design-system/` spec (tokens/icons/component-spec/a11y). The design
  manual's named React+React-Aria stack is SUPERSEDED by docs 08+10 (a banner says so at both entry points).

## Dependency waves (the batch runs them linearly in this order; waves show what *could* parallelize)

- **W1 (no deps):** MR-001/002 census · MR-004 absence scanners · MR-016 frontend package/guide/lint · (MR-006 shape review after census)
- **W2:** MR-003 census synthesis · MR-005 evidence gate · **MR-022 persistence foundation** · MR-007/008 durable persistence · **MR-023 events persistence · MR-024 control-plane persistence · MR-025 KMS durable root** · MR-017 overlays · MR-018 Tauri skeleton
- **W3:** MR-009 persistence verify · MR-010/011 auth crypto
- **W4:** MR-012 remove Structural + scanner · MR-013 tenant isolation
- **W5:** MR-014/015 product API
- **W6:** MR-019 app shell · MR-020 CLI core · MR-021 MCP server

> **Revision 1 (2026-06-26, post-census).** The MR-003 synthesis (`shortcut-inventory.md`) found the spine's
> durable-persistence coverage (MR-007/008) was **identity-crate only**, leaving five CRITICAL load-bearing
> substrate organs with no spine prompt. The destination is unchanged (the master plan already requires "nothing
> is real while load-bearing state lives in a `HashMap`"); this is the authoring-time split the ledger anticipated.
> Inserted prompts **MR-022..MR-025** (IDs stable; run order is the wave list above + the per-prompt Deps, not the
> ID sequence). The persistence band now runs **MR-022 (foundation) → MR-007 → MR-008 → MR-023 → MR-024 → MR-025
> → MR-009 (verify, scope expanded to all four store families)**. Git ref-store durability, the git server binary,
> and real backup/restore (SI-012/013/014/015) remain correctly routed to the **Git subsystem track** (decomposed
> after the spine), not the spine — flagged in `shortcut-inventory.md` §B so they aren't lost.

### Inserted persistence prompts (Revision 1)

| ID | Epic | Title | Type | Deps | Size note |
|---|---|---|---|---|---|
| MR-022 | E0.3 | **Persistence foundation:** real migration-runner DDL execution against the live pool + the composition root that injects a real Postgres/Valkey pool into the substrate + **flip the real backings onto the DEFAULT test gate** (off `--features integration`, fixes SI-022) | NET-NEW | MR-003 | **prerequisite for all persistence**; ~mid–high. Fixes SI-010. |
| MR-023 | E0.3 | **Events bus durable persistence:** outbox + dedup ledger + relay delivery on the live pool/bus + the events `serve()` composition root | REUSE P-522/523 + P-539 | MR-022 | ~mid–high. Fixes SI-007/008/009/023/024/025/037. May split outbox vs serve. |
| MR-024 | E0.3 | **Control-plane placement registry durable persistence:** tenant→cell routing survives restart | REUSE P-522/523 | MR-022 | ~mid. Fixes SI-011/026/027/028. |
| MR-025 | E0.3 | **KMS durable cell-root + KEK persistence slice** (software-sealed, *not* HSM — HSM stays Tier-4): the L0 root + L1 KEKs survive restart so encrypted columns are recoverable; `backup_snapshot` includes them | REUSE P-522 (KMS slice) | MR-022 | ~mid. Fixes SI-006; without it MR-009's restart verify is hollow. |

## The spine prompt set

| ID | Epic | Title | Type | Deps | Size note |
|---|---|---|---|---|---|
| MR-001 | E0.1 | Census: shared-substrate surfaces (identity, storage, events, control-plane, tenancy) — adversarial read → findings | NET-NEW | — | read-heavy; ~mid |
| MR-002 | E0.1 | Census: Git surfaces (myelin-git + the ci-sandbox seam) — adversarial read → findings | NET-NEW | — | ~mid |
| MR-003 | E0.1 | Census synthesis: ranked `shortcut-inventory.md` (spine+Git) + the duplicate-risk map | NET-NEW | MR-001, MR-002 | analysis; ~low–mid |
| MR-004 | E0.2 | Production-graph **absence scanners** (extend `myelin-lints`): no `Structural*`, no in-memory durable store, no bare tenant pool — **each with a red fixture** | NET-NEW | — | ~mid |
| MR-005 | E0.2 | **Attested scorecards + red-by-default gate** binary skeleton (the internal P-540/541 evidence spine) | NET-NEW | MR-004 | ~mid |
| MR-006 | E0.4 | Shape/design review: identity/authz, sandbox, KMS, tenancy, GDPR **+ the mock→`LlmAgentRuntime` seam** — "is the frozen shape still right" → reshape findings | NET-NEW | MR-003 | review; ~mid (may spawn reshape prompts) |
| MR-007 | E0.3 | Durable persistence impl: bind the live OLTP/cache pool under the **principal + tuple** stores | REUSE P-522 | MR-003 | **split of P-522**; ~mid–high |
| MR-008 | E0.3 | Durable persistence impl: the **revocation + expiry** stores on the live pool | REUSE P-522 | MR-007 | second half of P-522; ~mid |
| MR-009 | E0.3 | Durable persistence **verify** (scope expanded, Rev 1): kill-9/restart + 3-instance consistency + the no-in-memory scanner green — across **all four store families** (identity, events, control-plane, KMS root) | REUSE P-523 | MR-007, MR-008, MR-023, MR-024, MR-025, MR-004 | integration; ~mid–high |
| MR-010 | E0.5 | Auth: **human/SSO** real crypto (OIDC JWKS / SAML XML-DSig / WebAuthn / SSH) + the forged/expired/replayed negative corpus | REUSE P-526 | MR-009 | **likely splits per credential type**; ~high |
| MR-011 | E0.5 | Auth: **machine/capability tokens + DPoP** (signed, attenuated, sender-constrained, revocable) + negative corpus | REUSE P-527 | MR-009 | ~mid–high |
| MR-012 | E0.5 | **Remove the `Structural*` verifiers/signers** from the production graph; the absence scanner (MR-004) goes green-on-prod, red-on-fixture | REUSE P-528 | MR-010, MR-011, MR-004 | ~mid |
| MR-013 | E0.5 | Tenant isolation: **`SET LOCAL` RLS + reset-on-release** (fixes `set_config(...,false)` bleed) + identifier allowlist + mTLS/region fail-fast | REUSE P-531 | MR-009 | ~mid |
| MR-014 | E0.6 | Product/edge API: the **edge gateway design** + auth integration (E0.5 tokens) + the API conventions every subsystem follows | NET-NEW | MR-012, MR-013 | design+impl; ~mid–high |
| MR-015 | E0.6 | Product/edge API: **wire the existing `serve(AppSpec)` shells through the edge**; the Git API contract first (reuse git `api.rs` grammar) | NET-NEW | MR-014 | ~mid |
| MR-016 | E0.7 | Frontend: the **Solid design-system package** (Tier 0 tokens/icons/styleguide wired) + the **"Solid patterns for agents" guide** (seeded from `10-frontend-component-patterns.md`) + the **frontend lint** (eslint-plugin-solid/jsx-a11y/axe) | NET-NEW | — | ~mid |
| MR-017 | E0.7 | Frontend: the **hand-built Tier-1 overlay primitives** (Dialog/Confirm/Popover/Dropdown/Tooltip/Toast — focus-trap/portal/scroll-lock/z-index/ARIA once, per doc 10 §1) gated by axe+keyboard | NET-NEW | MR-016 | ~mid–high |
| MR-018 | E0.7 | Frontend: the **Tauri 2 shell skeleton** (desktop + mobile) sharing the Rust core (`myelin-content`/`myelin-client`); **validate the mobile target early** | NET-NEW | MR-016 | ~mid (mobile risk) |
| MR-019 | E0.7/E0.8 | Frontend: the **SolidStart app shell** (nav/routing/auth-session/layout/⌘K trigger/inbox/identity menu/residency cue) + the **Playwright+axe** harness | NET-NEW | MR-015, MR-017 | ~mid–high; **may split shell / test-harness** |
| MR-020 | E0.9 | **CLI:** the `myelin` CLI core (clap binary + auth + command framework) — reuse the git/notif command grammars | NET-NEW | MR-015 | ~mid |
| MR-021 | E0.9 | **MCP server:** the skeleton + tool-registration framework so a local Claude drives Myelin under the same auth+audit as a human | NET-NEW | MR-015, MR-020 | ~mid |

> **Revision 2 (2026-06-26, authoring-time split of MR-010).** As flagged, MR-010 (human/SSO real crypto)
> splits per credential type — four distinct security-critical protocols, each with its own forged/expired/
> replayed negative corpus, each independently verified: **MR-010a OIDC JWKS** (JWT sig verify, alg-confusion
> defence), **MR-010b SAML 2.0 XML-DSig**, **MR-010c WebAuthn/FIDO2 attestation**, **MR-010d SSH pubkey
> challenge-response**. All swap in behind the existing `CredentialVerifier` seam (`authenticate.rs`, MR-006
> confirmed harden-as-is). MR-011 (machine/DPoP) also discharges the carried-forward `CapabilityAuthenticator`
> →durable-`RevocationStore` wiring (the S7Denylist gap from MR-008).

~21 spine prompts. Flagged for likely split at authoring: **MR-010** (per credential type) and possibly **MR-007**
(if principal+tuple alone exceeds the window) and **MR-019** (shell vs. test-harness). MR-006 may *emit* reshape
prompts depending on what the shape review finds.

## What each NET-NEW prompt delivers (the scope a full body will expand)

- **MR-001/002 (census):** per assigned crate/surface, adversarially read against the contracts/prompts it claims
  to satisfy; report `file:symbol`, the claimed-vs-built gap, whether the existing test would pass on a stub, and
  a blast-radius severity. No code change.
- **MR-003 (synthesis):** dedup + rank into `planning/system-reviews/2026-06-26/shortcut-inventory.md`; flag
  duplicate-risk surfaces for the build prompts.
- **MR-004 (scanners):** extend `myelin-lints` with the three production-graph absence checks, **each shipping a
  red fixture** that proves the scanner bites; wire into the gate.
- **MR-005 (gate):** generated, attested scorecards (hash + command + date) + a red-by-default `make-it-real` gate
  binary that reads them; a tamper fixture proves it fails closed.
- **MR-006 (shape review):** the design-soundness + "frozen-shape-still-right" pass; output is findings + any
  reshape prompts (e.g. if the agent-runtime seam or a single→multi-cell shape needs redrawing before hardening).
- **MR-014/015 (product API):** define the one edge the UI/CLI/MCP call (auth, versioning, error model, the
  view-model/data contract), then expose the existing service shells through it — **reconcile, don't reinvent**;
  git's `web.rs` view-models + `api.rs` grammar are the first thing exposed.
- **MR-016 (frontend package):** the Solid monorepo package consuming Tier-0 tokens/icons; the agent-patterns
  guide and the lint gate are first-class deliverables (the Solid-fluency mitigation).
- **MR-017 (overlays):** the six Tier-1 primitives on Kobalte, validated for coverage — the substrate every later
  component inherits.
- **MR-018 (Tauri):** the desktop + mobile shell wrapping the (soon-to-exist) web app, with the Rust-core bridge;
  prove a "hello, shared `myelin-content`" round-trips through the Tauri Rust side on desktop **and** a mobile
  target.
- **MR-019 (app shell):** the running SolidStart shell with real auth-session + the nav/command-palette/inbox/
  identity chrome; the Playwright+axe harness that **re-platforms the switch-test onto a real browser**.
- **MR-020/021 (CLI/MCP):** the `myelin` binary and the MCP server, both calling the product API under real
  auth+audit — the near-term "agents, but driven locally" path.

> **Frontend note:** the hard components (overlays, command palette, block editor, real-time, data, app
> shell) have a known-good behavioural approach in `10-frontend-component-patterns.md` — minimal deps,
> hand-built primitives, per-block contenteditable, SolidStart-native data + a server cookie-auth gateway,
> SSE. The subsystem UI prompts implement these fresh against the design tokens and harden the named edges.

## After the spine

Each subsystem track (Git first) is decomposed the same way: `[backend HARDEN+RECONCILE]` + `[expose via the
product API]` + `[the Tier-2 components it needs, built just-ahead-of-use, starting with Git's set]` + `[its web
UI]` + `[its CLI/MCP surface]` + `[its external-oracle test]`. CI/sandbox (P-544/545) is its own long-pole track.
Phase-2 decomposition of the Git track follows once the spine sequence is agreed.

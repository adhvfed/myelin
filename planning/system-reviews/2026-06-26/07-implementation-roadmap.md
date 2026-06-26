# Make It Real — Implementation Roadmap (Phase 1: sequence the work)

Date: 2026-06-26. Status: PLAN, Phase 1 of two.
- **Phase 1 (this doc):** sequence the work — the epics, in build order, grounded in what already exists.
- **Phase 2 (next):** right-size each epic into 400k–700k-execution prompts for the gated batch runner.

---

## Grounding: what the survey of the 521-prompt ledger + the 31 crates actually shows

The cardinal rule (founder's): **duplicate surface area is the poison.** So every epic below is tagged
**HARDEN** (exists, stubbed → make real), **REUSE/EXPOSE** (exists → wire it up, don't rebuild), or **NET-NEW**
(build it — but thin over existing logic).

**The single most important finding:** the build is **backend/contract-complete and interaction-surface-empty —
and the interaction surface "exists" in the ledger as Rust shape/logic/harnesses, not as runnable software.**
- Every subsystem backend + the shared substrate **EXIST** (31 crates), with the load-bearing organs **stubbed**
  (auth-token crypto, KMS, durable identity stores, sandbox production exec, durable persistence).
- The UI "exists" as Rust **view-models, ViewSpec definitions, the `myelin-content` render path, and "switch
  test" harnesses that *model* browser-driven behaviour** (e.g. chat's switch test is a Rust `#[test]`, not a
  browser test). There is **no runnable frontend**: no `package.json`, no React/`.tsx` anywhere outside the design
  *specs*; `myelin-git/src/web` is a test module.
- "CLI" exists only as a few Rust command modules (notif list/show/read/prefs). There is **no installable
  `myelin` CLI binary.**
- There is **no MCP server** (only named as a future seam in P-481).
- A coherent **product/edge API** for a UI/CLI/MCP to call is **partial**: services boot internal shells via the
  harness `serve(AppSpec)`, but there is no unified product API surface.
- The **design system** exists as **specs** (`design-planning/08-design-system/`: tokens/components/styleguide/
  icons), not as a built component library.

**So "make it real" = (A) harden the stubbed backend organs + (B) build the *runnable* interaction surface —
product API, web UI, app, CLI, MCP — as a THIN layer that REUSES the existing Rust view-models, render paths,
command logic, and design-system specs.** B is net-new *as running software*; it must reuse the existing logic,
never reinvent it.

### The anti-duplication discipline (binding on every Phase-2 prompt)
Step 1 of every prompt: grep `planning/07-prompts/` + the crates for an existing implementation of the thing about
to be built, and **extend/reconcile it — never fork it.** The ledger is the authoritative surface map; read the
relevant rows before writing a line.

---

## The grounded inventory

| Component | Exists? | State | What's needed |
|---|---|---|---|
| Shared substrate (identity, events, storage, refs, search, notif, flow, gdpr, tenancy, control-plane, query, content) | Yes | organs stubbed | **HARDEN** |
| Subsystem backends (git, ci-controlplane/dispatch/sandbox, chat/chat-gateway, issues, knowledge) | Yes | organs stubbed | **HARDEN + RECONCILE** |
| UI logic (ViewSpec, render paths, `myelin-content`, switch-test harnesses) | Yes (Rust) | models, not runnable | **REUSE** as the UI data/logic layer |
| Design system | Yes (specs only) | `design-planning/08-design-system/` | **NET-NEW** component library from specs |
| Product/edge API | Partial | internal `serve(AppSpec)` shells | **RECONCILE/EXPOSE** as one product API |
| Web UI + app | No | — | **NET-NEW** (runnable) over existing logic |
| CLI (`myelin`) | No (only command modules) | — | **NET-NEW** binary over existing logic |
| MCP server | No (named only) | — | **NET-NEW** |
| Agent fabric | Yes | mock runtime | **DEFERRED** (cost) |

---

## The sequenced roadmap (epics in build order)

### Phase 0 — The spine (gates every subsystem track)
- **E0.1 Canonical census & inventory.** Read the ledger in full + map every crate; produce the authoritative
  "exists / stubbed / duplicate-risk" map. The anti-duplication foundation; also the confidence-campaign Stage 0.
- **E0.2 Evidence-integrity skeleton.** Red-fixtured production-graph absence scanners, attested scorecards, a
  red-by-default gate. So nothing below can green-lie.
- **E0.3 Durable persistence (P-522/P-523).** The universal floor — nothing is real while load-bearing state is a
  `HashMap`. Bind the live OLTP/cache pool; prove crash/restart keeps state.
- **E0.4 Shape/design review.** Incl. the mock→agent-runtime seam; "is the frozen shape still right" per
  security-critical subsystem (campaign Stage D).
- **E0.5 Real auth/token crypto (P-526/P-527/P-528) + tenant isolation (P-531).** Required before any exposed
  surface; removes the `Structural*` verifiers from the prod graph; fixes the `set_config(..., false)` bleed.
- **E0.6 The product/edge API surface.** RECONCILE the existing `serve(AppSpec)` service shells into one coherent
  API the UI/CLI/MCP call. Expose existing contracts; do not invent a parallel API.
- **E0.7 Design system → real component library.** Implement the `design-planning/08-design-system/` specs as the
  actual components (React + React-Aria + Style Dictionary). The UI foundation.
- **E0.8 The UI shell.** Runnable web-app skeleton (routing, auth, layout) on the component library.
- **E0.9 The CLI + MCP substrate.** The `myelin` CLI core + MCP server skeleton + the auth/command/tool framework
  each subsystem plugs into — reusing the existing Rust command logic, not re-deriving it.

### Phase 1 — Git (priority #1; the first real daily driver)
- **E1.1** Git backend HARDEN + RECONCILE — durable storage, real *destructive* backup/restore of your repos.
- **E1.2** Git product API — expose the existing git backend through E0.6.
- **E1.3** Git web UI — repo/file browse, commits, diffs, PRs, review (reuse ViewSpec/render; real components).
- **E1.4** Git CLI + MCP surface — clone/push/PR/review as CLI commands + MCP tools.
- **Oracle:** real `git` clone/push/fetch + `git fsck` against your repos.

### Phase 2 — Actions / CI (priority #2; the long pole — its own track, no rush, get it right)
- **E2.1** Sandbox production exec (P-544) — real `JobSpec.command` through hardened `launch()` on both backends.
- **E2.2** Production-path escape verification (P-545) — AG-D4 corpus through the prod path, 0 escapes.
- **E2.3** CI backend HARDEN + RECONCILE (ci-controlplane / dispatch / sandbox).
- **E2.4** CI API + UI (pipelines, runs, live log tail) + CLI/MCP.
- **E2.5** Cut over from GitHub Actions — only after the sandbox is genuinely hardened.

### Phase 3 — Team chat (priority #3)
- Backend HARDEN + RECONCILE + real-time delivery; API; web UI + app; CLI/MCP. Reuse the composer + `myelin-content`.

### Phase 4 — Issue tracker (priority #4)
- Backend HARDEN + RECONCILE; API; web UI (board/table/roadmap/etc. — reuse the existing ViewSpec views); CLI/MCP.

### Phase 5 — Docs / knowledge base (priority #5)
- Backend HARDEN + RECONCILE; the editor (reuse the Knowledge render/editor logic + `myelin-content`); API; web UI; CLI/MCP.

### Cross-cutting (continuous, every phase)
- Secret handling (P-532/P-533), runtime lifecycle + OTel (P-539), the evidence gate, external-oracle tests, the
  ongoing design/shape review.

### Graduation (staged for when users arrive — on the roadmap)
- HSM-KMS (P-524/P-525), supply-chain governance (P-534–538), independent crypto+sandbox review + pentest
  (P-542/P-543), the fail-closed release gate (P-546), sovereignty certifications.

---

## Critical path & parallelism

- The **spine (E0.\*)** gates every subsystem track — especially E0.3 (persistence), E0.5 (auth), E0.6 (API),
  E0.7/E0.8 (UI foundation), E0.9 (CLI/MCP).
- **Git** is the first usable daily driver; it unblocks the dogfood.
- **CI** is the long pole — runs as its own parallel track from the moment Git is moving. No rush; get the sandbox
  right before trusting it with your supply chain.
- The **UI is the largest net-new effort.** Once the component library + shell exist (E0.7/E0.8), each subsystem's
  UI parallelizes. The **app** follows the web UI per subsystem.
- The **CLI/MCP** surfaces advance per-subsystem alongside that subsystem's API.

## Honest scope note

The interaction surface (API + UI + app + CLI + MCP) is comparable in effort to the backend hardening — possibly
larger, because it is net-new *runnable* software, even though it reuses existing Rust logic. The 521 prompts
built a substrate, not a usable product; this roadmap is roughly "harden the substrate + build the entire
front-of-house on top of it." That is the honest size of "make it real," and it is why *no rush* is the right
call.

## What Phase 2 produces

Each epic → a dependency-ordered set of 400k–700k-execution prompts, each opening with the anti-duplication grep,
delivered as a batch-runner-ready ledger. Phase 2 starts once this sequence is agreed.

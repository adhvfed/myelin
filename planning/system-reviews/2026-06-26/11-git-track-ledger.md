# Make-It-Real Ledger — Phase 3: the Git subsystem track (E1.1–E1.4)

Date: 2026-06-29. Status: PLAN. The spine (MR-001..025) is complete; this decomposes the **Git track** —
"Git as a real daily driver" (roadmap Phase 1, priority #1) — into batch-runner-ready prompts, grounded in
the MR-002 git census + the shortcut-inventory git CRITICALs (SI-012/013/014/015 + F-git-2) + roadmap E1.1–E1.4.

## Architecture (confirmed from the code + arch docs)
- Myelin hosts **real on-disk BARE git repos** at `<tenant>/<region>/<repo>.git`, opened via `git2`/`gix`
  (`gix_backend.rs::GixCore`/`RootedResolver` — REAL, the genuinely-real read organ). Read/diff/blame/
  projection already run on real repos.
- The **smart-transport wire** (`upload-pack`/`receive-pack` = clone/push) is served by **canonical `git`,
  SANDBOXED** (gix has no server-side upload-pack/receive-pack, re-verified 2026). ⇒ the raw wire **DEPENDS
  on the sandbox production-exec floor (P-544/545)** — the CI long-pole track + RESHAPE-001 (the sandbox
  launch() result/lifecycle seam). So the raw clone/push wire is **GATED** and sequenced with the CI track.
- Durable storage, backup/restore, API-writes, web UI, CLI/MCP are **sandbox-INDEPENDENT** (real on-disk git
  via git2/gix in-process) and proceed now. `git2`/`libgit2-sys` are in the lock.

## The census CRITICALs this track closes
- **SI-012** RefStore in-memory (`receive_pack.rs:537`), `open` loads nothing → GT-001.
- **F-git-2** pack oid→hash index in-memory, rebuilt on open; FsBlobStore is `Mutex<HashMap>` not fs → GT-001.
- **SI-014/015** backup modeled (WAL offset, no bytes) / restore modeled → GT-002.
- **SI-013** no production `WireExecutor` (clone/push has no backing) → GT-006 (gated on the sandbox).

## Conventions
Same as the spine (`09-spine-prompt-ledger.md`): `GT-NNN` ids; anti-duplication grep + ledger-vs-commits
cross-check opens every prompt (reuse `gix_backend`/`git2` + the MR-022 provider + the MR-014/015 edge +
the MR-019 app shell + the MR-020/021 CLI/MCP — extend, never fork); orchestrator runs the FULL gate
(`cargo check --workspace --all-targets` + `cargo test -p myelin-lints` + the touched crates +
`--features integration` against live PG); **independent verification** on every load-bearing prompt
(builder ≠ verifier); the **external-oracle test** (real `git` clone/push/fetch + `git fsck`) is the
Git-track done-bar (woven where possible, full in GT-006). Commit per prompt.

## The Git-track prompt set

| ID | Epic | Title | Deps | Sandbox? | Size |
|---|---|---|---|---|---|
| GT-001 | E1.1 | **Durable git storage:** real on-disk bare repos (git2/gix) as the production backend; the `RefStore` (refs + reflog) + the object/pack store become durable (survive restart); `open` loads from disk. Reconcile `receive_pack.rs::RefStore` + `pack_tier.rs`/`blob.rs` with the real on-disk odb. | MR-022 | no | high |
| GT-002 | E1.1 | **Real backup + DESTRUCTIVE restore** of your repos (the repo bytes, not a modeled WAL offset): back up the on-disk bare repos, restore to a CLEAN target, read the repos back + `git fsck` clean. Fixes SI-014/015 (git slice). | GT-001 | no | mid–high |
| GT-003 | E1.2 | **Git product-API writes durable:** the MR-015 `durable:false` writes (create-repo / ref-update / open-PR / review / merge) now PERSIST on the real on-disk backend through the edge, under the verified tenant scope + the merge-gate/fork-trust policy (reuse the existing in-process logic). | GT-001, MR-015 | no | high |
| GT-004 | E1.3 | **Git web UI:** repo home / file+tree browse / commit log / diff / PR overview + review — real Solid components on the MR-019 app shell, reusing the `web.rs` ViewModels (now edge-served, GT-003) + the MR-016/017 design-system/overlays; Playwright+axe. | GT-003, MR-019 | no | high; may split (browse vs PR/review) |
| GT-005 | E1.4 | **Git CLI + MCP surface:** the git operations (repo/PR/review/merge) as real `myelin git` CLI commands (MR-020) + MCP tools (MR-021, governed via EffectApi) wired to the real backend; **+ the MR-010d follow-up** — populate `KeyBindingIndex` from the durable PG `PrincipalStore` on SSH-key registration + issue the SSH challenge on the git handshake. | GT-003, MR-020, MR-021 | no | mid–high |
| GT-006 | E1.1/E1.4 | **The smart-transport WIRE** (`upload-pack`/`receive-pack` = real `git clone`/`push`/`fetch`) via the SANDBOXED canonical git + the production `WireExecutor` (SI-013) + the git server binary/listener (SSH/http) + **the external-oracle test** (real `git clone`/`push`/`fetch` + `git fsck` against your repos). | GT-001, **the sandbox prod-exec (P-544/545) + RESHAPE-001** | **YES — gated on the CI/sandbox track** | high; long-pole |

## Waves
- **W1:** GT-001 (durable storage)
- **W2:** GT-002 (backup/restore) · GT-003 (durable API writes)
- **W3:** GT-004 (web UI) · GT-005 (CLI/MCP + SSH-auth wiring)
- **W4 (gated on the sandbox/CI track):** GT-006 (the wire + the external-oracle test)

GT-001..005 make Git a usable daily driver through the UI/CLI/API/MCP on durable, backed-up, restorable
real git repos. GT-006 (raw `git clone`/`push` over the wire) lands when the sandbox prod-exec is real —
sequenced with the CI track, per the master plan's "don't let the hardest subsystem block the useful one."

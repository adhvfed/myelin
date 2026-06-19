# Sketch 03 — The push path: policy locus, outbox emit, per-ref ordering

> Exploration note. Where push-time policy runs (native hooks vs in-process receive-pack), and how the
> ref move + outbox emit + per-ref ordering compose into one atomic transaction. This is the spine of
> agent-nativeness (every reaction hangs off `git.ref.updated`) and the per-ref-ordering obligation
> the Phase-3 handoff assigns us. Date: 2026-06-19.

## What must be true (obligations)

- **Per-ref ordering at push QPS** (Phase-3 handoff; bus §2.3): `git.ref.updated` for one ref delivers
  in push order; the **aggregate is the ref** (`git/ref/<repo_id>:<ref_name>`), so different refs of a
  repo fan out in parallel but a hot `main` is totally ordered.
- **Outbox-only emit** (BUS-2): the event is written to the per-service `outbox` table **in the same
  transaction as the ref move**; no `publish_now`. Outbox order == ref-update order by construction.
- **Reject before the ref moves** (Phase-1 §6): branch protection, push limits, secret-scan, size
  limits, signed-commit/DCO, agent-vs-human rules must run *pre-receive* and reject atomically.
- **No event on a rejected push; exactly one event per accepted ref move** (no lost/ghost events on a
  busy server — Phase-1 §11.7).

## The locus question — where does push-time policy run?

### Candidate A — native git hooks (`pre-receive`/`update`/`post-receive` scripts)
Canonical git invokes hook scripts; we drop in a `pre-receive` that calls back into our policy engine
and a `post-receive` that emits events.
- **Pros:** standard, works with shell-out-to-git (sketch 02); well-understood.
- **Cons:** the hook is a **separate process** with a stdio contract — slow (spawn per push), awkward
  to make transactional (the post-receive emit is *after* the ref already moved → the dual-write
  hazard the outbox exists to kill); failures in post-receive can't be retried atomically with the ref
  move; harder to reason about ordering.

### Candidate B — in-process receive-pack with an embedded policy + outbox step
We wrap `receive-pack` so the **quarantine → policy → atomic (ref move + outbox insert)** sequence runs
**inside our Rust serving tier as one DB transaction**. Git's *quarantine* mechanism
(`GIT_QUARANTINE_PATH`) already stages incoming objects in a temp area that is only migrated into the
repo if `pre-receive` accepts — we lean on exactly that.

- **Pros:** **transactional** — the ref move and the outbox row are one commit (BUS-2 satisfied by
  construction); policy runs in-process (fast, no spawn, shares the authz/refs caches); ordering is
  the DB's `UNIQUE(aggregate, seq)` (bus §3.2); a rejected push leaves zero trace because the objects
  never leave quarantine.
- **Cons:** more to build than dropping a hook script; must orchestrate git's quarantine + our ref-store
  transaction (since the ref store is DB-backed — sketch 04 — this is *natural*: the ref move IS a DB
  txn).

**Leaning: Candidate B.** Phase-2 §2 already prescribes "an in-process receive-pack with embedded
policy engine (faster/safer at scale)" as the open option, and the DB-backed ref store (sketch 04)
makes the atomic-(ref-move + outbox) transaction the *natural* shape, not extra work. The dual-write
hazard is the exact thing the outbox doctrine (EI-02 §4) forbids — a post-receive script *is* a dual
write. So we reject native post-receive for emit.

## The atomic push transaction (the committed shape)

```
receive-pack(repo, refspecs):                          # in-process, in the serving tier
  1. Stream objects into a QUARANTINE area (GIT_QUARANTINE_PATH); index-pack --strict (validate).
  2. For each proposed ref update (old_sha → new_sha):
       policy = evaluate_push_policy(principal, repo, ref, old_sha, new_sha, commits)   # in-proc Rust
         - Id.check(principal, push|protected_push, ref)        # authz (sketch 05)
         - branch-protection ruleset (required signatures, linear history, no-force, agent rules)
         - size/secret-scan/file-policy on the quarantined objects
       if policy.reject: ABORT (objects stay in quarantine → GC'd; NOTHING moved; no event). return error.
  3. BEGIN TXN (ref-store DB):
       - migrate quarantined objects into the repo's object store (pack/loose; via BlobStore trait)
       - UPDATE ref tip old_sha → new_sha  (compare-and-set on old_sha = the linearisation point)
       - INSERT outbox row { aggregate='git/ref/<repo>:<ref>', seq=next(aggregate), type='git.ref.updated',
                             envelope{ subject, actor, causation, contains_personal_data=false, ... } }
       - (force-push?) also stamp the reflog entry + an outbox 'git.ref.force_updated' detail
     COMMIT       # ref move + event are now one fact; relay drains the outbox to the bus (bus §4.1)
  4. ack the client.
```

- **The compare-and-set on `old_sha`** is how concurrent pushes to one ref serialise: two pushers both
  see `old=A`; the first commits `A→B`; the second's CAS fails (tip is now `B`, not `A`) → it is
  rejected with non-fast-forward (or, for `--force-with-lease`, the lease check). This is the
  linearisation Phase-2 §3 demands, expressed as a **DB row CAS**, not a filesystem lock.
- **`seq` is per-ref** (`aggregate = git/ref/<repo>:<ref>`), allocated inside the txn, so
  `UNIQUE(aggregate, seq)` enforces per-ref order at the source (bus §3.2/§2.3). Different refs get
  different aggregates → parallel fan-out.
- **Object migration uses the `BlobStore` trait** (STOR-5) so the same transaction works whether packs
  are local-disk or (later) object-backed.

## Events emitted on the push path (seed; full taxonomy in sketch 08)

`git.ref.updated` (the core push event: repo, ref, old_sha, new_sha, forced?, commit shas, pusher).
Derived/adjacent on the same path: `git.branch.created`/`git.branch.deleted` (ref create/delete),
`git.tag.created`/`git.tag.deleted`, and **`git.protection.bypass_used`** (audit-critical) when a
bypass-list principal overrides a rule. **One `git.ref.updated` per accepted ref move**; commit-level
detail rides the payload (references-not-payloads — the event carries shas + an `ArtifactRef`, not
file contents).

## PII at push time (ties to GIT-1 / sketch 09)

The push path is where **pseudonymous-by-default commit identity** must already be in effect: the
commit object's author/committer is the **stable opaque pseudonym** (`<pseudonym>@<tenant>.noreply`),
resolved at commit time, never the raw email (sketch 09). The `git.ref.updated` envelope therefore
carries `actor = pseudonymous principal`, `contains_personal_data = false` — the event-log half is
erasure-clean by construction.

## Leaning (committed in findings)

**In-process receive-pack** with an **embedded Rust policy engine**, using git's **quarantine** to stage
objects, and a **single ref-store DB transaction** that does `{migrate objects, CAS the ref tip, insert
the outbox row}` atomically. Per-ref ordering = `aggregate=git/ref/<repo>:<ref>` + `UNIQUE(aggregate,
seq)`. **No native post-receive emit** (it's a dual write). Policy *rejects before the ref moves*; a
rejected push leaves zero trace.

## Prior art / sources

- git **quarantine** (`GIT_QUARANTINE_PATH`) — atomic accept/reject of incoming objects (git internals).
- Transactional outbox (Richardson, *Microservices Patterns* 2018; EI-02 §4; Phase-3 bus §3.2/§4.1).
- Per-ref ordering via partition-by-the-entity-whose-order-you-need (Kreps 2011; bus §2.3).
- Phase-2 git-hosting §2 (in-process receive-pack option), §7.1 (events + per-ref ordering).

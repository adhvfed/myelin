# Self-hosted CI follow-ons

CT-007 is done: Myelin's CI builds/tests/lints itself in gVisor; GitHub Actions off.
Remaining:

- **Durable log 512 KiB cap** — on a long *green* run the tail crates' result lines
  truncate (correctness is covered by the exit-code disposition; observability isn't).
  Raise/redesign the cap without losing a DoS bound.
- **Runner asset breadth (#7)** — Node/browser/container-shaped jobs + an advisory-DB
  egress lane, to broaden paid CI beyond Rust.
- **ci.pipeline v5 fleet bump** — a multi-cell deploy needs the definition-cutover
  fence bumped 4→5; single-cell dogfood is fine.
- **Lint gate red** — `myelin-ci-controlplane` tenant-predicate flags are verified
  false-positives (no leak; queries are `with_tenant_tx` RLS + explicit predicates);
  the `forward-only-migration` flag is a real minor in-place `SET NOT NULL`. Fix the
  lint's `with_tenant_tx` detection or thread an explicit TenantId. (task #38)

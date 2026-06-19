# Sketch 05 — Config grammar (config-as-code) + the component/action registry supply-chain (TE-30)

> Phase 4 — CI exploration. Two coupled decisions: (1) the **pipeline definition grammar** —
> config-as-code, the most consequential design axis (CI-DD §3); and (2) the **reusable
> component/action registry** and its **supply-chain trust model** (TE-30), citing SLSA + sigstore.

---

## Part 1 — Config grammar

### The tension (CI-DD §3, an explicit unresolved spectrum)
- **Pure declarative YAML** (Actions/GitLab): approachable, diffable, **agent-generatable**, but
  "YAML programming" pain (anchors, templating, expression mini-languages, copy-paste matrices).
- **Programmable/typed** (Dagger/Earthly/CDK-for-pipelines): real abstraction, testable — but a higher
  barrier, a **supply-chain surface** (you run arbitrary code just to *compute* the pipeline), and
  harder for PMs to read.
- **Hybrid** (declarative core + a safe expression language + an optional dynamic-generation step): the
  pragmatic middle most mature systems converge on.

### The binding selection criteria (already committed, CI-DD §3 / Phase-2 §3)
**Agent-generatability and human-diffability are first-class** — this **biases away from
pure-programmatic**. Two more platform constraints push the same way:
- **One matcher engine platform-wide:** trigger predicates MUST be the shared **`EventMatcher` / query
  AST** (ADR-07; Bus §4.5) — JSON, bounded interpreter, **no UDFs/loops/recursion, statically
  cost-bounded, permission-aware** ("Not CEL/JSONLogic"). CI does not invent a trigger expression
  language; it reuses this. That forecloses a Turing-complete trigger condition by construction.
- **Determinism of resolution is an audit requirement** (CI-DD §3): definition + event-context →
  concrete run (matrix expansion, conditionals, secret/cache-key resolution) must be **deterministic
  and content-addressed-snapshotted** per run (Phase-2 §2.1).

### Candidate leanings
- **A — pure YAML, no escape hatch.** Simplest, but hits the YAML-programming wall for advanced users;
  no answer for genuinely dynamic fan-out.
- **B — declarative JSON-schema'd core + safe expressions + a sandboxed dynamic-generation escape
  hatch — chosen.** A strongly-schema'd declarative format (authored as YAML/TOML, **validated against
  a published JSON Schema**) is the primary mode; expressions use the **same bounded query-AST grammar**
  as triggers (one expression language platform-wide, no second mini-language); a **dynamic-generation
  step** (a job that *emits* a pipeline fragment) is the escape hatch for genuinely programmatic
  fan-out — and crucially **that generation step runs in the sandbox** (sketch 01), so "run code to
  compute the pipeline" inherits the *same* isolation as any other untrusted code (it doesn't get a
  privileged config-eval path). This is the Buildkite-dynamic-pipelines shape, hardened.
- **C — typed Rust/WASM config.** Powerful + testable, but worst on PM-readability and agent-
  generatability, and it makes config-eval a first-class supply-chain surface. Rejected as the default;
  the dynamic-generation escape hatch (B) covers the programmatic need without making *every* pipeline
  a program.

### Shift-left is core, not nice-to-have (runner compute is the cost center, CI-DD §3)
`myelin ci validate` (JSON-schema + lint) and `myelin ci plan` (resolved DAG + matrix expansion +
referenced secrets, **no runner spend**) are first-class — they reduce wasted compute and are the
**editor + validator view**'s backing (design wireframes, §05). The plan output is exactly the
content-addressed snapshot the run will pin (reproducibility).

```yaml
# Illustrative — declarative core; expressions are the shared bounded query-AST (NOT a new language)
on:
  pull_request: { branches: ["main", "release/*"], paths: ["src/**"] }   # → compiles to an EventMatcher
jobs:
  test:
    runs-on: { labels: [linux, large] }                                  # affinity (sketch 03)
    matrix: { os: [linux], rust: ["1.79", "stable"] }                    # deterministic fan-out
    uses: myelin://acme/ci/actions/cargo-test@sha256:abcd…               # component pinned BY DIGEST (Part 2)
    steps: [ { run: "cargo test --all" } ]
deploy:
  needs: [test]
  environment: prod                                                      # protected → HITL gate (sketch 02)
```

## Part 2 — The component/action registry + supply-chain (TE-30)

Reusable components (Actions' "actions", GitLab `include`, Tekton tasks, Drone plugins) are essential
for not-copy-pasting — and they immediately raise **supply-chain trust** (CI-DD §3/§7). The product
question "does Myelin host an EU-sovereign registry?" is commercial-flagged; the **trust model is
architectural and decided here**.

### The non-negotiable: pin by digest, fail-closed on a floating tag
- **A reusable component reference MUST resolve to a content digest** (`@sha256:…`). A **floating tag is
  rejected, fail-closed** — exactly the image-digest-pinning rule of the hardening profile (sketch 01;
  CI-1), applied to *components* too. This is the single highest-leverage supply-chain control (it kills
  "the action changed under me" and tag-mutation attacks).
- The resolved digest is recorded in the **per-run content-addressed snapshot** (Part 1) → a run is
  exactly reproducible and auditable down to which component bytes it ran.

### Provenance + signing — cite SLSA + sigstore
- **SLSA** (Supply-chain Levels for Software Artifacts — the OpenSSF framework, successor framing of
  Google's Binary Authorization for Borg) is the **provenance ladder** we target: a produced artifact
  carries **signed provenance** attesting *which run, which definition snapshot, which inputs* built it.
  v1 floor: provenance generated for artifacts (the builder is trusted, attestation recorded). The
  higher SLSA levels (hermetic, isolated, two-party) are the named follow-on.
- **sigstore** (Fulcio keyless signing + Rekor transparency log; cf. Certificate Transparency, RFC
  6962, which Phase-3 already adopts for the audit log) is the **signing + verification** mechanism:
  components and produced artifacts are **signed**, and signatures are **verified before use**. The
  Rekor-style transparency log is conceptually the same Merkle-CT structure GDPR/Audit already builds
  (GDPR §6) — we reuse the pattern, EU-hosted.
- **SBOM** (CycloneDX/SPDX-class) generation for produced artifacts is a first-class capability — the
  **EU-sovereign supply-chain-security pitch is strongest if Myelin is *better* than incumbents here**
  (CI-DD §6.3, a deliberate differentiator).

### Self-hosted runner attestation (the fleet trust surface — ties to sketch 03)
A self-hosted runner is a semi-trusted node; it **attests** at registration (hardware/TPM or a
provisioning-time signed token) and receives a **scoped job token**. Attestation status is surfaced in
the runner-fleet view (§05 wireframes). A compromised runner is bounded by its scoped token (CI-DD §7).

### Egress + untrusted components
A component is **untrusted code** like any step — it runs in the sandbox under the same egress
default-deny + trust-tier gating. A fork-tier run using a component still gets no secrets and an
isolated cache (sketch 04). Pinning-by-digest + signature-verify + sandbox = the layered defence.

## Floors & follow-ons
- **FLOOR:** v1 ships JSON-schema validation + the bounded-expression core + the sandboxed
  dynamic-generation escape hatch; the full reusable-component *registry product* (hosting, discovery)
  is commercial-flagged (TE-30) — the **trust model (digest-pin + sign + verify + SLSA provenance) is
  built regardless**, even for components referenced from a customer's own repo.
- **FLOOR:** SLSA L1–L2-grade provenance (build-time attestation) ships; hermetic/two-party (L3+) is
  the named follow-on (measured by customer demand).
- **`myelin ci local`** (laptop execution) is `[OPEN → P4]` (CI-DD §11 Q12) — a UX win vs fidelity cost;
  not committed.
- **Drill owed (PROVE-IT):** **un-digested-reference rejection** — a pipeline referencing a floating tag
  (image or component) **fails closed** at plan time; and **signature-verify-before-use** — a
  tampered/unsigned component is refused. Gate: zero un-pinned executions, zero unsigned-component runs.

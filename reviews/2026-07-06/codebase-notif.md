# Notifications (myelin-notif)

_myelin-notif is a mature, heavily-tested unit: per-recipient authz (list_inbox recipient-scope + step-0 check + fail-closed on Conditional), the read-fanout zookie watermark gate, the references-not-payloads holder/erasure posture, cross-cell tombstone-not-leak, and the delivery idempotency ledger are all correct and well-guarded. The core delivery-guarantee and no-leak invariants hold. The findings below are a genuine missed-notification path in storm-control and some defense-in-depth gaps, not authz bypasses._

**Kept findings:** 4  (🟡 1 medium  ·  🔵 3 low)

---

### 1. 🟡 Direct-class @mentions are rate-damped and their inbox row is dropped entirely, silently losing notifications

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** correctness
- **Location:** `crates/myelin-notif/src/storm_control.rs:457`

**What:** StormControl::decide (storm_control.rs:457) gates mechanism-4 rate damping on `if !pierces && !self.buckets.try_take(...)` → Suppress(RateDamped). `pierces = ctx.quiet.pierces(item.class)`, and QuietHours::default() (prefs.rs:347) has `pierce_classes: vec![Class::Critical]`, so `pierces(Class::Direct)` is false. Write-fanout @mentions are class=Direct (router.rs:734, derive_mention_item), so they ARE subject to rate damping. SuppressReason::RateDamped.writes_row() == false (storm_control.rs:125), and route_one_candidate returns early on `!decision.writes_row()` (router.rs:658) — writing NO inbox row and emitting nothing. This contradicts this module's own mechanism-4 doc line 38 ('A Critical/Direct item is exempt — the on-call page is never damped') and the 'break out the direct — you always see the one addressed to you' principle applied for coalescing (mechanism 3, lines 31-32/214-216). Rate-damping is one of the mechanisms the router marks fully live now (router.rs:640), so the defect is active, not deferred. The hardcoded `tick: 0` (router.rs:646) means the per-(recipient, subject_root) bucket never refills in the current skeleton wiring, so damping past the 5-token burst is permanent for that pair — but the core exemption bug stands independent of tick.

**Impact:** Five distinct comments/events each @mentioning the same user on the same PR/thread (distinct dedup_keys, same subject_root) exhaust the 5-token burst; the 6th and later Direct mentions to that user on that subject produce Suppress(RateDamped) and thus no inbox row — the directly-addressed notification is absent from the ONE inbox (the recipient's system of record). The underlying Signal remains on the bus (audit untouched), but the recipient never sees they were mentioned.

**Fix:** Exempt break-out classes (Direct/Critical) from rate-damping, e.g. gate the bucket on `!is_break_out_class(item.class) && !pierces` (is_break_out_class already exists at storm_control.rs:231). Alternatively make the RateDamped verdict still write the inbox row (writes_row=true, suppressing only the channel push) so a direct mention is never lost from the inbox. Reconcile the internally-contradictory doc: module line 38 says Direct is exempt, but the decide() inline comment at lines 439-442 says only a 'piercing class' is exempt.

> _Verifier note:_ Confirmed by reading storm_control.rs decide() (line 457 gate uses `pierces` only), writes_row() (line 122-127: RateDamped→false), router.rs route_one_candidate (line 658 early-return on !writes_row), derive_mention_item (line 734: class=Direct), and prefs.rs QuietHours::default (line 347: pierce_classes = {Critical} only, so pierces(Direct)=false). Module doc line 38 explicitly promises Direct exemption. Real, active defect.

### 2. 🔵 Delivery fabric does not enforce off-cell redaction; it trusts the caller and records redacted=true regardless

- **Severity:** low  ·  **Verdict:** 🟨 PLAUSIBLE  ·  **Category:** gdpr
- **Location:** `crates/myelin-notif/src/delivery.rs:480`

**What:** DeliveryFabric::deliver (delivery.rs:453-511) takes a `&RedactedMessage` and, for an off-cell channel, sends it verbatim (`adapter.send(message, &idem_key)`, line 480) then records `redacted = channel.is_off_cell()` (line 481) — the flag is derived purely from the CHANNEL, never from an inspection of the message. RedactedMessage (lib.rs:410-416) is `{ rendered: HumanisedString, class }` and is the SAME type for in-cell and off-cell; there is no type-level or runtime guarantee the off-cell message was built via redact_for_offcell. The module docs state 'the redaction discipline is the CALLER's' (line 477). So a future/incorrect off-cell call site that carries a fuller humanised render would egress it off-cell while the ledger stamps redacted=true.

**Impact:** If a future call site passes an un-minimised message to an off-cell channel (email/web_push/mobile_push/desktop), the extra content egresses off-cell and the durable notif_delivery ledger records redacted=true — so the exposure is neither prevented nor observable. This is defense-in-depth: the payload is constrained to a HumanisedString (already per-viewer, tombstone-safe by construction, lib.rs:331-344), and no current call site misuses it, so it is not an active leak.

**Fix:** Add a structural or runtime guard on the off-cell path: a distinct OffCellMessage newtype the off-cell adapter path requires (constructible only via redact_for_offcell), so `redacted = true` is a fact the fabric enforces rather than a channel-derived flag it trusts.

> _Verifier note:_ Confirmed the code shape: deliver() sends verbatim (line 480) and sets redacted from channel.is_off_cell() (line 481); RedactedMessage is one shared type (lib.rs:410). Downgraded confidence to PLAUSIBLE because there is no current failing call site and the carried type is a viewer-safe HumanisedString, not a raw body — this is a valid hardening/defense-in-depth suggestion, not an active defect. Low severity retained.

### 3. 🔵 read_fanout lowers an empty SetExpr::Intersect to Reachable::All (unbounded widening) on a leak-critical path

- **Severity:** low  ·  **Verdict:** 🟨 PLAUSIBLE  ·  **Category:** security
- **Location:** `crates/myelin-notif/src/read_fanout.rs:475`

**What:** In lower() (read_fanout.rs:473-480), SetExpr::Intersect(parts) seeds `acc = Reachable::All` and narrows per part. For an empty `parts` slice the loop never runs and it returns Reachable::All, which project_with (line 617) turns into EVERY ambient marker of the tenant (Reachable::contains returns true for All, line 529-530). On this permission-gated read (the viewer's watched subject_roots) that is a fail-open widening, whereas SetExpr::Union([]) correctly returns the empty set (line 467). The module's stated posture elsewhere is deny-when-unsure / held-not-leaked.

**Impact:** If the Identity list_objects resolver ever emitted Filter{Intersect([])}, the viewer's ambient inbox slice would expand to every tenant subject_root (existence/metadata over-exposure), rather than failing closed. Content is still tombstoned per-viewer at humanise time, so this is metadata/existence over-exposure, not an immediate content leak.

**Fix:** Treat an empty Intersect as the empty set (fail-closed) on this read-fanout path, or reject a degenerate empty boolean node loudly — mirroring the held-not-leaked default used for unavailable resolvers.

> _Verifier note:_ Confirmed the code fact: lower()'s Intersect arm seeds Reachable::All and returns it unchanged for empty parts (read_fanout.rs:473-480); Union's empty case is empty (line 467). PLAUSIBLE not CONFIRMED because the widening only triggers if Identity emits a degenerate empty Intersect (algebraically the intersection identity IS the universe, so `Intersect([])`==All is mathematically defensible), and no producer here does so — this is a fail-closed hardening on the leak-critical path. Low severity retained.

### 4. 🔵 cold_rebuild returns only the first 50 items despite claiming a full zero-loss rebuild from source

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** correctness
- **Location:** `crates/myelin-notif/src/watch.rs:305`

**What:** cold_rebuild (watch.rs:297-313) — the named resync_required fallback documented as 'the cold rebuild replaces the lost live view entirely' — calls list_inbox with `Page::default()`, whose limit is 50 (list_inbox.rs:259) and which returns a forward Cursor (list_inbox.rs:346-350). cold_rebuild returns a single InboxPage and drops nothing itself, but the convenience cold_rebuild_item_ids (watch.rs:317-328) maps ONLY that first page's item_ids and discards the cursor. So for an inbox with more than 50 active items, cold_rebuild yields only the first page and the zero-loss drill helper compares a truncated 50-item set against the live set.

**Impact:** A recipient with >50 inbox items who hits the resync_required path recovers only their first 50 items via cold_rebuild / cold_rebuild_item_ids. The D-N11 zero-loss assertion built on cold_rebuild_item_ids would be evaluated against a truncated set, masking loss rather than proving its absence. Currently latent: the D-N11 drill (drill_notif_d11.rs:191) seeds only 6 items, and the production CLI recovery path directs users to the paginated `myelin inbox list` (cli.rs) rather than cold_rebuild, so no live path is broken today.

**Fix:** Have cold_rebuild page to exhaustion (loop on the returned Cursor) or return the cursor so the caller/drill can, and make cold_rebuild_item_ids collect across all pages before asserting zero loss. Alternatively soften the 'replaces the lost live view entirely / ZERO items lost' doc claim to match the single-page behavior.

> _Verifier note:_ Confirmed: Page::default() limit=50 (list_inbox.rs:259); cold_rebuild passes Page::default() (watch.rs:305-312); cold_rebuild_item_ids maps items and drops the cursor (watch.rs:323-327). Drill seeds 6 items so it currently passes without exercising the truncation (drill_notif_d11.rs:191). Real doc-vs-behavior gap in a pub API; low severity because currently latent and the CLI recovery uses paginated list.

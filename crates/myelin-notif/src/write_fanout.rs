//! # Write-fanout for the bounded high-signal set (NOTIF-P12 / P-190, M2) — the frozen
//! `mention(Principal)` structured node + the hot-subject cap
//!
//! **Owning architecture doc:** `notifications.md` §3.5 ("Write-fanout for mentions vs read-fanout
//! for bodies"): **fan-out on WRITE** for the small bounded high-signal set
//! (mentioned/assigned/reviewer/escalation targets) — materialise one `inbox_item` per recipient;
//! the **hot-subject cap** (§3.2.4) bounds even the write-fanout side so a mention-storm can't
//! write-amplify. **Contract:** 13.1 (the `mention(Principal)` frozen inline structured node in the
//! `myelin-content` taxonomy — identical across Chat/Issues/Knowledge, X-2/C10). **External
//! insight:** `04-hard-problems.md` §5.3 (Notif reads the STRUCTURED node, never free text — AG-6:
//! only a structured ref re-triggers), §2.2 (the bounded high-signal set);
//! `01-process-and-quality-doctrine.md` §3 (prove-it). **Drill:** NOTIF-D2 (the mention-storm side —
//! the hot-subject cap bounds write-amplification; asserted jointly with NOTIF-P13's read-fanout).
//!
//! ## What this prompt (NOTIF-P12) ships — write-fanout from the STRUCTURED node, bounded
//!
//! The router (NOTIF-P3, [`crate::router`]) already routes ONE candidate per Signal (the ambient
//! `psn:watcher:<rule>` skeleton recipient). **Write-fanout is the bounded high-signal LEG**: when a
//! Signal carries `mention(Principal)` structured nodes (the recipient was directly addressed), the
//! router materialises **one inbox_item per mentioned recipient** — the §3.5 step-1 DIRECT
//! (write-fanout) set: `mentioned`/`assigned`/`reviewer`/`escalation` targets. Each materialised row:
//!
//! 1. is derived from the **STRUCTURED node** [`InlineNode::Mention`]`(Principal)` — the
//!    `principal_id` is read off the verified [`Principal`], never scraped from a free-text body
//!    (AG-6: `extract_mentions` takes `&[InlineNode]`, NOT a `&str` — a free-text parse is
//!    *unconstructable* at the type level).
//! 2. is classified `reason = Mentioned`, `class = Direct` (the §3.1 high-signal mapping — a mention
//!    is directly addressed to you; it is a break-out class storm-control never folds into a digest).
//! 3. UPSERTs through the SAME [`InboxProjection`](crate::router::InboxProjection) write-time collapse
//!    (`(tenant, recipient, dedup_key)`) the NOTIF-P11 storm-control reads — a redelivered / same-key
//!    mention collapses, it does NOT double-notify.
//!
//! ## The hot-subject cap (§3.2.4) — bounds the WRITE-FANOUT side
//!
//! A mention-storm on a hot subject (a `@here`-style spray, or an agent that mentions 10k principals
//! on one thread) must NOT write-amplify into N rows. The **hot-subject cap**
//! ([`HotSubjectCap`]) bounds, per `subject_root`, how many DISTINCT mention rows a single fanout may
//! materialise. Beyond the cap, further mentions **coalesce** into the ONE overflow marker for that
//! `subject_root` (the "+N more were mentioned" counter) rather than materialising N new rows — the
//! write-amplification is bounded by [`HotSubjectCap::cap`], not by the mention count. This is the
//! write-side analogue of the read-fanout's "store ONE coalesced marker" (§3.5): a celebrity-spray
//! mention costs at most `cap` write rows, never `N`.
//!
//! The cap is `(recipient, subject_root)`-distinct-counted: each NEW `(recipient, subject_root)`
//! consumes one slot; a repeat of the same recipient on the same root collapses on the dedup key
//! (storm-control mechanism 2) and does NOT consume a fresh slot. So the cap bounds the count of
//! DISTINCT recipients write-amplified per subject — exactly the mention-storm axis.
//!
//! ## FLOOR named (per EI-01 §1)
//! - **The read-fanout** for the unbounded ambient set (every watcher of a hot PR, every member of a
//!   50k channel — store ONE coalesced marker, materialise per-watcher lazily on inbox open: the
//!   `SetExpr` watcher push-down JOIN + the zookie watermark) is **NOTIF-P13** (§3.5). Write-fanout
//!   here is ONLY the bounded DIRECT set; the unbounded AMBIENT set is read-fanned. Named so
//!   write-fanout is not mistaken for the full scale answer (the §3.5 hybrid is both legs).
//! - **The live OLTP backing** of the inbox UPSERT + the durable overflow marker rides the
//!   `notif_inbox_item` table (the `coalesce_count` column NOTIF-P2 declares) when the OLTP client
//!   wires into `serve` (P-007 / P-S12); this module models the write-time decision in-memory (the
//!   same pattern as [`InboxProjection`](crate::router::InboxProjection)). The DECISION shape (read
//!   the structured node, one row per recipient, cap the storm) does not change.
//!
//! ## Mutation floor (the write-fanout decision module — mandatory-core)
//! Write-fanout is mandatory-core (a wrong verdict either mis-fans a mention or lets a storm
//! write-amplify). The mutation-tested core is [`extract_mentions`] (reads ONLY the structured
//! `Mention` node, never free text), [`HotSubjectCap::admit`] (the per-`subject_root` distinct-count
//! cap), and [`SignalRouter::write_fanout`](crate::router::SignalRouter) (one row per mentioned
//! recipient through the storm-control collapse, the rest coalesced past the cap). **Floor: ≥ 80%
//! line/branch mutation score on `write_fanout.rs`** (measured with `cargo mutants`; reported in the
//! P-190 commit body). The unit + chained tests below assert one-row-per-recipient, the no-free-text
//! property (the type forbids it), the cap bounds the burst, and a mutant that drops the cap,
//! mis-counts a recipient, or scrapes free text is caught.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use myelin_content::InlineNode;
use myelin_identity::Principal;

/// The platform default hot-subject write-fanout cap (§3.2.4) — how many DISTINCT mention rows a
/// single subject_root may materialise on write before further mentions coalesce into the overflow
/// marker. A frozen value read from ONE place (never re-stated per call-site). `64` is the bounded
/// high-signal floor: a normal thread mentions a handful of people (well under the cap); a
/// mention-storm (`@here` on a 10k channel) is bounded to `64` write rows + ONE "+N more" marker,
/// never 10k rows. The cap is a NAMED floor the drills tune (T-5), not a claimed-final number — but
/// it is a real bound, never weakened to pass.
pub const DEFAULT_HOT_SUBJECT_WRITE_CAP: u32 = 64;

/// **Mechanism (§3.5 step-1, AG-6): extract the mentioned [`Principal`]s from STRUCTURED content
/// nodes — NEVER from free text.**
///
/// Takes the structured inline nodes (`&[InlineNode]`) carried by the originating content (a chat
/// message, an issue comment, a KN paragraph). Returns the [`Principal`]s of the
/// [`InlineNode::Mention`] nodes IN ORDER, deduplicated by `principal_id` (mentioning the same
/// person twice in one body is ONE high-signal recipient, not two). The other inline nodes
/// ([`InlineNode::ArtifactRefNode`] / [`InlineNode::Embed`]) are NOT mentions — they do not fan out
/// here.
///
/// **The AG-6 property is structural, not a convention:** the parameter is `&[InlineNode]`, the
/// frozen 13.1 taxonomy node — there is NO `&str` overload, so a free-text parse ("find `@alice` in
/// the body") is *unconstructable* at this seam. Notif reads the structured node the producer froze;
/// it never re-derives the mention shape from raw text (which is also the agent-loop reference gate:
/// only a structured ref re-triggers, never raw text).
pub fn extract_mentions(nodes: &[InlineNode]) -> Vec<Principal> {
    let mut out: Vec<Principal> = Vec::new();
    for node in nodes {
        // ONLY the structured Mention node fans out (AG-6). ArtifactRefNode/Embed are not mentions.
        if let InlineNode::Mention(principal) = node {
            // Dedup by principal_id: the SAME person mentioned twice in one body is ONE recipient.
            let already = out.iter().any(|p| p.principal_id == principal.principal_id);
            if !already {
                out.push(principal.clone());
            }
        }
    }
    out
}

/// **The hot-subject cap (§3.2.4) — bounds the WRITE-FANOUT side so a mention-storm cannot
/// write-amplify.** Tracks, per `subject_root`, how many DISTINCT `(recipient, subject_root)` mention
/// rows have been materialised. Each NEW pair consumes one slot up to [`HotSubjectCap::cap`]; beyond
/// the cap, further DISTINCT recipients are NOT materialised as new rows — they overflow into the ONE
/// coalesced "+N more were mentioned" marker for that root. A repeat of an already-admitted
/// `(recipient, subject_root)` does NOT consume a fresh slot (it collapses on the dedup key — the
/// write-time collapse mechanism 2 — so the cap bounds DISTINCT recipients, the mention-storm axis).
///
/// A cloneable handle over shared state so the whole router pool shares ONE cap truth per
/// subject_root (a `@here` spray split across pool workers is still bounded by the one cap).
#[derive(Clone)]
pub struct HotSubjectCap {
    /// The cap — the max DISTINCT mention rows one subject_root may materialise on write.
    cap: u32,
    /// Per-`subject_root`: the set of `(recipient)` already ADMITTED + the overflow count. Keyed by
    /// `subject_root`; the value is the admitted-recipient set for that root.
    admitted: Arc<Mutex<HashMap<String, RootState>>>,
}

/// The per-`subject_root` write-fanout state: which recipients were admitted (consumed a slot) and
/// how many DISTINCT recipients overflowed past the cap (the "+N more were mentioned" counter).
#[derive(Default)]
struct RootState {
    /// The DISTINCT recipients admitted as their own row (bounded by the cap).
    admitted: std::collections::HashSet<String>,
    /// The DISTINCT recipients that overflowed past the cap (coalesced into the ONE marker).
    overflowed: std::collections::HashSet<String>,
}

/// The verdict of [`HotSubjectCap::admit`] for one `(recipient, subject_root)` mention candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapVerdict {
    /// Admit: materialise this recipient's own inbox row (within the cap, OR a repeat of an
    /// already-admitted recipient — a repeat re-admits so the dedup-key collapse bumps its counter,
    /// it never write-amplifies a NEW row).
    Admit,
    /// Overflow: the cap is reached for this subject_root and this is a NEW distinct recipient — do
    /// NOT materialise a new row; coalesce into the ONE "+N more were mentioned" marker (the
    /// write-amplification bound, §3.2.4).
    Overflow,
}

impl HotSubjectCap {
    /// A hot-subject cap with the platform-default write cap ([`DEFAULT_HOT_SUBJECT_WRITE_CAP`]).
    pub fn new() -> HotSubjectCap {
        HotSubjectCap::with_cap(DEFAULT_HOT_SUBJECT_WRITE_CAP)
    }

    /// A hot-subject cap with an explicit cap (a drill drives a small cap to exercise the overflow
    /// without materialising thousands of rows).
    pub fn with_cap(cap: u32) -> HotSubjectCap {
        HotSubjectCap {
            cap,
            admitted: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The configured cap (so a drill / a test reads the bound it asserts against).
    pub fn cap(&self) -> u32 {
        self.cap
    }

    /// **Admit or overflow one `(recipient, subject_root)` mention candidate (the §3.2.4 cap
    /// decision).**
    ///
    /// - An already-ADMITTED `(recipient, subject_root)` → [`CapVerdict::Admit`] (a repeat — it
    ///   re-admits so the dedup-key collapse bumps the existing row's counter; it never opens a NEW
    ///   row, so it does NOT consume a fresh slot or write-amplify).
    /// - A NEW recipient while admitted-count `< cap` → [`CapVerdict::Admit`] (consume a slot,
    ///   materialise the row).
    /// - A NEW recipient while admitted-count `>= cap` → [`CapVerdict::Overflow`] (the cap is
    ///   reached; coalesce into the ONE "+N more were mentioned" marker — bounded write-amplification).
    pub fn admit(&self, recipient: &str, subject_root: &str) -> CapVerdict {
        let mut g = self.admitted.lock().unwrap_or_else(|e| e.into_inner());
        let state = g.entry(subject_root.to_string()).or_default();
        if state.admitted.contains(recipient) {
            // A repeat of an already-admitted recipient: re-admit (the dedup-key collapse handles it),
            // never a NEW row, never a fresh slot.
            return CapVerdict::Admit;
        }
        if (state.admitted.len() as u32) < self.cap {
            state.admitted.insert(recipient.to_string());
            CapVerdict::Admit
        } else {
            // The cap is reached for this subject_root — a NEW distinct recipient overflows into the
            // coalesced marker (the write-amplification bound). Tracked so `overflow_count` reads it.
            state.overflowed.insert(recipient.to_string());
            CapVerdict::Overflow
        }
    }

    /// The number of DISTINCT recipients ADMITTED (materialised their own row) for `subject_root`.
    /// Bounded by [`HotSubjectCap::cap`]. A drill asserts this never exceeds the cap.
    pub fn admitted_count(&self, subject_root: &str) -> u32 {
        self.admitted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subject_root)
            .map(|s| s.admitted.len() as u32)
            .unwrap_or(0)
    }

    /// The number of DISTINCT recipients that OVERFLOWED past the cap for `subject_root` (the "+N
    /// more were mentioned" counter — the coalesced bound). A drill asserts a mention-storm's
    /// overflow is the storm size minus the cap (bounded, never lost — the count is preserved).
    pub fn overflow_count(&self, subject_root: &str) -> u32 {
        self.admitted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subject_root)
            .map(|s| s.overflowed.len() as u32)
            .unwrap_or(0)
    }
}

impl Default for HotSubjectCap {
    fn default() -> HotSubjectCap {
        HotSubjectCap::new()
    }
}

#[cfg(test)]
mod tests;

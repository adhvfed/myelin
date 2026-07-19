//! The **make-it-real evidence gate** (MR-005 — the internal P-540/541 evidence spine).
//!
//! This module is the *attestation layer* on top of the existing band-boundary [`Scorecard`]
//! ([`crate::scorecard`]). It does NOT fork the scorecard framework — it reuses [`Band`],
//! [`GateRow`], [`Scorecard`], [`RowResult`], [`today_iso`] and [`Scorecard::render_markdown`].
//! What it ADDS is the property the existing system lacked: the green verdict is
//! **cryptographically bound to the actual proof-command output**, so a hand-edited scorecard
//! (a verdict flipped to PASS, or the recorded output changed) is detected as a tamper and reds
//! the gate, instead of silently passing.
//!
//! ## Why the existing scorecard was not enough
//! The existing [`Scorecard`] already records a date + a proof line per row and has a ratchet
//! that "cannot be gamed" by *dropping a row* ([`Scorecard::missing_required`]) or *flipping a
//! row green-without-proof* ([`RowResult::pass`] panics on an empty proof). But its on-disk
//! artifact is hand-editable markdown: the PASS verdict is just text, not bound to the real
//! output of the command. MR-005 closes that gap with a content-hash attestation.
//!
//! ## The attestation (what is hashed, where stored, how validated)
//! When the gate records a PASS it captures the proof command's argv, the bytes of its captured
//! stdout+stderr+exit, and the ISO date, then computes:
//!   * `output_digest = blake3(captured-output-bytes)` — the digest of the real output, and
//!   * `hash = blake3(DOMAIN ∥ id ∥ PASS ∥ date ∥ argv ∥ output_digest)` — the attestation hash
//!     that binds the verdict to that output.
//!
//! blake3 is the workspace-standard content hash (the same `blake3::hash(..).to_hex()` that
//! `myelin-storage`'s ContentHash / `pg_migrator` use, P-047) — never a hand-rolled digest
//! (VISION §4 / EI-01 §7). The attested scorecard is serialised to JSON
//! (`testing/scorecards/make-it-real.json`) — the machine-readable artifact the gate
//! re-validates; the `.md` rendered by [`Scorecard::render_markdown`] is the human mirror.
//!
//! Validation ([`AttestedScorecard::validate`]) RECOMPUTES every attestation hash from the
//! stored fields and reds the gate on:
//!   * a missing required row (the ratchet's drop-a-row half),
//!   * a row that is not a proven PASS (claimed-not-proven),
//!   * a PASS row whose attestation is absent or whose recomputed hash ≠ the stored hash (a
//!     tamper / hand-edited verdict / changed output bytes), or
//!   * a stale row (its date older than the freshness window).
//!
//! Only an all-present, all-PASS, all-hash-valid, all-fresh scorecard is GREEN. This is the
//! **red-by-default / fails-closed** property: any of the above leaves the gate RED.
//!
//! ## A named residual (so the verifier probes the right place)
//! The offline hash check detects a *naive* hand-edit (flip the verdict / change a number
//! without recomputing the hash). It does NOT by itself stop a determined attacker who edits the
//! file AND recomputes the public hash over fabricated output — there is no secret key (this is a
//! content hash, not an HMAC/signature). The un-fakeable guarantee comes from the gate's LIVE
//! path: the [`make-it-real-scorecard`](../bin) binary re-runs each proof command and re-derives
//! the output digest, so a green that did not actually run its command produces a digest that
//! does not match a fresh run. The hardening path (an HMAC keyed by a CI secret, or signing the
//! manifest) is named here as the next step.

use serde::{Deserialize, Serialize};

use crate::scorecard::{Band, RowVerdict, Scorecard};

/// Domain-separation tag for the attestation hash (so a make-it-real attestation can never
/// collide with any other blake3 use in the workspace).
const DOMAIN: &str = "myelin.make-it-real.attestation.v1";

/// The default freshness window: a row's dated attestation is stale (and reds the gate) if it is
/// older than this many days. Evidence that a floor "was real a month ago" is not evidence it is
/// real now — a green that cannot prove it *currently* bites is not evidence.
pub const DEFAULT_MAX_AGE_DAYS: i64 = 30;

/// The content-hash attestation that binds one PASS verdict to the real output of its proof
/// command (MR-005). Stored on a [`RowResult`] (the make-it-real gate's passes) and serialised
/// into the attested manifest. A hand-edit that changes any bound field without recomputing
/// [`RowAttestation::hash`] fails [`RowAttestation::verify`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowAttestation {
    /// The proof command argv that was run (e.g. `["test","-p","myelin-lints",...]`).
    pub argv: Vec<String>,
    /// `blake3(captured stdout+stderr+exit)` as lowercase hex — the digest of the real output.
    pub output_digest: String,
    /// The attestation hash: `blake3(DOMAIN ∥ id ∥ PASS ∥ date ∥ argv ∥ output_digest)`, hex.
    pub hash: String,
}

impl RowAttestation {
    /// Compute the attestation for a PASS of `id` whose proof command `argv` ran on `date` and
    /// produced `output` bytes (the captured stdout+stderr+exit). This is the ONLY constructor —
    /// a `RowAttestation` always carries a hash computed from its own fields, so a fabricated
    /// attestation cannot be assembled field-by-field with a stale hash.
    pub fn compute(id: &str, argv: &[String], date: &str, output: &[u8]) -> Self {
        let output_digest = blake3::hash(output).to_hex().to_string();
        let hash = attestation_hash(id, true, date, argv, &output_digest);
        RowAttestation {
            argv: argv.to_vec(),
            output_digest,
            hash,
        }
    }

    /// Recompute the attestation hash for a PASS of `id` on `date` from the stored fields and
    /// compare it to [`RowAttestation::hash`]. `Ok(())` iff they match (the row is untampered);
    /// `Err` naming the mismatch otherwise (the gate reds the row). This is the tamper detector:
    /// flipping a verdict to PASS without matching evidence, or changing the recorded output
    /// digest, no longer reproduces the stored hash.
    pub fn verify(&self, id: &str, date: &str) -> Result<(), String> {
        let recomputed = attestation_hash(id, true, date, &self.argv, &self.output_digest);
        if recomputed == self.hash {
            Ok(())
        } else {
            Err(format!(
                "attestation hash MISMATCH for row `{id}` — stored {} but recomputed {} \
                 (the verdict/output was hand-edited without re-attesting; the gate reds it)",
                short(&self.hash),
                short(&recomputed),
            ))
        }
    }
}

/// The canonical attestation hash over the bound fields. Uses blake3 with record (`\x1e`) and
/// field (`\x1f`) separators so the argv vector cannot be re-segmented to collide.
fn attestation_hash(
    id: &str,
    passed: bool,
    date: &str,
    argv: &[String],
    output_digest: &str,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(DOMAIN.as_bytes());
    hasher.update(b"\x1e");
    hasher.update(id.as_bytes());
    hasher.update(b"\x1e");
    hasher.update(if passed { b"PASS" } else { b"NOTPROVEN" });
    hasher.update(b"\x1e");
    hasher.update(date.as_bytes());
    hasher.update(b"\x1e");
    for a in argv {
        hasher.update(a.as_bytes());
        hasher.update(b"\x1f");
    }
    hasher.update(b"\x1e");
    hasher.update(output_digest.as_bytes());
    hasher.finalize().to_hex().to_string()
}

/// The first 12 hex chars of a hash, for compact human messages.
fn short(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
}

/// Build the bytes whose digest a proof command's output is attested against: the exit code
/// followed by stdout then stderr. Stable across runs of a deterministic command (no timestamps).
pub fn output_bytes(exit_code: i32, stdout: &[u8], stderr: &[u8]) -> Vec<u8> {
    let mut v = format!("exit={exit_code}\n").into_bytes();
    v.extend_from_slice(b"--stdout--\n");
    v.extend_from_slice(stdout);
    v.extend_from_slice(b"\n--stderr--\n");
    v.extend_from_slice(stderr);
    v
}

/// One row of the serialised attested manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestedRow {
    /// The gate id (matched against [`Band::MakeItReal`]'s required rows).
    pub id: String,
    /// `true` iff this row is a proven PASS.
    pub passed: bool,
    /// The ISO-8601 date the verdict was asserted.
    pub date: String,
    /// The proof line (for a PASS) or the claimed-not-proven reason (for a RED).
    pub detail: String,
    /// `Some(..)` iff `passed` — the content-hash attestation binding this green to its output.
    pub attestation: Option<RowAttestation>,
}

/// The serialisable attested scorecard — the machine-readable artifact the make-it-real gate
/// writes and re-validates (`testing/scorecards/make-it-real.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestedScorecard {
    /// The band display name (always `"make-it-real (evidence spine)"`).
    pub band: String,
    /// The ISO-8601 date the manifest was generated.
    pub generated_on: String,
    /// The recorded rows, in required-row order.
    pub rows: Vec<AttestedRow>,
}

/// Why the make-it-real gate is RED (one entry per failing required row). Carried by
/// [`GateVerdict`] so the binary can print exactly what failed closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateProblem {
    /// A required row id is absent from the manifest (the drop-a-row half of the ratchet).
    Missing(String),
    /// A required row is recorded but not a proven PASS (claimed-not-proven).
    NotProven { id: String, reason: String },
    /// A PASS row's attestation is absent or its recomputed hash ≠ the stored hash (a tamper).
    Tampered { id: String, detail: String },
    /// A PASS row's date is older than the freshness window (stale evidence).
    Stale { id: String, date: String, age_days: i64 },
}

impl std::fmt::Display for GateProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateProblem::Missing(id) => {
                write!(f, "MISSING required row `{id}` (the ratchet re-reds a dropped row)")
            }
            GateProblem::NotProven { id, reason } => {
                write!(f, "claimed-not-proven `{id}` — {reason}")
            }
            GateProblem::Tampered { id, detail } => write!(f, "TAMPER `{id}` — {detail}"),
            GateProblem::Stale { id, date, age_days } => write!(
                f,
                "STALE `{id}` — attested {date} ({age_days}d old, older than the freshness window)"
            ),
        }
    }
}

/// The make-it-real gate verdict. Green iff `problems` is empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateVerdict {
    /// Every required row that left the gate red. Empty ⇒ GREEN.
    pub problems: Vec<GateProblem>,
}

impl GateVerdict {
    /// GREEN iff there are no problems (every required row is present, proven, hash-valid, fresh).
    pub fn is_green(&self) -> bool {
        self.problems.is_empty()
    }
}

impl AttestedScorecard {
    /// Build an attested manifest from a recorded [`Scorecard`] (band [`Band::MakeItReal`]). Each
    /// PASS carries the attestation the gate computed when it ran the proof command; each RED
    /// carries its claimed-not-proven reason. Panics if `card.band` is not `MakeItReal` (the
    /// attested manifest is the make-it-real gate's artifact, not a band gate's).
    pub fn from_scorecard(card: &Scorecard, generated_on: &str) -> Self {
        assert_eq!(
            card.band,
            Band::MakeItReal,
            "AttestedScorecard is only for the make-it-real gate band"
        );
        let rows = card
            .rows
            .iter()
            .map(|r| match &r.verdict {
                RowVerdict::Pass { proof } => AttestedRow {
                    id: r.id.clone(),
                    passed: true,
                    date: r.date.clone(),
                    detail: proof.clone(),
                    attestation: r.attestation.clone(),
                },
                RowVerdict::ClaimedNotProven { reason } => AttestedRow {
                    id: r.id.clone(),
                    passed: false,
                    date: r.date.clone(),
                    detail: reason.clone(),
                    attestation: None,
                },
            })
            .collect();
        AttestedScorecard {
            band: Band::MakeItReal.to_string(),
            generated_on: generated_on.to_string(),
            rows,
        }
    }

    /// Serialise to the pretty JSON manifest body.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("AttestedScorecard serialises")
    }

    /// Parse a JSON manifest body. `Err` on malformed JSON (a corrupt manifest is itself a fail-
    /// closed condition — the caller reds the gate).
    pub fn from_json(s: &str) -> Result<Self, String> {
        serde_json::from_str(s).map_err(|e| format!("attested manifest is not valid JSON: {e}"))
    }

    /// **The make-it-real gate verdict — RED BY DEFAULT, fails closed.** Validates this manifest
    /// against the FROZEN required-row set of [`Band::MakeItReal`] as of `today`, with `max_age_days`
    /// freshness. The returned [`GateVerdict`] is GREEN iff EVERY required row is present, a proven
    /// PASS, hash-valid (untampered), and fresh. Any missing / not-proven / tampered / stale row is
    /// a problem and the gate is RED. (A row present in the manifest but NOT required is ignored —
    /// the gate is keyed off the required set, so padding the manifest cannot help.)
    pub fn validate(&self, today: &str, max_age_days: i64) -> GateVerdict {
        let mut problems = Vec::new();
        let today_days = days_from_iso(today);
        for required in Band::MakeItReal.required_rows() {
            let Some(row) = self.rows.iter().find(|r| r.id == required.id) else {
                problems.push(GateProblem::Missing(required.id.to_string()));
                continue;
            };
            if !row.passed {
                problems.push(GateProblem::NotProven {
                    id: row.id.clone(),
                    reason: row.detail.clone(),
                });
                continue;
            }
            // A PASS must carry a hash-valid attestation.
            match &row.attestation {
                None => problems.push(GateProblem::Tampered {
                    id: row.id.clone(),
                    detail:
                        "row marked PASS but carries NO attestation (a verdict flipped to PASS \
                         without binding evidence)"
                            .to_string(),
                }),
                Some(att) => {
                    if let Err(detail) = att.verify(&row.id, &row.date) {
                        problems.push(GateProblem::Tampered {
                            id: row.id.clone(),
                            detail,
                        });
                        continue;
                    }
                }
            }
            // Freshness (only meaningful for a hash-valid PASS).
            if let (Some(t), Some(d)) = (today_days, days_from_iso(&row.date)) {
                let age = t - d;
                if age < 0 || age > max_age_days {
                    problems.push(GateProblem::Stale {
                        id: row.id.clone(),
                        date: row.date.clone(),
                        age_days: age,
                    });
                }
            }
        }
        GateVerdict { problems }
    }
}

/// Parse an ISO-8601 `YYYY-MM-DD` date into days since the Unix epoch (Howard Hinnant's
/// days-from-civil, the inverse of the algorithm in [`crate::scorecard::today_iso`]). Returns
/// `None` on a malformed date so a corrupt date reds the gate rather than panicking.
fn days_from_iso(s: &str) -> Option<i64> {
    let mut parts = s.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    Some(era * 146_097 + doe - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_from_iso_inverts_today_iso_algorithm() {
        // Known anchors against the scorecard module's forward algorithm.
        assert_eq!(days_from_iso("1970-01-01"), Some(0));
        assert_eq!(days_from_iso("2000-01-01"), Some(10_957));
        assert_eq!(days_from_iso("2026-06-26"), Some(20_630));
        assert_eq!(days_from_iso("not-a-date"), None);
        assert_eq!(days_from_iso("2026-13-01"), None);
    }

    #[test]
    fn attestation_round_trips_and_detects_tamper() {
        let argv = vec!["test".to_string(), "-p".to_string(), "x".to_string()];
        let att = RowAttestation::compute("MR-004", &argv, "2026-06-26", b"all green\nexit=0");
        assert!(att.verify("MR-004", "2026-06-26").is_ok());
        // Changing the output digest without recomputing the hash is a tamper.
        let mut tampered = att.clone();
        tampered.output_digest = blake3::hash(b"different").to_hex().to_string();
        assert!(tampered.verify("MR-004", "2026-06-26").is_err());
        // Re-targeting the attestation at a different id/date is a tamper.
        assert!(att.verify("MR-009", "2026-06-26").is_err());
        assert!(att.verify("MR-004", "2026-06-27").is_err());
    }
}

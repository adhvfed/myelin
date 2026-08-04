use serde::{Deserialize, Serialize};

use crate::scorecard::{Band, RowResult, RowVerdict, Scorecard};

const DOMAIN: &str = "myelin.make-it-real.attestation.v1";

pub const DEFAULT_MAX_AGE_DAYS: i64 = 30;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowAttestation {
    pub argv: Vec<String>,
    pub output_digest: String,
    pub hash: String,
}

impl RowAttestation {
    pub fn compute(id: &str, argv: &[String], date: &str, output: &[u8]) -> Self {
        let output_digest = blake3::hash(output).to_hex().to_string();
        let hash = attestation_hash(id, true, date, argv, &output_digest);
        RowAttestation {
            argv: argv.to_vec(),
            output_digest,
            hash,
        }
    }

    pub fn verify(&self, id: &str, date: &str) -> Result<(), String> {
        let recomputed = attestation_hash(id, true, date, &self.argv, &self.output_digest);
        if recomputed == self.hash {
            Ok(())
        } else {
            Err(format!(
                "attestation hash MISMATCH for row `{id}` - stored {} but recomputed {} \
                 (the verdict/output was hand-edited without re-attesting; the gate reds it)",
                short(&self.hash),
                short(&recomputed),
            ))
        }
    }
}

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

fn short(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
}

pub fn output_bytes(exit_code: i32, stdout: &[u8], stderr: &[u8]) -> Vec<u8> {
    let mut v = format!("exit={exit_code}\n").into_bytes();
    v.extend_from_slice(b"--stdout--\n");
    v.extend_from_slice(stdout);
    v.extend_from_slice(b"\n--stderr--\n");
    v.extend_from_slice(stderr);
    v
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestedRow {
    pub id: String,
    pub passed: bool,
    pub date: String,
    pub detail: String,
    pub attestation: Option<RowAttestation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestedScorecard {
    pub band: String,
    pub generated_on: String,
    pub rows: Vec<AttestedRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateProblem {
    Malformed { id: String, detail: String },
    Missing(String),
    NotProven { id: String, reason: String },
    Tampered { id: String, detail: String },
    Stale {
        id: String,
        date: String,
        age_days: i64,
    },
}

impl std::fmt::Display for GateProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateProblem::Malformed { id, detail } => {
                write!(f, "MALFORMED `{id}` - {detail}")
            }
            GateProblem::Missing(id) => {
                write!(
                    f,
                    "MISSING required row `{id}` (the ratchet re-reds a dropped row)"
                )
            }
            GateProblem::NotProven { id, reason } => {
                write!(f, "claimed-not-proven `{id}` - {reason}")
            }
            GateProblem::Tampered { id, detail } => write!(f, "TAMPER `{id}` - {detail}"),
            GateProblem::Stale { id, date, age_days } => write!(
                f,
                "STALE `{id}` - attested {date} ({age_days}d old, older than the freshness window)"
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateVerdict {
    pub problems: Vec<GateProblem>,
}

impl GateVerdict {
    pub fn is_green(&self) -> bool {
        self.problems.is_empty()
    }
}

impl AttestedScorecard {
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

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("AttestedScorecard serialises")
    }

    pub fn render_markdown(&self) -> String {
        let mut card = Scorecard::new(Band::MakeItReal);
        for row in &self.rows {
            if row.passed {
                let result = match &row.attestation {
                    Some(attestation) => RowResult::pass_attested(
                        &row.id,
                        &row.detail,
                        &row.date,
                        attestation.clone(),
                    ),
                    None => RowResult::pass(&row.id, &row.detail, &row.date),
                };
                card.record(result);
            } else {
                card.record(RowResult::claimed_not_proven(
                    &row.id,
                    &row.detail,
                    &row.date,
                ));
            }
        }
        card.render_markdown(&self.generated_on)
    }

    pub fn from_json(s: &str) -> Result<Self, String> {
        serde_json::from_str(s).map_err(|e| format!("attested manifest is not valid JSON: {e}"))
    }

    pub fn validate(&self, today: &str, max_age_days: i64) -> GateVerdict {
        let mut problems = Vec::new();
        let today_days = days_from_iso(today);
        if self.band != Band::MakeItReal.to_string() {
            problems.push(GateProblem::Malformed {
                id: "manifest.band".to_string(),
                detail: format!("expected `{}`, got `{}`", Band::MakeItReal, self.band),
            });
        }
        if today_days.is_none() {
            problems.push(GateProblem::Malformed {
                id: "validation.today".to_string(),
                detail: format!("`{today}` is not a canonical calendar date"),
            });
        }
        match (today_days, days_from_iso(&self.generated_on)) {
            (_, None) => problems.push(GateProblem::Malformed {
                id: "manifest.generated_on".to_string(),
                detail: format!("`{}` is not a canonical calendar date", self.generated_on),
            }),
            (Some(today), Some(generated)) => {
                let age = today - generated;
                if age < 0 || age > max_age_days {
                    problems.push(GateProblem::Stale {
                        id: "manifest.generated_on".to_string(),
                        date: self.generated_on.clone(),
                        age_days: age,
                    });
                }
            }
            (None, Some(_)) => {}
        }
        let required_rows = Band::MakeItReal.required_rows();
        for row in &self.rows {
            if !required_rows.iter().any(|required| required.id == row.id) {
                problems.push(GateProblem::Malformed {
                    id: row.id.clone(),
                    detail: "row is not in the frozen make-it-real registry".to_string(),
                });
            }
        }
        for required in Band::MakeItReal.required_rows() {
            let matches = self
                .rows
                .iter()
                .filter(|row| row.id == required.id)
                .collect::<Vec<_>>();
            if matches.len() > 1 {
                problems.push(GateProblem::Malformed {
                    id: required.id.to_string(),
                    detail: format!("duplicate required row appears {} times", matches.len()),
                });
                continue;
            }
            let Some(row) = matches.first().copied() else {
                problems.push(GateProblem::Missing(required.id.to_string()));
                continue;
            };
            let row_days = days_from_iso(&row.date);
            if row_days.is_none() {
                problems.push(GateProblem::Malformed {
                    id: row.id.clone(),
                    detail: format!("row date `{}` is not a canonical calendar date", row.date),
                });
            }
            if !row.passed {
                problems.push(GateProblem::NotProven {
                    id: row.id.clone(),
                    reason: row.detail.clone(),
                });
                continue;
            }
            match &row.attestation {
                None => problems.push(GateProblem::Tampered {
                    id: row.id.clone(),
                    detail:
                        "row marked PASS but carries NO attestation (a verdict flipped to PASS \
                         without binding evidence)"
                            .to_string(),
                }),
                Some(att) => {
                    let expected = required
                        .proof_command
                        .iter()
                        .map(|value| value.to_string())
                        .collect::<Vec<_>>();
                    if att.argv != expected {
                        problems.push(GateProblem::Tampered {
                            id: row.id.clone(),
                            detail: format!(
                                "attested argv {:?} does not equal frozen proof command {:?}",
                                att.argv, expected
                            ),
                        });
                        continue;
                    }
                    if let Err(detail) = att.verify(&row.id, &row.date) {
                        problems.push(GateProblem::Tampered {
                            id: row.id.clone(),
                            detail,
                        });
                        continue;
                    }
                }
            }
            if let (Some(t), Some(d)) = (today_days, row_days) {
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

fn days_from_iso(s: &str) -> Option<i64> {
    if s.len() != 10 || s.as_bytes().get(4) != Some(&b'-') || s.as_bytes().get(7) != Some(&b'-') {
        return None;
    }
    let mut parts = s.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&m) {
        return None;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days_in_month = match m {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=days_in_month).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_from_iso_inverts_today_iso_algorithm() {
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
        let mut tampered = att.clone();
        tampered.output_digest = blake3::hash(b"different").to_hex().to_string();
        assert!(tampered.verify("MR-004", "2026-06-26").is_err());
        assert!(att.verify("MR-009", "2026-06-26").is_err());
        assert!(att.verify("MR-004", "2026-06-27").is_err());
    }
}

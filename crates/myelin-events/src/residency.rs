//! # `residency` — residency-pin the Bus streams: no cross-region stream read path (EB-13 / P-090)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/event-bus.md`
//! §7.1–§7.3 (per-(tenant, subsystem) streams, **region-pinned**, no cross-region stream read
//! path — the residency-pin lint applies). `external-insights/04-hard-problems.md` §1 (residency:
//! region-pinning, **no cross-region query path**). **Contract:** `contract-index.md` row 12.4
//! (`residency_verify` — **CONSUMED**; every store reports the tenant's region, the no-global-pool
//! property attestable). Drill catalogue rows **CP-D3 / STOR-D5** (residency — the Bus's slice).
//!
//! ## What EB-13 adds (and what it does NOT duplicate)
//! EB-12 ([`crate::partition`]) made a stream live under the `(tenant, region)` partition key
//! ([`crate::partition::PartitionKey`]): the subject carries the tenant; the **region is the
//! cell's residency pin** — a stream is provisioned in exactly one cell, so the region is the
//! cell's, never a subject token. EB-12 deliberately stopped at the partition; EB-13 enforces the
//! pin:
//!
//! 1. **A stream is provisioned PINNED to its cell's region** ([`BusStreamResidency::provision`]) —
//!    the region is the **cell's** (threaded by the harness), never read from a request field
//!    (`@residency-write`: the residency-pin write-boundary leg applies — the pin is the cell's, not
//!    the caller's). A provision where `partition.region != cell.region` is **REJECTED** (a stream
//!    can only ever exist in its cell's region).
//! 2. **No cross-region stream READ path** ([`BusStreamResidency::authorize_read`]) — a read routed
//!    from a region ≠ the stream's pinned region is **REJECTED** ([`CrossRegionRead`]); the stream
//!    is only ever read in-region. 0 cross-region stream reads is the CP-D3 (Bus slice) green
//!    artifact.
//! 3. **The Bus's `residency_verify` CONSUMER side (contract 12.4)** — the Bus's stream
//!    provisioning REPORTS its region ([`BusStreamResidency::region_report`]) so the control-plane
//!    `residency_verify` (P-CP-09) can aggregate it into the no-global-pool signed attestation. The
//!    Bus is the `index/stream` half of the store set: it reports the tenant's region exactly like
//!    every other store, and a Bus stream serving a tenant in the wrong region would FAIL the
//!    attestation. This module owns the Bus's CALL of 12.4; it does NOT re-implement the
//!    aggregation/sign (that is the control plane's authority — one authority, EI-01 §7).
//! 4. **The `bus-residency` telemetry signal** ([`BusResidencySignal`]) — the aggregate, PII-free
//!    `(tenant, region, cross_region_reads)` the CP-D3 (Bus slice) drill asserts against
//!    (`cross_region_reads == 0` is the green artifact). Observability is part of the pass (EI-01
//!    §3).
//!
//! ## The residency-pin lint applies to this code
//! The substrate `residency-pin` lint (P-S11 → P-018, sharpened P-CP-03 → P-026) forbids
//! constructing a store/stream WITHOUT a pinned region and forbids a write that sources the region
//! from an untrusted REQUEST field instead of the harness-threaded CELL region. This module's
//! provisioning is the Bus's stream-construction site: every [`BusStreamResidency`] carries a
//! [`Region`] and the provision REJECTS a request-sourced region (the pin is the cell's). The
//! write-boundary marker `@residency-write` arms the lint's layer-3 leg on this file — the pin is
//! read from the harness-threaded `cell` parameter, never a request field, so the lint admits it.
//!
//! ## DB-free by construction (the binding policy floor)
//! The aggregation + the in-/out-of-region decision + the reports are pure logic — `cargo build
//! --workspace` stays DB-free. The LIVE proof that a real NATS JetStream stream provisioned in one
//! region is never read from another rides the `integration` feature (`tests/
//! integration_eb13_residency.rs`) against the docker-compose dev stack (registered in the infra
//! scorecard as `EB-D-RESIDENCY`, red-until-proven). The store-layer write-boundary that GUARANTEES
//! a stream only ever writes in its cell's region is the four-layer enforcement P-CP-12 / P-096
//! (STOR-D5); EB-13 owns the Bus's slice of that gate (the stream-read pin + the region report).

// @residency-write — the residency-pin write-boundary (layer-3) leg arms on this file: the stream's
// region is the CELL's (threaded by the harness), NEVER a request field. The provision below reads
// `cell` (the harness-threaded region), never `req.region`/`payload.region`/… so the lint admits.

use crate::partition::PartitionKey;
use myelin_tenancy::{Region, TenantId};

/// **Why a Bus stream residency operation was REFUSED (a loud refusal — NEVER a silent pass; EI-01
/// §3).** Either a stream was asked to provision in a region ≠ its cell's region (a stream can only
/// live in its cell) or a read was routed from a region ≠ the stream's pinned region (the
/// cross-region read path the residency pin forbids). Both carry the offending regions so the
/// refusal is named (architecture §7.1–§7.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResidencyError {
    /// **A stream's partition region ≠ the cell's region.** A stream is provisioned inside exactly
    /// one cell; asking to pin it to a different region than the cell it lives in is rejected (the
    /// stream cannot exist outside its cell). This is the provisioning write-boundary: the region is
    /// the cell's, never the caller's.
    WrongCellRegion {
        /// The tenant the stream is for (opaque id, PII-free).
        tenant: TenantId,
        /// The cell's region (the authoritative residency pin — harness-threaded).
        cell_region: Region,
        /// The (wrong) region the partition asked the stream to be pinned to.
        partition_region: Region,
    },
    /// **THE HEADLINE: a read was routed from a region ≠ the stream's pinned region.** The
    /// cross-region stream read path the residency pin forbids — a consumer in region B trying to
    /// read a stream pinned to region A. The read is REJECTED (no cross-region read path). 0 of
    /// these is the CP-D3 (Bus slice) green artifact.
    CrossRegionRead {
        /// The tenant the stream is for (opaque id, PII-free).
        tenant: TenantId,
        /// The region the stream is pinned to (where it lives).
        stream_region: Region,
        /// The (different) region the read was routed from.
        read_from_region: Region,
    },
}

impl std::fmt::Display for ResidencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResidencyError::WrongCellRegion {
                tenant,
                cell_region,
                partition_region,
            } => write!(
                f,
                "Bus stream residency REFUSED for tenant `{}`: the stream's partition pins region \
                 `{}` but the cell it lives in is region `{}` — a stream is provisioned inside \
                 exactly one cell and cannot exist outside its cell's region (the pin is the \
                 cell's, NOT the caller's; architecture §7.1). REFUSED (not a silent pass, EI-01 \
                 §3).",
                tenant.as_str(),
                partition_region.as_str(),
                cell_region.as_str(),
            ),
            ResidencyError::CrossRegionRead {
                tenant,
                stream_region,
                read_from_region,
            } => write!(
                f,
                "Bus stream residency REFUSED for tenant `{}`: a read routed from region `{}` may \
                 NOT read a stream pinned to region `{}` — there is no cross-region stream read \
                 path (residency: no cross-region query path, external-insights/04 §1). REFUSED \
                 (0 cross-region reads is the CP-D3 green artifact).",
                tenant.as_str(),
                read_from_region.as_str(),
                stream_region.as_str(),
            ),
        }
    }
}

impl std::error::Error for ResidencyError {}

/// **A residency-pinned Bus stream provisioning (architecture §7.1–§7.3, region-pinned).** A
/// per-(tenant, subsystem) stream provisioned inside exactly ONE cell, carrying the cell's
/// [`Region`] as its residency pin. It is the Bus's residency-pin enforcement point:
/// [`Self::authorize_read`] rejects any read routed from a different region (no cross-region read
/// path) and [`Self::region_report`] is the Bus's consumed-side report into the control-plane
/// `residency_verify` (contract 12.4).
///
/// The [`Region`] field is the pin the `residency-pin` lint requires on a stream-construction site;
/// it is the CELL's region (harness-threaded), never a request field (the write-boundary rule).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusStreamResidency {
    /// The tenant the stream is for (the partition's tenant; opaque id, PII-free).
    tenant: TenantId,
    /// The producing subsystem (the per-(tenant, subsystem) stream's subsystem token).
    subsystem: String,
    /// **The residency pin** — the cell's region the stream is provisioned in. A stream is read
    /// ONLY from this region (no cross-region read path); reported into `residency_verify`.
    region: Region,
}

impl BusStreamResidency {
    /// **Provision a per-(tenant, subsystem) stream PINNED to the CELL's region (architecture
    /// §7.1).** `partition` is the `(tenant, region)` partition key (EB-12); `cell` is the cell's
    /// authoritative region, threaded by the harness (NOT a request field). The stream is pinned to
    /// `cell` — and the provision REJECTS a partition whose region ≠ the cell's region (a stream can
    /// only exist in its cell). The returned stream is read-only-in-region by construction.
    ///
    /// This is the residency-pin write-boundary: the region pinned on the stream is the harness's
    /// `cell`, so even though the partition carries a region, the stream NEVER takes its pin from a
    /// caller-controlled value that disagrees with the cell — a disagreement is a loud refusal.
    pub fn provision(
        partition: &PartitionKey,
        subsystem: &str,
        cell: &Region,
    ) -> Result<BusStreamResidency, ResidencyError> {
        // The pin is the CELL's region (harness-threaded). A partition asking for a different region
        // than the cell it lives in is a cross-region provision — REFUSED (the stream cannot exist
        // outside its cell). This is the write-boundary: `cell`, never the partition's value, is the
        // authority.
        if partition.region != *cell {
            return Err(ResidencyError::WrongCellRegion {
                tenant: partition.tenant.clone(),
                cell_region: cell.clone(),
                partition_region: partition.region.clone(),
            });
        }
        Ok(BusStreamResidency {
            tenant: partition.tenant.clone(),
            subsystem: subsystem.to_string(),
            region: cell.clone(),
        })
    }

    /// The tenant this stream serves (opaque id, PII-free).
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    /// The producing subsystem of this per-(tenant, subsystem) stream.
    pub fn subsystem(&self) -> &str {
        &self.subsystem
    }

    /// **The region this stream is residency-pinned to (where it lives).** A read is authorized ONLY
    /// from this region ([`Self::authorize_read`]); the report into `residency_verify` carries this.
    pub fn region(&self) -> &Region {
        &self.region
    }

    /// **Authorize a read routed from `read_from`: ACCEPT iff in-region, REJECT cross-region
    /// (architecture §7.1–§7.3; the no-cross-region-read-path rule).** A read routed from a region ≠
    /// the stream's pinned region is the cross-region stream read path the residency pin forbids — it
    /// is REJECTED with a loud [`ResidencyError::CrossRegionRead`] (never a silent cross-region
    /// read). 0 rejections-that-should-have-been-accepted AND 0 cross-region reads admitted is the
    /// CP-D3 (Bus slice) gate.
    pub fn authorize_read(&self, read_from: &Region) -> Result<(), ResidencyError> {
        if *read_from != self.region {
            return Err(ResidencyError::CrossRegionRead {
                tenant: self.tenant.clone(),
                stream_region: self.region.clone(),
                read_from_region: read_from.clone(),
            });
        }
        Ok(())
    }

    /// **The Bus's region report into `residency_verify` (contract 12.4, CONSUMED).** The Bus stream
    /// REPORTS the tenant's region — the no-global-pool attestation aggregates this with every other
    /// store's report (P-CP-09); a Bus stream serving a tenant in the wrong region would FAIL the
    /// attestation. The report is PII-free: a `(tenant, region)` pair, never personal data.
    ///
    /// The control plane owns the aggregation/sign (`residency_verify`) — this is the Bus's CALL of
    /// 12.4, the consumer side. The control plane's `ResidencyStoreClass` carries an `IndexStream`
    /// class the Bus reports under; here the report is the bare `(tenant, region)` the aggregator
    /// keys on (the Bus does not depend on the control-plane crate — the DAG forbids it — so the
    /// store-class tagging is done at the aggregation site).
    pub fn region_report(&self) -> BusRegionReport {
        BusRegionReport {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
        }
    }
}

/// **The Bus's region report for a tenant (contract 12.4, CONSUMED — the consumer side).** "For
/// tenant `T`, the Bus's stream served the events in region `R`." PII-free — a `(tenant, region)`
/// pair, never personal data. The control-plane `residency_verify` (P-CP-09) aggregates this with
/// every store's report into the no-global-pool signed attestation; a report whose region ≠ the
/// tenant's region of record FAILS the attestation (the Bus is just another store the no-global-pool
/// property covers).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusRegionReport {
    /// The tenant the report is for (opaque id, PII-free).
    pub tenant: TenantId,
    /// The region the Bus served the tenant's events in (== the stream's residency pin).
    pub region: Region,
}

impl BusRegionReport {
    /// Whether this report agrees with the tenant's region of record (the control plane's
    /// authoritative region). The aggregator's per-report check — a `false` here is the residency
    /// breach the no-global-pool attestation catches.
    pub fn matches_region_of_record(&self, region_of_record: &Region) -> bool {
        self.region == *region_of_record
    }
}

/// **The `bus-residency` telemetry signal (the CP-D3 / STOR-D5 Bus-slice artifact).** The aggregate,
/// PII-free residency posture of the Bus's streams: the tenant + region and the count of
/// cross-region reads that were REJECTED (the headline zero is `cross_region_reads_admitted == 0`).
/// Observability is part of the pass (EI-01 §3) — this is the signal the drill reads. PII-free:
/// opaque id + region code + an aggregate count, never per-subject data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusResidencySignal {
    /// The tenant the signal is for (opaque id, PII-free).
    pub tenant: TenantId,
    /// The region the Bus's streams for this tenant are pinned to.
    pub region: Region,
    /// **The headline zero** — how many cross-region reads were ADMITTED (leaked across a region
    /// boundary). `0` is the green `bus-residency` artifact; `> 0` reads RED (a residency breach).
    pub cross_region_reads_admitted: u32,
    /// How many cross-region reads were correctly REJECTED (the pin doing its job). Informational —
    /// a positive count means the pin actively blocked a cross-region read attempt.
    pub cross_region_reads_rejected: u32,
}

impl BusResidencySignal {
    /// The green `bus-residency` signal: 0 cross-region reads admitted (the pin held). `rejected` is
    /// how many cross-region read attempts the pin actively blocked (informational).
    pub fn green(tenant: TenantId, region: Region, rejected: u32) -> BusResidencySignal {
        BusResidencySignal {
            tenant,
            region,
            cross_region_reads_admitted: 0,
            cross_region_reads_rejected: rejected,
        }
    }

    /// The RED `bus-residency` signal: a cross-region read leaked (`admitted >= 1`) — a residency
    /// breach.
    pub fn red(tenant: TenantId, region: Region, admitted: u32) -> BusResidencySignal {
        BusResidencySignal {
            tenant,
            region,
            cross_region_reads_admitted: admitted,
            cross_region_reads_rejected: 0,
        }
    }

    /// The green artifact predicate: 0 cross-region reads admitted.
    pub fn is_green(&self) -> bool {
        self.cross_region_reads_admitted == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fr_par() -> Region {
        Region::new("fr-par")
    }
    fn eu_north() -> Region {
        Region::new("eu-north")
    }
    fn acme() -> TenantId {
        TenantId::from_token("01J0ACME")
    }
    fn partition(region: &Region) -> PartitionKey {
        PartitionKey::new(acme(), region.clone())
    }

    /// **A stream provisioned for (tenant, region=fr-par) is pinned to fr-par; a read routed from a
    /// DIFFERENT region is REJECTED — no cross-region stream read path (the EB-13 unit gate).**
    #[test]
    fn a_stream_pinned_to_one_region_rejects_a_read_from_another() {
        let cell = fr_par();
        let stream = BusStreamResidency::provision(&partition(&cell), "issue", &cell)
            .expect("a partition matching the cell region provisions");
        assert_eq!(stream.region(), &fr_par());

        // An in-region read is ACCEPTED.
        stream
            .authorize_read(&fr_par())
            .expect("an in-region read is authorized");

        // A cross-region read is REJECTED — the headline: no cross-region stream read path.
        let err = stream
            .authorize_read(&eu_north())
            .expect_err("a read from a different region MUST be rejected (no cross-region read path)");
        assert_eq!(
            err,
            ResidencyError::CrossRegionRead {
                tenant: acme(),
                stream_region: fr_par(),
                read_from_region: eu_north(),
            }
        );
        assert!(
            err.to_string().contains("no cross-region stream read path"),
            "loud reason: {err}"
        );
    }

    /// **A write/provision where partition.region ≠ cell.region is REJECTED (the write-boundary: the
    /// pin is the cell's, never the caller's).** A stream cannot be provisioned outside its cell.
    #[test]
    fn a_provision_with_a_region_disagreeing_with_the_cell_is_rejected() {
        let cell = fr_par();
        // The partition asks for eu-north, but the cell is fr-par — a cross-region provision.
        let err = BusStreamResidency::provision(&partition(&eu_north()), "issue", &cell)
            .expect_err("a partition region ≠ the cell region MUST be rejected (the stream lives in its cell)");
        assert_eq!(
            err,
            ResidencyError::WrongCellRegion {
                tenant: acme(),
                cell_region: fr_par(),
                partition_region: eu_north(),
            }
        );
        assert!(err.to_string().contains("the pin is the cell's"), "loud reason: {err}");
    }

    /// **The Bus reports its region into `residency_verify` (contract 12.4, consumed): a report
    /// matching the tenant's region of record is in-region; a mismatch is the breach the
    /// no-global-pool attestation catches.**
    #[test]
    fn the_bus_reports_its_region_into_residency_verify() {
        let cell = fr_par();
        let stream = BusStreamResidency::provision(&partition(&cell), "issue", &cell)
            .expect("provision");
        let report = stream.region_report();
        assert_eq!(report.tenant, acme());
        assert_eq!(report.region, fr_par());
        // In-region: the report agrees with the tenant's region of record (the green leg).
        assert!(report.matches_region_of_record(&fr_par()));
        // A mismatch is the residency breach (had the Bus served the tenant in eu-north).
        assert!(!report.matches_region_of_record(&eu_north()));
    }

    /// **The `bus-residency` telemetry signal: GREEN has 0 cross-region reads admitted; RED has
    /// `admitted >= 1`.** Observability is part of the pass (EI-01 §3).
    #[test]
    fn bus_residency_signal_green_and_red() {
        let green = BusResidencySignal::green(acme(), fr_par(), 3);
        assert_eq!(green.cross_region_reads_admitted, 0, "the green artifact is 0 admitted");
        assert_eq!(green.cross_region_reads_rejected, 3, "3 cross-region reads were blocked");
        assert!(green.is_green());

        let red = BusResidencySignal::red(acme(), fr_par(), 1);
        assert_eq!(red.cross_region_reads_admitted, 1, "a leaked cross-region read reads RED");
        assert!(!red.is_green());
    }

    /// **The CP-D3 (Bus slice) drill in-process: across a representative set of reads — some
    /// in-region, some cross-region — EVERY cross-region read is rejected (0 admitted), and the
    /// `bus-residency` signal is green.** This is the harness slice the live broker drill
    /// (`integration_eb13_residency.rs`) proves on real hardware.
    #[test]
    fn cp_d3_bus_slice_zero_cross_region_reads() {
        let cell = fr_par();
        let stream = BusStreamResidency::provision(&partition(&cell), "issue", &cell)
            .expect("provision");

        // A mixed read workload: in-region reads and cross-region read attempts.
        let reads = [
            fr_par(),   // in-region
            eu_north(), // cross-region — must be rejected
            fr_par(),   // in-region
            Region::new("us-east"), // cross-region — must be rejected
            eu_north(), // cross-region — must be rejected
        ];

        let mut admitted = 0u32;
        let mut rejected = 0u32;
        for read_from in &reads {
            match stream.authorize_read(read_from) {
                Ok(()) => {
                    // An admitted read MUST be in-region — assert it (a green that admitted a
                    // cross-region read would be a manufactured pass).
                    assert_eq!(read_from, stream.region(), "an admitted read must be in-region");
                }
                Err(ResidencyError::CrossRegionRead { read_from_region, .. }) => {
                    assert_ne!(&read_from_region, stream.region(), "a rejected read was cross-region");
                    rejected += 1;
                }
                Err(other) => panic!("unexpected error: {other}"),
            }
            // Count an admitted CROSS-region read as the breach (there must be none).
            if stream.authorize_read(read_from).is_ok() && read_from != stream.region() {
                admitted += 1;
            }
        }

        assert_eq!(admitted, 0, "THE GATE: 0 cross-region stream reads admitted (CP-D3 Bus slice)");
        assert_eq!(rejected, 3, "every cross-region read attempt was rejected");

        let signal = BusResidencySignal::green(acme(), fr_par(), rejected);
        assert!(signal.is_green(), "the bus-residency artifact is green (0 admitted)");
    }
}

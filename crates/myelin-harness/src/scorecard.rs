use std::fmt;

pub fn today_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    M0,
    M1Identity,
    Infra,
    M2Reactive,
    M3Producers,
    M4Consumers,
    M5World,
    M6Dogfood,
    MakeItReal,
}

impl fmt::Display for Band {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Band::M0 => write!(f, "M0"),
            Band::M1Identity => write!(f, "M1→M2 (Identity)"),
            Band::Infra => write!(f, "Infra (integration)"),
            Band::M2Reactive => write!(f, "M2 (reactive shared layer)"),
            Band::M3Producers => write!(f, "M3 (producer subsystems)"),
            Band::M4Consumers => write!(f, "M4 (consumer subsystems)"),
            Band::M5World => write!(f, "M5 (world-scale hardening)"),
            Band::M6Dogfood => write!(f, "M6 (dogfooding)"),
            Band::MakeItReal => write!(f, "make-it-real (evidence spine)"),
        }
    }
}

impl Band {
    pub fn required_rows(self) -> Vec<GateRow> {
        match self {
            Band::M0 => required_rows(),
            Band::M1Identity => id_m1_required_rows(),
            Band::Infra => infra_required_rows(),
            Band::M2Reactive => m2_required_rows(),
            Band::M3Producers => m3_required_rows(),
            Band::M4Consumers => m4_required_rows(),
            Band::M5World => m5_required_rows(),
            Band::M6Dogfood => m6_required_rows(),
            Band::MakeItReal => make_it_real_required_rows(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateRow {
    pub id: &'static str,
    pub title: &'static str,
    pub proof_command: &'static [&'static str],
    pub permanent: bool,
    pub floor: Option<&'static str>,
}

pub fn required_rows() -> Vec<GateRow> {
    vec![
        GateRow {
            id: "SUB-D1",
            title: "kill service between commit & publish → 0 ghost / 0 lost (outbox + dedup)",
            proof_command: &[
                "test",
                "-p",
                "myelin-events",
                "--test",
                "drills_sub_d1_bus_d4",
            ],
            permanent: true,
            floor: None,
        },
        GateRow {
            id: "SUB-D2",
            title: "drop broker mid-stream → 0 lost across reconnect; slow subject no HoL stall",
            proof_command: &[
                "test",
                "-p",
                "myelin-events",
                "--test",
                "drills_sub_d2_consumer",
            ],
            permanent: true,
            floor: None,
        },
        GateRow {
            id: "BUS-D4",
            title: "crash producer between state-commit and publish → emit-iff-committed",
            proof_command: &[
                "test",
                "-p",
                "myelin-storage",
                "--test",
                "sub_d1_bus_d4_coloc_drill",
            ],
            permanent: true,
            floor: None,
        },
        GateRow {
            id: "SUB-D5",
            title: "trip a downstream breaker → fail fast, honour Retry-After, no amplification",
            proof_command: &[
                "test",
                "-p",
                "myelin-client",
                "--test",
                "sub_d5_retry_storm",
            ],
            permanent: false,
            floor: None,
        },
        GateRow {
            id: "SUB-D7",
            title: "cross-tenant read via path≠token → 0 misroute; tenant-predicate lint catches",
            proof_command: &[
                "test",
                "-p",
                "myelin-substrate",
                "--test",
                "drill_sub_d7_idor",
            ],
            permanent: false,
            floor: None,
        },
        GateRow {
            id: "SUB-D8",
            title: "agent→agent loop → depth ceiling + shared-root tripwire + bounded pool halt",
            proof_command: &[
                "test",
                "-p",
                "myelin-substrate",
                "--test",
                "drill_sub_d8_causal_loop",
            ],
            permanent: false,
            floor: None,
        },
        GateRow {
            id: "SUB-D9",
            title: "kill a critical dependency → not-ready + sheds; no liveness restart-storm",
            proof_command: &[
                "test",
                "-p",
                "myelin-substrate",
                "--test",
                "drill_sub_d9_liveness_readiness",
            ],
            permanent: false,
            floor: None,
        },
        GateRow {
            id: "lints",
            title: "the twelve architecture lints - each red fixture rejects + green admits",
            proof_command: &["run", "-p", "myelin-lints", "--bin", "lint-gate"],
            permanent: false,
            floor: None,
        },
        GateRow {
            id: "lint-fixtures",
            title: "the lint fixture matrix + the CI-gate self-test (red fixture ⇒ non-zero)",
            proof_command: &["test", "-p", "myelin-lints"],
            permanent: false,
            floor: None,
        },
        GateRow {
            id: "contract-coverage",
            title: "the contract-coverage scanner - no falsely-claimed/dropped/un-named row",
            proof_command: &["run", "-p", "myelin-lints", "--bin", "contract-coverage"],
            permanent: false,
            floor: None,
        },
        GateRow {
            id: "harness-self-test",
            title: "the harness injects a fault and reads one telemetry assertion green",
            proof_command: &[
                "test",
                "-p",
                "myelin-harness",
                "drills::tests::harness_self_test",
            ],
            permanent: false,
            floor: None,
        },
    ]
}

pub fn id_m1_required_rows() -> Vec<GateRow> {
    fn drill(id: &'static str, title: &'static str, target: &'static str) -> GateRow {
        GateRow {
            id,
            title,
            proof_command: id_drill_argv(target),
            permanent: false,
            floor: None,
        }
    }
    vec![
        drill(
            "ID-D3",
            "cross-tenant check/list/read via path spoof → 0 cross-tenant tuples readable",
            "drill_id_d3_cross_tenant",
        ),
        drill(
            "ID-D2",
            "break Id dep → authenticated traffic survives on the coarse fail-static cache; just-revoked still denied (zookie bypass)",
            "drill_id_d2_fail_static",
        ),
        drill(
            "ID-D1",
            "SCIM-disable → every surface denies within N = 5 min; cache+token+denylist ≤ W; stale re-grant 0",
            "drill_id_d1_revocation",
        ),
        drill(
            "ID-D4",
            "confidential object ABSENT from any list_objects for an unauthorized viewer, incl. the Filter-lowered S8 JOIN (zero-escape == 0)",
            "drill_id_d4_zero_escape",
        ),
        drill(
            "ID-D7",
            "revoke then re-read with the post-revoke zookie → no stale allow (watermark honoured)",
            "drill_id_d7_watermark",
        ),
        drill(
            "ID-D5",
            "adversarial delegation confined to agent.policy ∩ delegation ∩ tenant.policy (intersection proof; 0 escapes)",
            "drill_id_d5_delegation",
        ),
        drill(
            "ID-D6",
            "kill a run mid-flight → per-run token revoked + auto-expires within run-life ≤ W (revocation lag ≤ W)",
            "drill_id_d6_run_token",
        ),
        drill(
            "ID-D8",
            "restore to a consistent point → no resurrected grants past an erasure; post-restore re-erasure receipt emitted",
            "drill_id_d8_re_erasure",
        ),
        GateRow {
            id: "contract-coverage",
            title: "the contract-coverage scanner re-affirms the 4.1–4.11 CDC pairs are all present (the coverage gate)",
            proof_command: &["run", "-p", "myelin-lints", "--bin", "contract-coverage"],
            permanent: false,
            floor: None,
        },
    ]
}

fn id_drill_argv(target: &'static str) -> &'static [&'static str] {
    match target {
        "drill_id_d1_revocation" => &[
            "test",
            "-p",
            "myelin-identity-service",
            "--test",
            "drill_id_d1_revocation",
        ],
        "drill_id_d2_fail_static" => &[
            "test",
            "-p",
            "myelin-identity-service",
            "--test",
            "drill_id_d2_fail_static",
        ],
        "drill_id_d3_cross_tenant" => &[
            "test",
            "-p",
            "myelin-identity-service",
            "--test",
            "drill_id_d3_cross_tenant",
        ],
        "drill_id_d4_zero_escape" => &[
            "test",
            "-p",
            "myelin-identity-service",
            "--test",
            "drill_id_d4_zero_escape",
        ],
        "drill_id_d5_delegation" => &[
            "test",
            "-p",
            "myelin-identity-service",
            "--test",
            "drill_id_d5_delegation",
        ],
        "drill_id_d6_run_token" => &[
            "test",
            "-p",
            "myelin-identity-service",
            "--test",
            "drill_id_d6_run_token",
        ],
        "drill_id_d7_watermark" => &[
            "test",
            "-p",
            "myelin-identity-service",
            "--test",
            "drill_id_d7_watermark",
        ],
        "drill_id_d8_re_erasure" => &[
            "test",
            "-p",
            "myelin-identity-service",
            "--test",
            "drill_id_d8_re_erasure",
        ],
        other => panic!("unknown Id-M1 drill target `{other}` - the proof-command table is frozen"),
    }
}

pub fn infra_required_rows() -> Vec<GateRow> {
    vec![
        GateRow {
            id: "STOR-D-OUTBOX",
            title: "outbox no-loss under crash → 0 lost / 0 ghost (real PG + real NATS JetStream)",
            proof_command: &[
                "test",
                "-p",
                "myelin-storage",
                "--features",
                "integration",
                "--test",
                "stage3_drills",
                "drill1_outbox_no_loss_under_crash",
            ],
            permanent: true,
            floor: None,
        },
        GateRow {
            id: "STOR-D-RESTORE",
            title: "restore-verify cross-seam → rows⟷blobs⟷bus-offset consistent (real PG + real RustFS)",
            proof_command: &[
                "test",
                "-p",
                "myelin-storage",
                "--features",
                "integration",
                "--test",
                "stage3_drills",
                "drill2_restore_verify_cross_seam",
            ],
            permanent: true,
            floor: None,
        },
        GateRow {
            id: "STOR-D-RLS",
            title: "(tenant, region) RLS isolation → cross-tenant leak = 0 (DB-enforced, NOBYPASSRLS role)",
            proof_command: &[
                "test",
                "-p",
                "myelin-storage",
                "--features",
                "integration",
                "--test",
                "stage3_drills",
                "drill3_tenant_region_rls_isolation",
            ],
            permanent: false,
            floor: None,
        },
        GateRow {
            id: "ID-D-REBAC",
            title: "ReBAC check/list_objects no-leak / no-N+1 → visible set exact, 1 reverse-index query (real PG tuples)",
            proof_command: &[
                "test",
                "-p",
                "myelin-storage",
                "--features",
                "integration",
                "--test",
                "stage3_drills",
                "drill4_rebac_check_list_objects_no_leak_no_n_plus_1",
            ],
            permanent: false,
            floor: None,
        },
        GateRow {
            id: "EB-D-PARTITION",
            title: "(EB-12) (tenant, region)-partitioned stream subject reaches the live broker; per-(tenant, subsystem) filter isolates one tenant's events (the bulkhead) - real NATS JetStream",
            proof_command: &[
                "test",
                "-p",
                "myelin-events",
                "--features",
                "integration",
                "--test",
                "integration_eb12_partition",
            ],
            permanent: true,
            floor: None,
        },
        GateRow {
            id: "EB-D-RESIDENCY",
            title: "(EB-13) residency-pinned Bus streams → a stream provisioned in region A has 0 cross-region read path (CP-D3 / STOR-D5 Bus slice) - real NATS JetStream",
            proof_command: &[
                "test",
                "-p",
                "myelin-events",
                "--features",
                "integration",
                "--test",
                "integration_eb13_residency",
            ],
            permanent: true,
            floor: None,
        },
        GateRow {
            id: "CP-D3/STOR-D5",
            title: "(P-CP-12) four-layer region-pin, store leg → an out-of-region write (row.region ≠ cell.region) REJECTED by the (tenant, region) RLS WITH CHECK; cross-region read = 0 rows (NOBYPASSRLS app role) - real PG",
            proof_command: &[
                "test",
                "-p",
                "myelin-storage",
                "--features",
                "integration",
                "--test",
                "stor_d5_cross_region_egress_drill",
            ],
            permanent: true,
            floor: None,
        },
        GateRow {
            id: "SRCH-D-LAYOUT",
            title: "(SRCH-P03) Search service-shell forward-only migration → search_index_directory CREATE applies forward-only; (tenant, region) PRIMARY KEY keys per-tenant index dirs (duplicate rejected) - real PG",
            proof_command: &[
                "test",
                "-p",
                "myelin-search",
                "--features",
                "integration",
                "--test",
                "integration_srch_p03_index_directory",
            ],
            permanent: true,
            floor: None,
        },
        GateRow {
            id: "SANDBOX-SMOKE",
            title: "hardened-container sandbox smoke → egress-deny + read-only-root + dropped caps asserted",
            proof_command: &[
                "test",
                "-p",
                "myelin-storage",
                "--features",
                "integration",
                "--test",
                "stage4_floor_smokes",
                "sandbox_escape_containerized_smoke",
            ],
            permanent: false,
            floor: Some(
                "the real-kernel SANDBOX-ESCAPE gate needs a real isolation kernel \
                 (gVisor / Firecracker microVM), not a Docker container - RED until run on one",
            ),
        },
        GateRow {
            id: "LOAD-10X-SMOKE",
            title: "10× containerized load smoke → myelin-harness LoadGenerator at 10× against the live PG+NATS stack survives",
            proof_command: &[
                "test",
                "-p",
                "myelin-storage",
                "--features",
                "integration",
                "--test",
                "stage4_floor_smokes",
                "load_10x_containerized_smoke",
            ],
            permanent: false,
            floor: Some(
                "the WORLD-SCALE 30× LOAD drill needs real hardware (a multi-node cluster), \
                 not a single dev box - RED until run on real hardware",
            ),
        },
    ]
}

pub fn m2_required_rows() -> Vec<GateRow> {
    fn row(id: &'static str, title: &'static str, cmd: &'static [&'static str]) -> GateRow {
        GateRow {
            id,
            title,
            proof_command: cmd,
            permanent: false,
            floor: None,
        }
    }
    vec![
        row(
            "BUS-D1",
            "dispatch reconnect → 0 lost across a broker drop on the reactive dispatch path",
            &["test", "-p", "myelin-query", "--test", "drills_bus_d1_dispatch_reconnect"],
        ),
        row(
            "BUS-D3",
            "dispatch replay → at-least-once redelivery is idempotent (no double-fire)",
            &["test", "-p", "myelin-query", "--test", "drills_bus_d3_dispatch_replay"],
        ),
        row(
            "BUS-D6",
            "dispatch loop guards → reactive automation cycle halts at the depth ceiling",
            &["test", "-p", "myelin-query", "--test", "drills_bus_d6_dispatch_loop_guards"],
        ),
        row(
            "BUS-D5",
            "reindex → a re-index pass is replay-safe; no stale/dup projection rows",
            &["test", "-p", "myelin-events", "--test", "drills_bus_d5_reindex"],
        ),
        row(
            "BUS-D8",
            "crypto-shred → erased-key payload is unrecoverable across the bus replay path",
            &["test", "-p", "myelin-events", "--test", "drills_bus_d8_crypto_shred"],
        ),
        row(
            "REF-CDC",
            "ArtifactRef provider mints canonical URN / consumer parses + rejects display projections (CDC 5.1)",
            &["test", "-p", "myelin-refs", "--test", "cdc_5_1_artifactref"],
        ),
        row(
            "SRCH-D1",
            "zero-leak keystone → a confidential doc never appears in any unauthorized search result set",
            &["test", "-p", "myelin-search", "--test", "drill_srch_d1_zero_escape_leak"],
        ),
        row(
            "SRCH-D2",
            "no stale grant → revoke then re-query → the just-revoked doc is gone (watermark honoured)",
            &["test", "-p", "myelin-search", "--test", "drill_srch_d2_no_stale_grant"],
        ),
        row(
            "SRCH-D3",
            "cross-tenant → a path-spoofed query reads 0 cross-tenant documents",
            &["test", "-p", "myelin-search", "--test", "drill_srch_d3_cross_tenant"],
        ),
        row(
            "SRCH-D4",
            "erasure → an erased doc is purged from the index; re-query returns 0 hits",
            &["test", "-p", "myelin-search", "--test", "drill_srch_d4_erasure"],
        ),
        row(
            "SRCH-D7",
            "freshness → a just-indexed doc is visible within the freshness bound (no lost write)",
            &["test", "-p", "myelin-search", "--test", "drill_srch_d7_freshness"],
        ),
        row(
            "NOTIF-D1",
            "notif drill D1 - mention fan-out delivers exactly the addressed recipients",
            &["test", "-p", "myelin-notif", "--test", "drill_notif_d1"],
        ),
        row(
            "NOTIF-D2",
            "notif drill D2 - read-state fan-out / dedup is consistent across surfaces",
            &["test", "-p", "myelin-notif", "--test", "drill_notif_d2"],
        ),
        row(
            "NOTIF-D3",
            "notif drill D3 - preference gating: a muted channel delivers 0",
            &["test", "-p", "myelin-notif", "--test", "drill_notif_d3"],
        ),
        row(
            "NOTIF-D4",
            "notif drill D4 - escalation honours the ladder without double-paging",
            &["test", "-p", "myelin-notif", "--test", "drill_notif_d4"],
        ),
        row(
            "NOTIF-D7",
            "notif drill D7 - cross-tenant: a notification never leaks across the tenant boundary",
            &["test", "-p", "myelin-notif", "--test", "drill_notif_d7"],
        ),
        row(
            "NOTIF-D8",
            "notif drill D8 - erasure: an erased subject's notifications are structurally purged",
            &["test", "-p", "myelin-notif", "--test", "drill_notif_d8"],
        ),
        row(
            "NOTIF-D9",
            "notif drill D9 - holder replay: redelivery after a crash is idempotent",
            &["test", "-p", "myelin-notif", "--test", "drill_notif_d9"],
        ),
        row(
            "NOTIF-D10",
            "notif drill D10 - delivery survives a consumer reconnect with 0 lost",
            &["test", "-p", "myelin-notif", "--test", "drill_notif_d10"],
        ),
        row(
            "NOTIF-D11",
            "notif drill D11 - inbox watch / list consistency under concurrent reads",
            &["test", "-p", "myelin-notif", "--test", "drill_notif_d11"],
        ),
        row(
            "NOTIF-snooze",
            "notif snooze → a snoozed notification resurfaces exactly once at the wake time",
            &["test", "-p", "myelin-notif", "--test", "drill_notif_snooze_resurface"],
        ),
        row(
            "AG-D1/2/3",
            "plan-then-apply pipeline: schema→cap→delegation→tenant→budget→HITL→apply→meter (CDC 8.2)",
            &["test", "-p", "myelin-agent-service", "--test", "cdc_8_2_apply_pipeline"],
        ),
        row(
            "AG-D5-batch",
            "per-effect HITL exactly-once (batch) - each effect gated by its own approval (CDC 8.2)",
            &["test", "-p", "myelin-agent-service", "--test", "cdc_8_2_hitl_batch"],
        ),
        row(
            "AG-D5-loop",
            "per-effect HITL exactly-once (loop) - re-entry never double-applies an approved effect (CDC 8.2)",
            &["test", "-p", "myelin-agent-service", "--test", "cdc_8_2_hitl_loop"],
        ),
        row(
            "AG-D7",
            "agent loop guards → a self-invoking agent halts at the depth ceiling / shared-root tripwire",
            &["test", "-p", "myelin-agent-service", "--test", "drills_ag_d7_loop_guards"],
        ),
        row(
            "AG-D8",
            "per-run identity/skeleton → each run drives the chained substrate path under its own run token (CDC 8.5)",
            &["test", "-p", "myelin-agent-service", "--test", "cdc_8_5_skeleton_loop"],
        ),
        row(
            "AG-D11",
            "runaway self-limiter → a reserve/settle runaway agent is rate-limited and halts",
            &["test", "-p", "myelin-agent-service", "--test", "drills_ag_d11_runaway_self_limiter"],
        ),
        GateRow {
            id: "AG-D4",
            title: "REAL-kernel escape gate → a real Firecracker microVM boots, runs the adversarial corpus, 0 escapes (proven-on-real-hardware)",
            proof_command: &[
                "test",
                "-p",
                "myelin-ci-sandbox",
                "--features",
                "integration",
                "--test",
                "escape_drill_test",
            ],
            permanent: true,
            floor: None,
        },
        row(
            "FLOW-D1",
            "durable replay → a workflow re-driven from its log lands on the same deterministic state",
            &["test", "-p", "myelin-flow", "--test", "drills_flow_d1_replay"],
        ),
        row(
            "FLOW-D3",
            "timer wheel → a durable timer fires exactly once at its deadline across a restart",
            &["test", "-p", "myelin-flow", "--test", "drills_flow_d3_timer_wheel"],
        ),
        row(
            "FLOW-D4-hitl",
            "multiday HITL → a workflow parked on human approval resumes correctly days later",
            &["test", "-p", "myelin-flow", "--test", "drills_flow_d4_multiday_hitl"],
        ),
        row(
            "FLOW-D4-per-effect",
            "per-effect durability → each effect is committed exactly once across replay",
            &["test", "-p", "myelin-flow", "--test", "drills_flow_d4_per_effect"],
        ),
        row(
            "FLOW-D5",
            "co-commit → state + emitted effect commit in the same transaction (emit-iff-committed)",
            &["test", "-p", "myelin-flow", "--test", "drills_flow_d5_cocommit"],
        ),
        row(
            "FLOW-D6",
            "reserve/settle → a reserved budget settles exactly once; a crash leaves no double-charge",
            &["test", "-p", "myelin-flow", "--test", "drills_flow_d6_reserve_settle"],
        ),
        row(
            "FLOW-D7",
            "loop safety → a self-scheduling workflow halts at the loop-safety ceiling",
            &["test", "-p", "myelin-flow", "--test", "drills_flow_d7_loop_safety"],
        ),
        row(
            "FLOW-mergeq",
            "merge queue → serialized merge-queue admission commits in order without a lost update",
            &["test", "-p", "myelin-flow", "--test", "drills_flow_merge_queue"],
        ),
        GateRow {
            id: "contract-coverage",
            title: "the contract-coverage scanner re-affirms the M2 CDC rows - no falsely-claimed/dropped row",
            proof_command: &["run", "-p", "myelin-lints", "--bin", "contract-coverage"],
            permanent: false,
            floor: None,
        },
    ]
}

pub fn m3_required_rows() -> Vec<GateRow> {
    fn row(id: &'static str, title: &'static str, cmd: &'static [&'static str]) -> GateRow {
        GateRow {
            id,
            title,
            proof_command: cmd,
            permanent: false,
            floor: None,
        }
    }
    vec![
        row(
            "GIT-D1",
            "hot-ref burst → concurrent pushes to one hot ref serialize without a lost update (ref-CAS holds under contention)",
            &["test", "-p", "myelin-git", "--test", "drills_git_d1_hot_ref_burst"],
        ),
        row(
            "GIT-D2",
            "erasure/pseudonymity → an erase reaches every holder; receive-pack commits stay pseudonymous with 0 residual PII",
            &["test", "-p", "myelin-git", "--test", "drills_git_d2_erase_reaches_every_holder"],
        ),
        row(
            "GIT-D3",
            "reindex parity → a cold re-index pass equals the live projection; no stale/dup/resurrected rows",
            &["test", "-p", "myelin-git", "--test", "drills_git_d3_reindex_parity"],
        ),
        row(
            "GIT-D7",
            "inline-thread anchor resolution → every comment anchor resolves across rebase/force-push with 0 mis-anchored",
            &["test", "-p", "myelin-git", "--test", "e2e_git_d7_anchor_resolution"],
        ),
        row(
            "GIT-D8",
            "front-door cross-tenant → a path-spoofed front-door request reads 0 cross-tenant repository data",
            &["test", "-p", "myelin-git", "--test", "drill_git_d8_front_door"],
        ),
        row(
            "GIT-D9",
            "receive-pack ref-CAS (silent-data-loss) → emit-iff-committed; crash-before/after/mid is 0 ghost / 0 lost / redeliver-once",
            &["test", "-p", "myelin-git", "--test", "drills_git_d9_receive_pack"],
        ),
        row(
            "GIT-D9-seam",
            "receive-pack consumer-leg seam → dead-letters foreign/malformed payloads, idempotent on dup, drops stale supersession",
            &["test", "-p", "myelin-git", "--test", "drills_git_d9_check_seam_consumer_leg"],
        ),
        GateRow {
            id: "GIT-D10",
            title: "check_status projection → supersession holds one current row per key, order-independent, idempotent on dup (real PG)",
            proof_command: &[
                "test",
                "-p",
                "myelin-git",
                "--features",
                "integration",
                "--test",
                "integration_git_d10_check_status_projection",
            ],
            permanent: false,
            floor: None,
        },
        row(
            "GIT-D11",
            "leak-free PR lists / SetExpr push-down → the list pre-filter lowers the authz predicate into the query (CDC 4.3)",
            &["test", "-p", "myelin-git", "--test", "cdc_4_3_git_list_pushdown"],
        ),
        GateRow {
            id: "GIT-D11-int",
            title: "PR list SetExpr JOIN → one query, 0 leak, tenant-scoped, just-revoked grant reflected (real PG)",
            proof_command: &[
                "test",
                "-p",
                "myelin-git",
                "--features",
                "integration",
                "--test",
                "integration_git_p26_list_pushdown",
            ],
            permanent: false,
            floor: None,
        },
        row(
            "KN-D1",
            "resume-cursor collab → a collaborator's resume cursor replays exactly to the last applied op (no gap / no double-apply)",
            &["test", "-p", "myelin-knowledge", "--test", "drill_kn_d1_resume_cursor"],
        ),
        GateRow {
            id: "KN-D3",
            title: "per-block CAS merge (NAMED FLOOR) → soft-lock + offline reconcile converge; per-block CAS rejects a stale write",
            proof_command: &["test", "-p", "myelin-knowledge", "--test", "drill_kn_d3_cas_merge_floor"],
            permanent: false,
            floor: Some(
                "the per-block CAS-merge row proves the M3 FLOOR (soft-locks + offline reconcile + \
                 per-block CAS); the full real-time CRDT/OT convergence is the named later follow-on \
                 (KN-P-collab, post-M3) - proven floor now, full convergence dated-deferred, never \
                 silently claimed closed (EI-01 §1)",
            ),
        },
        row(
            "KN-D4",
            "crypto-shred erase → subject authors PII → erase → 0 recoverable structured PII incl. vectors (DEK shred + tombstone)",
            &[
                "test",
                "-p",
                "myelin-knowledge",
                "--lib",
                "gdpr::erase_floor::tests::kn_d4_erase_subject_zero_recoverable_pii_including_vectors",
            ],
        ),
        GateRow {
            id: "KN-D5",
            title: "list push-down 0-leak → DB-row list+count SetExpr JOIN, 0 leak, 0 count-leak, just-revoked reflected (real PG)",
            proof_command: &[
                "test",
                "-p",
                "myelin-knowledge",
                "--features",
                "integration",
                "--test",
                "integration_kn_d5_list_pushdown",
            ],
            permanent: false,
            floor: None,
        },
        row(
            "KN-D6",
            "reindex parity (cold==live) → a wipe-replay cold rebuild equals the live projection; no resurrected erased state",
            &["test", "-p", "myelin-knowledge", "--test", "drill_kn_d6_reindex_parity"],
        ),
        GateRow {
            id: "KN-D7",
            title: "outbox → emit-iff-committed: N blocks ⇒ N rows; a rollback emits 0 (on real Postgres)",
            proof_command: &[
                "test",
                "-p",
                "myelin-knowledge",
                "--features",
                "integration",
                "--test",
                "integration_kn_d7_outbox",
            ],
            permanent: false,
            floor: None,
        },
        GateRow {
            id: "KN-D9",
            title: "flexible DB → JSONB GIN view filter/sort/group SetExpr conjoin, 0 leak, 0 count-leak (real PG)",
            proof_command: &[
                "test",
                "-p",
                "myelin-knowledge",
                "--features",
                "integration",
                "--test",
                "integration_kn_d9_flex_db",
            ],
            permanent: false,
            floor: None,
        },
        GateRow {
            id: "KN-D10",
            title: "rollup/formula → read-time rollup is permission-filtered (conjoin), 0 leak across the aggregate (real PG)",
            proof_command: &[
                "test",
                "-p",
                "myelin-knowledge",
                "--features",
                "integration",
                "--test",
                "integration_kn_d10_rollup",
            ],
            permanent: false,
            floor: None,
        },
        row(
            "KN-D11",
            "agent governance → the Knowledge agent-tool path runs the schema→cap→delegation→tenant→budget→HITL pipeline (CDC 8.2)",
            &["test", "-p", "myelin-knowledge", "--test", "cdc_8_2_knowledge_agent_governance"],
        ),
        row(
            "KN-D12",
            "agent-trace holder → every agent effect lands on its trace holder (the audit projection, CDC 8.8)",
            &["test", "-p", "myelin-knowledge", "--test", "cdc_8_8_knowledge_agent_trace"],
        ),
        row(
            "KN-D13",
            "OLTP RLS partition → the OLTP store enforces the (tenant, region) RLS partition; cross-partition read = 0 (CDC 11.1/12.1)",
            &["test", "-p", "myelin-knowledge", "--test", "cdc_11_1_12_1_knowledge_oltp_store_and_partition"],
        ),
        GateRow {
            id: "contract-coverage",
            title: "the contract-coverage scanner re-affirms the M3 CDC rows - no falsely-claimed/dropped row",
            proof_command: &["run", "-p", "myelin-lints", "--bin", "contract-coverage"],
            permanent: false,
            floor: None,
        },
    ]
}

pub fn m4_required_rows() -> Vec<GateRow> {
    fn row(id: &'static str, title: &'static str, cmd: &'static [&'static str]) -> GateRow {
        GateRow {
            id,
            title,
            proof_command: cmd,
            permanent: false,
            floor: None,
        }
    }
    vec![
        row(
            "CI-D9",
            "ci-pipeline determinism → a re-driven CI pipeline lands on the same deterministic plan/result (no nondeterministic step)",
            &["test", "-p", "myelin-ci-controlplane", "--test", "drills_ci_p15_ci_pipeline"],
        ),
        row(
            "CI-D1",
            "effectively-once → a CI trigger fires its effect exactly once across replay/redelivery (no double-run, no lost run)",
            &["test", "-p", "myelin-ci-controlplane", "--test", "drills_ci_p16_effectively_once"],
        ),
        row(
            "CI-D5",
            "reserve/settle parity → a reserved CI budget settles exactly once; a crash leaves no double-charge / no orphaned reservation",
            &["test", "-p", "myelin-ci-controlplane", "--test", "drills_ci_p17_reserve_settle_parity"],
        ),
        row(
            "CI-D8/GIT-D10",
            "seam gate → the check_status producer/consumer seam dead-letters foreign/malformed, idempotent on dup, drops stale supersession",
            &["test", "-p", "myelin-ci-controlplane", "--test", "drills_ci_p19_seam_gate"],
        ),
        GateRow {
            id: "CI-D11",
            title: "live-tail → reconstructed durable producer + Edge resume a committed prefix exactly, no lost/dup byte-range",
            proof_command: &[
                "test",
                "-p",
                "myelin-edge",
                "--features",
                "integration",
                "--test",
                "integration_ci_http_surface",
                "production_sink_and_edge_resume_exactly_after_both_services_are_severed",
            ],
            permanent: true,
            floor: None,
        },
        row(
            "CI-D6",
            "fork cache-poison → a fork PR cannot poison the trusted build cache (cache key scoped, untrusted writes quarantined)",
            &["test", "-p", "myelin-ci-controlplane", "--test", "drills_ci_p22_fork_cache_poison"],
        ),
        row(
            "CI-D4",
            "supply-chain fail-closed → an unverifiable/unsigned supply-chain artifact fails CLOSED (the build is denied, never admitted-by-default)",
            &["test", "-p", "myelin-ci-controlplane", "--test", "drills_ci_p23_supply_chain_fail_closed"],
        ),
        row(
            "CI-D7",
            "fork-no-secrets → a fork-originated CI run reads 0 tenant secrets (the untrusted-fork secret boundary holds)",
            &["test", "-p", "myelin-ci-controlplane", "--test", "drills_ci_p24_fork_no_secrets"],
        ),
        GateRow {
            id: "AG-D4/CI-T1",
            title: "prod-image escape re-confirm → the COMMITTED prod CI runner image boots a real Firecracker microVM, runs the adversarial corpus, 0 escapes (proven-on-real-hardware)",
            proof_command: &[
                "test",
                "-p",
                "myelin-ci-sandbox",
                "--features",
                "integration",
                "--test",
                "escape_drill_ci_committed_gate_reconfirm_test",
            ],
            permanent: true,
            floor: None,
        },
        GateRow {
            id: "STOR-D1/D2",
            title: "restore-verify on the CI stores → cross-seam OLTP↔blob↔index↔offset restore to a consistent point; RPO/RTO within bound, 0 loss (real PG + RustFS)",
            proof_command: &[
                "test",
                "-p",
                "myelin-ci-controlplane",
                "--features",
                "integration",
                "--test",
                "integration_ci_p27_restore_verify_ci_stores",
            ],
            permanent: true,
            floor: None,
        },
        row(
            "ISS-P06",
            "emit-iff-committed → an Issues write co-commits its outbox event in the same transaction; a rollback emits 0",
            &["test", "-p", "myelin-issues", "--test", "drill_iss_p06_emit_iff_committed"],
        ),
        row(
            "ISS-D2",
            "cost-bounding → an adversarial Issues query/automation is cost-bounded and cannot run unbounded work (the resource ceiling holds)",
            &["test", "-p", "myelin-issues", "--test", "drill_iss_d2_cost_bounding"],
        ),
        row(
            "ISS-D3",
            "setexpr zero-leak → an Issues list SetExpr push-down lowers the authz predicate into the query; 0 cross-tenant / unauthorized rows leak",
            &["test", "-p", "myelin-issues", "--test", "drill_iss_d3_setexpr_zero_leak"],
        ),
        row(
            "ISS-D4",
            "create-storm → a burst of concurrent issue creates serializes without a lost write / duplicate key collision",
            &["test", "-p", "myelin-issues", "--test", "drill_iss_d4_create_storm"],
        ),
        row(
            "ISS-D5",
            "reorder zero-clobber → concurrent rank/reorder ops converge without clobbering a sibling's position (no lost reorder)",
            &["test", "-p", "myelin-issues", "--test", "drill_iss_d5_reorder_zero_clobber"],
        ),
        row(
            "ISS-D6-calendar",
            "SLA business-calendar → an SLA timer honours the business calendar (working hours/holidays) - fires at the correct deadline",
            &["test", "-p", "myelin-issues", "--test", "drill_iss_d6_sla_business_calendar"],
        ),
        row(
            "ISS-D6-escalation",
            "SLA escalation → a breached SLA escalates up the ladder exactly once, without double-paging",
            &["test", "-p", "myelin-issues", "--test", "drill_iss_d6_sla_escalation"],
        ),
        row(
            "ISS-D7",
            "stateful trigger → a stateful Issues automation trigger fires deterministically on the right state transition (no spurious / missed fire)",
            &["test", "-p", "myelin-issues", "--test", "drill_iss_d7_stateful_trigger"],
        ),
        row(
            "ISS-D8-rollup",
            "rollup → a read-time Issues rollup is permission-filtered (conjoin); 0 leak across the aggregate",
            &["test", "-p", "myelin-issues", "--test", "drill_iss_d8_rollup"],
        ),
        row(
            "ISS-D8-olap",
            "OLAP feed → the Issues OLAP feed is consistent with the OLTP projection; no resurrected/erased rows in the analytics feed",
            &["test", "-p", "myelin-issues", "--test", "drill_iss_d8b_olap_feed"],
        ),
        row(
            "ISS-D9",
            "import → an Issues import round-trips deterministically; no dropped / duplicated / mis-mapped issue on re-import",
            &["test", "-p", "myelin-issues", "--test", "drill_iss_d9_import"],
        ),
        row(
            "ISS-D11",
            "erase-reaches-every-holder → an Issues erase reaches every holder (issue + comments + attachments + projections); 0 recoverable residual",
            &["test", "-p", "myelin-issues", "--test", "drill_iss_d11_erase_reaches_every_holder"],
        ),
        row(
            "ISS-D13",
            "board sync → the Issues board view stays consistent with the underlying issue state under concurrent moves (no lost/ghost card)",
            &["test", "-p", "myelin-issues", "--test", "drill_iss_d13_board_sync"],
        ),
        row(
            "CHAT-D5-unfurl",
            "unfurl no-leak → a chat link unfurl never leaks content the viewer cannot see (the unfurl is authz-filtered, 0 cross-tenant leak)",
            &["test", "-p", "myelin-chat", "--test", "drill_chat_d5_unfurl_no_leak"],
        ),
        row(
            "CHAT-D5-humanise",
            "humanise no-leak → message humanisation (key/name resolution) never leaks an unauthorized display name / identity",
            &["test", "-p", "myelin-chat", "--test", "drill_chat_d5_humanise_leak"],
        ),
        row(
            "CHAT-D6/D7/D18",
            "invalidation → an unfurl/cache invalidation propagates correctly (a stale unfurl is invalidated on source change; no stale read)",
            &["test", "-p", "myelin-chat", "--test", "drill_chat_d6_d7_d18_invalidation"],
        ),
        row(
            "CHAT-D8",
            "erasure cascade → a chat erase cascades to every holder (message body + DEK + mentions + index); 0 recoverable residual PII",
            &["test", "-p", "myelin-chat", "--test", "drill_chat_d8_erasure_cascade"],
        ),
        row(
            "CHAT-D9",
            "HITL exactly-once → a chat agent's human-approved effect applies exactly once across replay (no double-apply)",
            &["test", "-p", "myelin-chat", "--test", "drill_chat_d9_hitl_exactly_once"],
        ),
        row(
            "CHAT-D10",
            "HITL per-effect → each chat agent effect is gated by its own approval (per-effect HITL; an approval never blanket-approves a sibling)",
            &["test", "-p", "myelin-chat", "--test", "drill_chat_d10_hitl_per_effect"],
        ),
        row(
            "CHAT-D11",
            "search ACL → a chat search reads 0 messages the viewer cannot see (the search ACL pre-filter holds; revoke reflected)",
            &["test", "-p", "myelin-chat", "--test", "drill_chat_d11_search_acl"],
        ),
        row(
            "CHAT-D12",
            "read-state → chat read-state fan-out/dedup is consistent across surfaces (no lost/double read marker)",
            &["test", "-p", "myelin-chat", "--test", "drill_chat_d12_read_state"],
        ),
        row(
            "CHAT-D15",
            "reindex parity → a cold chat re-index pass equals the live projection; no stale/dup/resurrected message rows",
            &["test", "-p", "myelin-chat", "--test", "drill_chat_d15_reindex_parity"],
        ),
        row(
            "CHAT-D16",
            "streaming → the chat live-streaming path delivers ordered frames; resume after a drop replays exactly, no lost/dup frame",
            &["test", "-p", "myelin-chat", "--test", "drill_chat_d16_streaming"],
        ),
        row(
            "CHAT-D17",
            "explicit-first → a chat agent's explicit (human-issued) instruction takes precedence over an inferred one (the explicit-first ordering holds)",
            &["test", "-p", "myelin-chat", "--test", "drill_chat_d17_explicit_first"],
        ),
        GateRow {
            id: "contract-coverage",
            title: "the contract-coverage scanner re-affirms the M4 CDC rows - no falsely-claimed/dropped row",
            proof_command: &["run", "-p", "myelin-lints", "--bin", "contract-coverage"],
            permanent: false,
            floor: None,
        },
    ]
}

pub fn m5_required_rows() -> Vec<GateRow> {
    fn row(id: &'static str, title: &'static str, cmd: &'static [&'static str]) -> GateRow {
        GateRow {
            id,
            title,
            proof_command: cmd,
            permanent: false,
            floor: None,
        }
    }
    vec![
        row(
            "SUB-D3",
            "F6 surge → substrate 30× surge family: human lane within budget, agent lane sheds, cross-tenant impact 0",
            &["test", "-p", "myelin-substrate", "--test", "drill_sub_d3_surge_family"],
        ),
        row(
            "ID-D9",
            "F6 surge → Identity authz 30× surge: check/list path holds under load, agent sheds, cross-tenant 0",
            &["test", "-p", "myelin-identity-service", "--test", "drill_id_d9_authz_surge"],
        ),
        row(
            "BUS-D7",
            "F6 surge → Bus agent 30× surge: reactive dispatch holds, agent lane sheds, no cross-tenant amplification",
            &["test", "-p", "myelin-substrate", "--test", "drills_bus_d7_agent_surge"],
        ),
        row(
            "REF-D10",
            "F6 surge → Reference Graph 30× surge: resolution holds within budget, agent sheds, cross-tenant 0",
            &["test", "-p", "myelin-refs-service", "--test", "ref_d10_surge_drill"],
        ),
        row(
            "SRCH-D6",
            "F6 surge → Search 30× surge: query path within budget, agent sheds, 0 cross-tenant leak under load",
            &["test", "-p", "myelin-search", "--test", "drill_srch_d6_surge"],
        ),
        row(
            "NOTIF-D5",
            "F6 surge → Notifications 30× surge: fan-out within budget, agent sheds, cross-tenant impact 0",
            &["test", "-p", "myelin-notif", "--test", "drill_notif_d5"],
        ),
        row(
            "AG-D6",
            "F6 surge → Agent dispatch 30× surge: human lane within budget, agent dispatch sheds, cross-tenant 0",
            &["test", "-p", "myelin-agent-service", "--test", "ag_d6_dispatch_surge_drill"],
        ),
        row(
            "FLOW-D8",
            "F6 surge → Durable Workflow 30× surge: human lane within budget, agent lane sheds, cross-tenant 0",
            &["test", "-p", "myelin-flow", "--test", "drills_flow_d8_surge"],
        ),
        row(
            "GIT-D6",
            "F6 surge → Git clone 30× surge: clone p99 held within budget, agent sheds, cross-tenant 0",
            &["test", "-p", "myelin-git", "--test", "drill_git_d6_clone_surge"],
        ),
        row(
            "CI-D2",
            "F6 surge → CI 30× surge: pipeline admission within budget, agent lane sheds, cross-tenant 0",
            &["test", "-p", "myelin-ci-controlplane", "--test", "ci_d2_surge_drill"],
        ),
        row(
            "CHAT-D3/D4",
            "F6 surge → Chat agent 30× surge: human lane within budget, agent lane sheds, cross-tenant 0",
            &["test", "-p", "myelin-chat-gateway", "--test", "drill_chat_d3_agent_surge"],
        ),
        row(
            "GIT-D4",
            "monorepo ceiling → object-backed packs: large-monorepo ceiling documented + clone p99 held under object-backed packs",
            &["test", "-p", "myelin-git", "--test", "drills_git_d4_object_backed_packs"],
        ),
        row(
            "GIT-D5",
            "concurrent-merge linearizability under failover → ref-CAS linearizable, no split-brain, 0 lost merge",
            &["test", "-p", "myelin-git", "--test", "drills_git_d5_concurrent_merge_linearizability"],
        ),
        row(
            "KN-D1-re-green",
            "Yrs CRDT promotion re-green → KN-D1 resume-cursor collab holds ACROSS the CRDT boundary (no gap / no double-apply)",
            &["test", "-p", "myelin-knowledge", "--test", "drill_kn_p29_yrs_promotion"],
        ),
        row(
            "KN-D8",
            "all-hands doc surge → thousands of concurrent editors on one doc → the per-doc caps hold (no runaway / no lost edit)",
            &["test", "-p", "myelin-knowledge", "--test", "drill_kn_d8_allhands_surge"],
        ),
        row(
            "GA-D1",
            "full H1–H18 DSR fan-out at cell scale → an erasure reaches every holder family, 0 holders missed, per-holder receipt",
            &["test", "-p", "myelin-gdpr-service", "--test", "ga_d1_full_fanout_cell_scale"],
        ),
        row(
            "GA-D8",
            "multi-cell DSR fan-out → an erasure fans out across every member cell, per-cell receipt set complete, 0 cell missed",
            &["test", "-p", "myelin-gdpr-service", "--test", "ga_d8_multi_cell_fanout"],
        ),
        row(
            "CP-D7",
            "cell→cell live migration → a tenant migrates between cells with 0 loss (no lost/ghost write across the cutover)",
            &["test", "-p", "myelin-control-plane", "--test", "cp_d7_live_migration_drill"],
        ),
        row(
            "CP-D8",
            "cross-cell PII-free bridge → a cross-cell reference resolves via the PII-free CrossCellPointer bridge; 0 PII crosses",
            &["test", "-p", "myelin-control-plane", "--test", "cp_d8_cross_cell_bridge_drill"],
        ),
        row(
            "E2E-2",
            "agent-native flagship → CI-fail → triage agent → issue → chat → fix-PR drives end-to-end with its named green artifact",
            &["test", "-p", "myelin-agent-service", "--test", "drills_ag_p24_e2e2_flagship"],
        ),
        row(
            "E2E-4",
            "DSAR fan-out flagship → 0 holders missed; 0 recoverable PII incl. vectors incl. backups; certificate sealed (named green artifact)",
            &["test", "-p", "myelin-gdpr-service", "--test", "e2e_4_dsar_fanout_flagship"],
        ),
        row(
            "E2E-3",
            "spec-to-ship / reindex-parity (storage half) → a cold re-index pass equals the live projection; audit tamper detected (named artifact)",
            &["test", "-p", "myelin-storage", "--test", "e2e3_reindex_parity_drill"],
        ),
        row(
            "E2E-1",
            "PR context pane (git slice) → the whole-system PR-context wedge drives end-to-end with its named green artifact",
            &["test", "-p", "myelin-git", "--test", "e2e_wedge_git_p34"],
        ),
        GateRow {
            id: "STOR-D2-cell",
            title: "restore-verify at CELL SCALE under world-scale load → RPO/RTO within bound, 0 loss per cell (the permanent restore gate)",
            proof_command: &[
                "test",
                "-p",
                "myelin-storage",
                "--test",
                "stor_d2_d8_cell_scale_under_world_scale_load_drill",
            ],
            permanent: true,
            floor: None,
        },
        GateRow {
            id: "contract-coverage",
            title: "the contract-coverage scanner re-affirms the M5 CDC rows - no falsely-claimed/dropped row",
            proof_command: &["run", "-p", "myelin-lints", "--bin", "contract-coverage"],
            permanent: false,
            floor: None,
        },
    ]
}

pub fn m6_required_rows() -> Vec<GateRow> {
    fn row(id: &'static str, title: &'static str, cmd: &'static [&'static str]) -> GateRow {
        GateRow {
            id,
            title,
            proof_command: cmd,
            permanent: false,
            floor: None,
        }
    }
    vec![
        row(
            "ISS-D14",
            "Issues switch test → driven over the real surface with measured contrast + latency (not a feature-list read-off)",
            &["test", "-p", "myelin-issues", "--test", "iss_p37_switch_test_drill"],
        ),
        row(
            "CHAT-D19",
            "Chat switch test → driven over the real surface with measured contrast + latency (the lib switch_test module)",
            &["test", "-p", "myelin-chat", "--lib", "switch_test"],
        ),
        row(
            "GIT-OQ-12",
            "Git switch test → driven over the real surface with measured contrast + latency (not a feature-list read-off)",
            &["test", "-p", "myelin-git", "--test", "git_p35_switch_test_drill"],
        ),
        row(
            "KN-switch",
            "Knowledge switch test → driven over the real surface with measured contrast + latency (not a feature-list read-off)",
            &["test", "-p", "myelin-knowledge", "--test", "drill_kn_p34_switch_test"],
        ),
        row(
            "REF-switch",
            "Refs switch test → driven over the real surface with measured contrast + latency (not a feature-list read-off)",
            &["test", "-p", "myelin-refs-service", "--test", "ref_p29_switch_test_drill"],
        ),
        row(
            "SRCH-switch",
            "Search switch test → driven over the real surface with measured contrast + latency (not a feature-list read-off)",
            &["test", "-p", "myelin-search", "--test", "srch_p33_switch_test_drill"],
        ),
        row(
            "CI-P35-switch",
            "CI dogfood + switch test → the CI surface driven over the real surface with measured contrast + latency",
            &["test", "-p", "myelin-ci-controlplane", "--test", "ci_p35_dogfood_switch_test_drill"],
        ),
        row(
            "self-hosting-CI",
            "the self-hosting CI graph is green on the platform's own commits → the dogfood loop is live; every-incident-adds-a-drill",
            &["test", "-p", "myelin-harness", "--test", "self_hosting_ci_dogfood"],
        ),
        row(
            "FLOW-P29",
            "Flow dogfood → a flow incident files an issue and joins the permanent drill suite (the platform runs on its own work)",
            &["test", "-p", "myelin-flow", "--test", "flow_p29_dogfood_drill"],
        ),
        row(
            "AG-P26",
            "Agent fabric dogfood → a fabric incident files an issue and joins the permanent drill suite (the platform runs on its own work)",
            &["test", "-p", "myelin-agent-service", "--test", "ag_p26_dogfood_drill"],
        ),
        row(
            "CP-D23-selfhost",
            "Control-plane dogfood → Myelin self-hosts as one cell, residency-verify green, truth-up passes (the platform runs on its own work)",
            &["test", "-p", "myelin-control-plane", "--test", "cp_d23_dogfood_self_host_drill"],
        ),
        GateRow {
            id: "STOR-D37",
            title: "restore-verify on Myelin's own commits → a synthetic storage incident files an issue and joins the permanent drill suite (the permanent restore gate)",
            proof_command: &[
                "test",
                "-p",
                "myelin-storage",
                "--test",
                "stor_d37_dogfood_restore_verify_drill",
            ],
            permanent: true,
            floor: None,
        },
        row(
            "GA-P511",
            "self-served DSR → the dogfood DSR loop runs end-to-end self-hosting (the platform serves its own data-subject requests)",
            &["test", "-p", "myelin-gdpr-service", "--test", "ga_p511_dogfood_self_served_dsr_drill"],
        ),
        row(
            "REF-P28",
            "Refs dogfood → a refs incident files an issue and joins the permanent drill suite (the platform runs on its own work)",
            &["test", "-p", "myelin-refs-service", "--test", "ref_p28_dogfood_drill"],
        ),
        row(
            "SRCH-P33",
            "Search dogfood → a search incident files an issue and joins the permanent drill suite (the platform runs on its own work)",
            &["test", "-p", "myelin-search", "--test", "srch_p33_dogfood_drill"],
        ),
        row(
            "KN-P34",
            "Knowledge dogfood → the every-incident loop joins the permanent suite and re-runs green (the platform runs on its own work)",
            &["test", "-p", "myelin-knowledge", "--test", "drill_kn_p34_dogfood"],
        ),
        row(
            "GIT-P35",
            "Git dogfood → the every-incident loop joins the permanent suite and re-runs green (the platform runs on its own work)",
            &["test", "-p", "myelin-git", "--test", "git_p35_dogfood_drill"],
        ),
        row(
            "GA-truth-up",
            "truth-up pass → every PROVEN gate rests on a dated green artifact, never a doc claim (code-wins-over-docs, EI-01 §1)",
            &["test", "-p", "myelin-gdpr-service", "--test", "ga_p512_truth_up_pass"],
        ),
        GateRow {
            id: "contract-coverage",
            title: "the contract-coverage scanner re-affirms the M6 CDC rows - no falsely-claimed/dropped row",
            proof_command: &["run", "-p", "myelin-lints", "--bin", "contract-coverage"],
            permanent: false,
            floor: None,
        },
    ]
}

pub fn make_it_real_required_rows() -> Vec<GateRow> {
    fn row(id: &'static str, title: &'static str, cmd: &'static [&'static str]) -> GateRow {
        GateRow {
            id,
            title,
            proof_command: cmd,
            permanent: true,
            floor: None,
        }
    }
    vec![
        row(
            "MR-004",
            "production-graph ABSENCE ratchet at/under the committed baseline (no new Structural* / in-memory-durable / bare-tenant-pool site; the skeleton this gate rests on)",
            &["test", "-p", "myelin-lints", "--test", "production_graph_absence"],
        ),
        row(
            "MR-009",
            "durable-persistence verify across identity/events/control-plane/KMS-root: kill-9/restart + 3-instance consistency + no-in-memory scanner green",
            &[
                "test",
                "-p",
                "myelin-identity-service",
                "--features",
                "integration",
                "--test",
                "integration_mr009_kill9_durability",
                "--",
                "--nocapture",
                "--test-threads=1",
            ],
        ),
        row(
            "MR-010",
            "human/SSO auth real crypto (OIDC JWKS / SAML XML-DSig / WebAuthn / SSH) + the forged/expired/replayed NEGATIVE corpus rejects",
            &[
                "test",
                "-p",
                "myelin-identity-service",
                "--lib",
                "--",
                "--nocapture",
            ],
        ),
        row(
            "MR-011",
            "machine/capability tokens + DPoP (signed, attenuated, sender-constrained, revocable) + the forged/expired/replayed NEGATIVE corpus rejects",
            &[
                "test",
                "-p",
                "myelin-identity-service",
                "--features",
                "integration",
                "--lib",
                "--test",
                "integration_mr011_machine_token_revocation_durable",
                "--",
                "--nocapture",
            ],
        ),
        row(
            "MR-012",
            "Structural* verifiers/signers REMOVED from the production graph - the no-structural-crypto absence scanner is green-on-prod, red-on-fixture",
            &[
                "test",
                "-p",
                "myelin-lints",
                "--test",
                "production_graph_absence",
                "--",
                "--nocapture",
            ],
        ),
        row(
            "MR-013",
            "tenant isolation: SET LOCAL RLS + reset-on-release (set_config(..,false) bleed fixed) + identifier allowlist + mTLS/region fail-fast",
            &[
                "test",
                "-p",
                "myelin-storage",
                "--features",
                "integration",
                "--test",
                "integration_mr013_rls_hardening",
                "--",
                "--nocapture",
                "--test-threads=1",
            ],
        ),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowVerdict {
    Pass {
        proof: String,
    },
    ClaimedNotProven {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowResult {
    pub id: String,
    pub verdict: RowVerdict,
    pub date: String,
    pub attestation: Option<crate::make_it_real::RowAttestation>,
}

impl RowResult {
    pub fn pass(id: impl Into<String>, proof: impl Into<String>, date: impl Into<String>) -> Self {
        let proof = proof.into();
        assert!(
            !proof.trim().is_empty(),
            "a PASS row must carry its dated green-artifact proof line - no green without proof \
             (EI-01 §3); recording a Pass with an empty proof is the gamed-green the ratchet forbids"
        );
        RowResult {
            id: id.into(),
            verdict: RowVerdict::Pass { proof },
            date: date.into(),
            attestation: None,
        }
    }

    pub fn pass_attested(
        id: impl Into<String>,
        proof: impl Into<String>,
        date: impl Into<String>,
        attestation: crate::make_it_real::RowAttestation,
    ) -> Self {
        let mut row = RowResult::pass(id, proof, date);
        row.attestation = Some(attestation);
        row
    }

    pub fn claimed_not_proven(
        id: impl Into<String>,
        reason: impl Into<String>,
        date: impl Into<String>,
    ) -> Self {
        RowResult {
            id: id.into(),
            verdict: RowVerdict::ClaimedNotProven {
                reason: reason.into(),
            },
            date: date.into(),
            attestation: None,
        }
    }

    pub fn is_pass(&self) -> bool {
        matches!(self.verdict, RowVerdict::Pass { .. })
    }
}

#[derive(Clone, Debug)]
pub struct Scorecard {
    pub band: Band,
    pub rows: Vec<RowResult>,
}

impl Scorecard {
    pub fn new(band: Band) -> Self {
        Scorecard {
            band,
            rows: Vec::new(),
        }
    }

    pub fn record(&mut self, row: RowResult) {
        self.rows.push(row);
    }

    pub fn missing_required(&self) -> Vec<&'static str> {
        self.band
            .required_rows()
            .into_iter()
            .map(|r| r.id)
            .filter(|id| !self.rows.iter().any(|row| row.id == *id))
            .collect()
    }

    pub fn not_proven(&self) -> Vec<&RowResult> {
        self.rows.iter().filter(|r| !r.is_pass()).collect()
    }

    pub fn is_green(&self) -> bool {
        self.missing_required().is_empty() && self.not_proven().is_empty()
    }

    pub fn render_markdown(&self, generated_on: &str) -> String {
        let rows = self.band.required_rows();
        let (subtitle, next_band) = match self.band {
            Band::M0 => (
                "SUB-D1/D2/BUS-D4/D5/D7/D8/D9 + 12 lints + harness self-test",
                "M1",
            ),
            Band::M1Identity => (
                "ID-D1/D2/D3/D4/D5/D6/D7/D8 + the 4.1–4.11 contract-coverage re-affirm",
                "M2",
            ),
            Band::Infra => (
                "STOR-D-OUTBOX/RESTORE/RLS + ID-D-REBAC (--features integration) + 2 floor smokes",
                "the next band",
            ),
            Band::M2Reactive => (
                "BUS-D1/D3/D6/D5/D8 + REF-CDC + SRCH-D1/D2/D3/D4/D7 + NOTIF-D1..D11+snooze + \
                 AG-D1/2/3/5/7/8/11 + AG-D4 (real-kernel escape, proven-on-real-hardware) + \
                 FLOW-D1/D3/D4/D5/D6/D7+mergeq + contract-coverage",
                "M3",
            ),
            Band::M3Producers => (
                "GIT-D1/D2/D3/D7/D8/D9(+seam)/D10/D11(+int) + KN-D1/D3(floor)/D4/D5/D6/D7/D9/D10/D11/D12/D13 \
                 + contract-coverage",
                "M4",
            ),
            Band::M4Consumers => (
                "CI-D9/D1/D5/D8/D11/D6/D4/D7 + AG-D4/CI-T1 (prod-image re-confirm, real microVM) + \
                 STOR-D1/D2 (restore-verify on CI stores) + ISS-P06/D2/D3/D4/D5/D6/D7/D8/D9/D11/D13 + \
                 CHAT-D5/D6/D7/D18/D8/D9/D10/D11/D12/D15/D16/D17 + contract-coverage",
                "M5",
            ),
            Band::M5World => (
                "F6 surge family (SUB-D3/ID-D9/BUS-D7/REF-D10/SRCH-D6/NOTIF-D5/AG-D6/FLOW-D8/GIT-D6/CI-D2/CHAT-D3/D4) + \
                 GIT-D4/D5 + KN-D1-re-green/KN-D8 + GA-D1/GA-D8/CP-D7/CP-D8 + E2E-1/E2E-2/E2E-3/E2E-4 + \
                 STOR-D2 (cell scale, permanent restore gate) + contract-coverage",
                "M6",
            ),
            Band::M6Dogfood => (
                "switch tests (ISS-D14/CHAT-D19/GIT-OQ-12/KN-switch/REF-switch/SRCH-switch/CI-P35-switch) + \
                 self-hosting-CI + dogfood drills (FLOW-P29/AG-P26/CP-D23-selfhost/STOR-D37/GA-P511/REF-P28/SRCH-P33/KN-P34/GIT-P35) + \
                 truth-up pass (GA-truth-up + contract-coverage)",
                "M7",
            ),
            Band::MakeItReal => (
                "MR-004 absence ratchet + MR-009 durable-persistence verify + MR-010/011 \
                 auth-crypto negative corpus + MR-012 Structural*-removed + MR-013 tenant isolation \
                 - each row ATTESTED (blake3-bound to its proof output)",
                "the spine to claim production-real",
            ),
        };
        let mut out = String::new();
        out.push_str(&format!(
            "# {} exit-gate scorecard ({subtitle})\n\n",
            self.band
        ));
        out.push_str(&format!("> Generated: {generated_on}. "));
        out.push_str(&format!(
            "The build-layer realisation of the master band gate invariant (master-sequencing \
             §2/§4, EI-01 §2): no later-band prompt runs over a red earlier gate. Each row is a \
             dated green artifact read off the per-feature drill (this scorecard WIRES the drills, \
             it does not re-implement them). A single RED row blocks {next_band} and is recorded \
             honestly as claimed-not-proven, never edited green (EI-01 §3 / roadmap §5).\n\n",
        ));

        let verdict = if self.is_green() {
            format!("GREEN - {next_band} may start")
        } else {
            format!("RED - {next_band} is BLOCKED (a row is missing or claimed-not-proven)")
        };
        out.push_str(&format!("**Gate verdict: {verdict}**\n\n"));

        out.push_str("| Gate | Title | Verdict | Date | Permanent | Proof / reason |\n");
        out.push_str("|---|---|---|---|---|---|\n");
        for gr in &rows {
            let recorded = self.rows.iter().find(|r| r.id == gr.id);
            let perm = if gr.floor.is_some() {
                "smoke (floor open)"
            } else if gr.permanent {
                "re-run-forever"
            } else {
                "-"
            };
            match recorded {
                Some(r) => match &r.verdict {
                    RowVerdict::Pass { proof } => out.push_str(&format!(
                        "| {} | {} | PASS | {} | {} | {} |\n",
                        gr.id, gr.title, r.date, perm, proof
                    )),
                    RowVerdict::ClaimedNotProven { reason } => out.push_str(&format!(
                        "| {} | {} | **RED (claimed-not-proven)** | {} | {} | {} |\n",
                        gr.id, gr.title, r.date, perm, reason
                    )),
                },
                None => out.push_str(&format!(
                    "| {} | {} | **RED (MISSING - row dropped)** | - | {} | the ratchet re-reds a dropped row |\n",
                    gr.id, gr.title, perm
                )),
            }
        }
        out.push('\n');
        match self.band {
            Band::M0 => out.push_str(
                "**Permanent gates (re-run forever).** SUB-D1 / SUB-D2 / BUS-D4 re-run on every \
                 emit-path-touching change from M0 on (master-sequencing §1 item 6); a regression \
                 on any of them halts the run.\n",
            ),
            Band::M1Identity => out.push_str(
                "**Floor named (M5 hardening).** Identity is *correct* at M1 and *hardened* at M5: \
                 ID-D9 (the 30× surge) + the multi-cell floor drills are M5 (P-ID-31 / P-ID-35) - \
                 not part of this M1→M2 go/no-go, recorded here so the deferral is visible, never \
                 invisible (EI-01 §1). ID-D8 rides the permanent restore-verify gate (STOR-D1/D2, \
                 Storage-owned P-061/P-100), which re-runs on every store-touching change.\n",
            ),
            Band::Infra => {
                out.push_str(
                    "**Red-until-proven (the testing-policy ratchet).** Every integration row above is \
                     proven ONLY by its `cargo test --features integration` against the LIVE \
                     docker-compose stack (Postgres / RustFS / Valkey / NATS JetStream). A DB-free run \
                     cannot flip any row green - the proof command FAILS without the stack. Run via \
                     `scripts/integration-test.sh` (brings the stack up `--wait`, runs the suite). The \
                     four full drills (STOR-D-OUTBOX/RESTORE/RLS, ID-D-REBAC) ARE the whole gate.\n\n",
                );
                out.push_str("**The two genuine floors (NAMED, still open - their smokes are not the full gate):**\n");
                for gr in self.band.required_rows().iter().filter(|r| r.floor.is_some()) {
                    if let Some(floor) = gr.floor {
                        out.push_str(&format!("- **{}** - {}\n", gr.id, floor));
                    }
                }
                out.push_str(
                    "\nThe smokes give each floor not-zero-coverage NOW (a hardened-container smoke: \
                     egress-deny + read-only-root + dropped caps; a 10× containerized load smoke via \
                     myelin-harness against the live stack). The full real-kernel SANDBOX-ESCAPE gate \
                     (gVisor / microVM) and the WORLD-SCALE 30× LOAD drill (real hardware) stay RED \
                     until run on the real substrate - the deferral is visible, never invisible \
                     (EI-01 §1).\n",
                );
            }
            Band::M2Reactive => {
                out.push_str(
                    "**AG-D4 is PROVEN-ON-REAL-HARDWARE, NOT vacuous.** The escape gate runs \
                     `--features integration` with `MYELIN_REQUIRE_KVM=1` set by the scorecard \
                     runner: on a host without /dev/kvm or firecracker the drill HARD-FAILS (it does \
                     not skip), so this row only goes green when a real Firecracker microVM actually \
                     boots, runs the 11-attack adversarial corpus, and attests 0 escapes (a dated \
                     `target/ag-d4-attestation/<date>.json`). It is marked re-run-forever (EI-01 §2: \
                     RCE/sandbox-escape outranks every feature).\n\n",
                );
                out.push_str("**AG-D4 - three NAMED residuals (proven-on-real-hardware is not absence-of-all-escapes):**\n");
                out.push_str(
                    "- (a) **one green run proves THIS config against THIS battery on THIS kernel** - \
                     continuous fuzzing + full CVE-corpus tracking + a pre-GA third-party pentest \
                     remain ongoing; a single green is necessary, not sufficient-forever.\n",
                );
                out.push_str(
                    "- (b) **production must run on KVM-capable Scaleway hardware** (Elastic Metal / \
                     nested-virt) - an explicit infra requirement; on a non-KVM box this gate cannot \
                     be greened (the row hard-fails, never fakes green).\n",
                );
                out.push_str(
                    "- (c) **single-box ≠ fleet** - multi-tenant density / blast-radius at scale \
                     still overlaps the unproven 30× world-scale LOAD floor below.\n\n",
                );
                out.push_str(
                    "**The ONE true remaining floor (named, dated deferral - NOT a row that reds this gate):** \
                     the **world-scale 30× LOAD drill** needs real fleet hardware (a multi-node \
                     cluster), so it is deferred to **M5** (the FLOW-D8 / AG-D6 / NOTIF surge \
                     prompts). It is the only genuine remaining floor - everything else in M2 is \
                     proven with a dated artifact above. The deferral is visible, never invisible \
                     (EI-01 §1).\n\n",
                );
                out.push_str(
                    "**Named residual (not a floor, run-when-available):** gVisor (`runsc`) as a \
                     SECOND escape-drill backend (CI-P28) - runsc is on PATH but running the corpus \
                     under it needs an OCI bundle + root/userns privileges this host lacks; the \
                     AG-D4 attestation records it as a NAMED parametrized residual, never faked \
                     green. Firecracker (the production default) IS the exercised gate backend.\n",
                );
            }
            Band::M3Producers => {
                out.push_str(
                    "**Integration rows are RED-until-proven against the LIVE stack.** GIT-D10, \
                     GIT-D11-int, KN-D5, KN-D7, KN-D9, KN-D10 are proven ONLY by their \
                     `cargo test --features integration` against the live docker-compose stack \
                     (Postgres / RustFS / Valkey / NATS JetStream). A DB-free run cannot flip them \
                     green - the proof command FAILS without the stack. The remaining rows are the \
                     deterministic per-feature drills / CDC pairs that run with no external backend. \
                     KN-D7 rides the permanent outbox emit-iff-committed gate and GIT-D9 the co-commit \
                     gate (Storage/EB-owned, re-run-forever); the producer drills here re-affirm them \
                     but the Storage/EB gates are the permanent markers.\n\n",
                );
                out.push_str("**KN-D3 - the per-block CAS-merge NAMED FLOOR (proven floor, dated follow-on, NOT a red row):**\n");
                for gr in self.band.required_rows().iter().filter(|r| r.floor.is_some()) {
                    if let Some(floor) = gr.floor {
                        out.push_str(&format!("- **{}** - {}\n", gr.id, floor));
                    }
                }
                out.push_str(
                    "\nThe M3 deliverable PROVED the per-block CAS-merge floor (soft-locks + offline \
                     reconcile + per-block CAS rejecting a stale write); the full real-time CRDT/OT \
                     convergence is the named later collaboration follow-on (post-M3), not part of this \
                     M3→M4 go/no-go. The floor row reads green; the rendered artifact still prints the \
                     full-convergence follow-on as open, so it is never silently claimed closed \
                     (EI-01 §1).\n\n",
                );
                out.push_str(
                    "**The ONE true remaining floor (named, dated deferral - NOT a row that reds this gate):** \
                     the **world-scale 30× LOAD surge** (the GIT / KN producer surge under fleet-scale \
                     fan-out) needs real fleet hardware (a multi-node cluster), so it is deferred to \
                     **M5** - not run on a single dev box. Everything else in M3 is proven with a dated \
                     artifact above. The deferral is visible, never invisible (EI-01 §1).\n",
                );
            }
            Band::M4Consumers => {
                out.push_str(
                    "**Two PERMANENT integration rows are RED-until-proven against the LIVE substrate.** \
                     The AG-D4/CI-T1 prod-image re-confirm and the STOR-D1/D2 restore-verify on the CI \
                     stores run `--features integration` against the live docker-compose stack (Postgres \
                     / RustFS / Valkey / NATS JetStream); the AG-D4 row additionally needs a real \
                     KVM-capable host. A DB-free (or non-KVM) run cannot flip them green - the proof \
                     command FAILS. Every other CI / Issues / Chat row is a deterministic per-feature \
                     drill / CDC pair that runs with no external backend. The two permanent rows are the \
                     re-run-forever markers: AG-D4 (the real-kernel escape gate, EI-01 §2 - RCE/sandbox \
                     escape outranks every feature) and STOR-D1/D2 (the shared Storage-owned restore \
                     gate - a backup never restored is not a backup, EI-01 §3).\n\n",
                );
                out.push_str(
                    "**AG-D4/CI-T1 is PROVEN-ON-REAL-HARDWARE, NOT vacuous.** The prod-image re-confirm \
                     runs `--features integration` with `MYELIN_REQUIRE_KVM=1` set by the scorecard \
                     runner: on a host without /dev/kvm or firecracker the drill HARD-FAILS (it does not \
                     skip), so this row only goes green when the COMMITTED prod CI runner image actually \
                     boots a real Firecracker microVM, runs the adversarial corpus, and attests 0 \
                     escapes.\n\n",
                );
                out.push_str("**AG-D4/CI-T1 - three NAMED residuals (proven-on-real-hardware is not absence-of-all-escapes):**\n");
                out.push_str(
                    "- (a) **one green run proves THIS config against THIS battery on THIS kernel** - \
                     continuous fuzzing + full CVE-corpus tracking + a pre-GA third-party pentest \
                     remain ongoing; a single green is necessary, not sufficient-forever.\n",
                );
                out.push_str(
                    "- (b) **production must run on KVM-capable Scaleway hardware** (Elastic Metal / \
                     nested-virt) - an explicit infra requirement; on a non-KVM box this gate cannot \
                     be greened (the row hard-fails, never fakes green).\n",
                );
                out.push_str(
                    "- (c) **single-box ≠ fleet** - multi-tenant density / blast-radius at 30× load \
                     still overlaps the unproven world-scale LOAD floor below.\n\n",
                );
                out.push_str(
                    "**The ONE true remaining floor (named, dated deferral - NOT a row that reds this gate):** \
                     the **world-scale 30× LOAD / surge drills** (FLOW-D8 / AG-D6 / the CHAT + Issues \
                     surge under fleet-scale fan-out) need real fleet hardware (a multi-node cluster), so \
                     they are deferred to **M5** - not run on a single dev box. Everything else in M4 is \
                     proven with a dated artifact above. The deferral is visible, never invisible \
                     (EI-01 §1).\n\n",
                );
                out.push_str(
                    "**Named residual (not a floor, run-when-available):** gVisor (`runsc`) as a \
                     SECOND escape-drill backend (CI-P28) - runsc is on PATH but running the corpus \
                     under it needs an OCI bundle + root/userns privileges this host lacks; the \
                     AG-D4 attestation records it as a NAMED parametrized residual, never faked \
                     green. Firecracker (the production default) IS the exercised gate backend.\n",
                );
            }
            Band::M5World => {
                out.push_str(
                    "**The world-scale 30× surge family is proven here as a SINGLE-BOX SCALED drill** \
                     (the shed-order / lane-priority / cross-tenant-isolation LOGIC is exercised and \
                     green). The **true multi-node FLEET proof** (30× fan-out across a real multi-box \
                     cluster, measured blast-radius/density at fleet scale) remains the ONE genuine \
                     named floor - it needs real fleet hardware this dev host does not have. The drill \
                     proves the mechanism; the fleet-scale residual is NAMED, never faked green \
                     (EI-01 §1).\n\n",
                );
                out.push_str(
                    "**STOR-D2 at cell scale** is the permanent restore gate (a backup never restored \
                     is not a backup, EI-01 §3) - re-run-forever.\n\n",
                );
                out.push_str(
                    "**Carried-forward floor (M7):** the AG-D4 sandbox isolation boundary is \
                     proven-on-real-hardware, but a real `JobSpec.command` does not yet flow through \
                     the PRODUCTION `launch()` on either backend (Firecracker prod boots \
                     `init=/bin/true`; gVisor prod runs only `runsc --version`) - production exec is \
                     filled by M7 P-544/P-545, named here, not a row that reds this M5 gate.\n\n",
                );
                out.push_str(
                    "**Measured-trigger-gated floors named in M5 (trigger not fired):** Chat ScyllaDB \
                     hot-tier promotion (M4-C1), mega-channel channel-sharded home-node (M4-C2), \
                     comment-threading consolidation (OQ-L) - each ships its seam + named follow-on, \
                     promoted only on its measured trigger; not a row that reds this gate.\n",
                );
            }
            Band::M6Dogfood => {
                out.push_str(
                    "**M6 is the platform done-bar reached by DOGFOODING** - Myelin hosts its own \
                     repos/CI/issues/docs/chat, and the switch tests are driven over the real surface \
                     (measured contrast + latency), not read off a feature list (EI-01 §4).\n\n",
                );
                out.push_str(
                    "**The self-hosting CI graph is green on the platform's own commits** - the \
                     dogfood loop is live; every-incident-adds-a-drill.\n\n",
                );
                out.push_str(
                    "**STOR-D37 dogfood restore-verify on Myelin's own commits** is permanent (a \
                     backup never restored is not a backup, EI-01 §3).\n\n",
                );
                out.push_str(
                    "**The truth-up pass holds:** every PROVEN row here rests on a dated green \
                     artifact, never a doc claim (code-wins-over-docs, EI-01 §1).\n\n",
                );
                out.push_str(
                    "**M7 (P-522..P-546) is the next band - production readiness & security hardening \
                     - and is NOT yet implemented.** M0..M6 deliberately shipped several production \
                     mechanisms as documented EI-01 §1 structural FLOORS (correct in shape, honestly \
                     named, not production-real): auth-token crypto (StructuralTokenSigner/Verifier \
                     still in prod constructors), HSM-class KMS, durable Identity stores (in-memory \
                     maps), real backup/restore (modeled offsets), and **sandbox PRODUCTION exec on \
                     both backends** (Firecracker prod boots `init=/bin/true`; gVisor prod runs only \
                     `runsc --version` - the AG-D4 isolation boundary is proven on real hardware, but \
                     a real `JobSpec.command` does not yet flow through prod `launch()`). M7 fills each \
                     floor with a real implementation + a SEPARATE verification prompt, and gates the \
                     first production release fail-closed (P-546). **This M6 green is dogfood-complete, \
                     NOT production-ready** - do not read it as the latter.\n",
                );
            }
            Band::MakeItReal => {
                out.push_str(
                    "**RED BY DEFAULT - the evidence-integrity skeleton (MR-005).** This gate is \
                     NOT a feature go/no-go; it is the spine's un-fakeable evidence floor. It reads \
                     GREEN only when EVERY required row carries a FRESH, hash-VALID, attested PASS. \
                     Missing row → RED. Stale row (older than the freshness window) → RED. \
                     Tamper / hash-mismatch → RED. A green that cannot prove it bites is not \
                     evidence (master-plan: \"attested, not hand-editable scorecards\"). Every row \
                     has landed, but any unavailable backend, skipped proof, stale artifact, or \
                     regression re-arms RED instead of trusting the last green (L1 / EI-01 §1).\n\n",
                );
                out.push_str(
                    "**Attestation (the un-fakeable layer).** Each PASS is bound by a blake3 hash \
                     over {row id, proof-command argv, ISO date, PASS flag, digest of the captured \
                     proof-command output}. The make-it-real gate re-runs each proof command, \
                     re-derives the output digest, and recomputes the hash; a hand-edited verdict \
                     (RED flipped to PASS) or changed output bytes no longer matches the stored \
                     hash, so the row reds the gate instead of passing silently. The machine-\
                     readable attested manifest lives next to this file as `make-it-real.json` - \
                     it is the artifact the gate re-validates (this `.md` is the human mirror).\n\n",
                );
                out.push_str(
                    "**Required rows → owning MR (each re-run forever):** MR-004 (absence \
                     ratchet) · MR-009 (durable-persistence verify) · \
                     MR-010 (human/SSO auth crypto + negative corpus) · MR-011 (machine/DPoP tokens \
                     + negative corpus) · MR-012 (Structural* removed, scanner green-on-prod) · \
                     MR-013 (tenant isolation, SET LOCAL RLS + reset-on-release).\n",
                );
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_rows_cover_the_full_sub_m0_gate() {
        let ids: Vec<&str> = required_rows().iter().map(|r| r.id).collect();
        for must in [
            "SUB-D1",
            "SUB-D2",
            "BUS-D4",
            "SUB-D5",
            "SUB-D7",
            "SUB-D8",
            "SUB-D9",
            "lints",
            "lint-fixtures",
            "contract-coverage",
            "harness-self-test",
        ] {
            assert!(
                ids.contains(&must),
                "SUB-M0 gate is missing required row {must}"
            );
        }
        assert_eq!(ids.len(), 11, "the SUB-M0 row set is frozen at 11 rows");
    }

    #[test]
    fn permanent_gates_are_the_three_emit_path_drills() {
        let perm: Vec<&str> = required_rows()
            .into_iter()
            .filter(|r| r.permanent)
            .map(|r| r.id)
            .collect();
        assert_eq!(perm, vec!["SUB-D1", "SUB-D2", "BUS-D4"]);
    }

    #[test]
    fn all_rows_proven_is_green() {
        let mut card = Scorecard::new(Band::M0);
        for r in required_rows() {
            card.record(RowResult::pass(
                r.id,
                format!("[2026-06-19] PASS {}", r.id),
                "2026-06-19",
            ));
        }
        assert!(card.is_green(), "every required row proven ⇒ green");
        assert!(card.missing_required().is_empty());
        assert!(card
            .render_markdown("2026-06-19")
            .contains("GREEN - M1 may start"));
    }

    #[test]
    fn dropping_a_row_reds_the_gate() {
        let mut card = Scorecard::new(Band::M0);
        for r in required_rows().into_iter().filter(|r| r.id != "SUB-D1") {
            card.record(RowResult::pass(r.id, "[2026-06-19] PASS", "2026-06-19"));
        }
        assert_eq!(card.missing_required(), vec!["SUB-D1"]);
        assert!(!card.is_green(), "a missing required row must RED the gate");
        assert!(card.render_markdown("2026-06-19").contains("RED (MISSING"));
    }

    #[test]
    fn claimed_not_proven_row_reds_the_gate() {
        let mut card = Scorecard::new(Band::M0);
        for r in required_rows() {
            if r.id == "SUB-D8" {
                card.record(RowResult::claimed_not_proven(
                    r.id,
                    "causal-depth ceiling not yet enforced past 12",
                    "2026-06-19",
                ));
            } else {
                card.record(RowResult::pass(r.id, "[2026-06-19] PASS", "2026-06-19"));
            }
        }
        assert!(!card.is_green(), "a claimed-not-proven row blocks M1");
        assert_eq!(card.not_proven().len(), 1);
        assert!(card
            .render_markdown("2026-06-19")
            .contains("RED (claimed-not-proven)"));
    }

    #[test]
    #[should_panic(expected = "no green without proof")]
    fn a_pass_without_proof_panics() {
        let _ = RowResult::pass("SUB-D1", "   ", "2026-06-19");
    }

    #[test]
    fn id_m1_required_rows_cover_the_eight_drills_plus_coverage() {
        let ids: Vec<&str> = id_m1_required_rows().iter().map(|r| r.id).collect();
        for must in [
            "ID-D1",
            "ID-D2",
            "ID-D3",
            "ID-D4",
            "ID-D5",
            "ID-D6",
            "ID-D7",
            "ID-D8",
            "contract-coverage",
        ] {
            assert!(
                ids.contains(&must),
                "Id-M1 gate is missing required row {must}"
            );
        }
        assert_eq!(
            ids.len(),
            9,
            "the Id-M1 row set is frozen at 8 drills + coverage = 9 rows"
        );
        assert_eq!(
            Band::M1Identity
                .required_rows()
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            ids
        );
    }

    #[test]
    fn id_m1_drill_proof_commands_target_the_service_drills() {
        for row in id_m1_required_rows()
            .into_iter()
            .filter(|r| r.id != "contract-coverage")
        {
            assert_eq!(row.proof_command[0], "test");
            assert_eq!(row.proof_command[1], "-p");
            assert_eq!(row.proof_command[2], "myelin-identity-service");
            assert_eq!(row.proof_command[3], "--test");
            assert!(
                row.proof_command[4].starts_with("drill_id_d"),
                "{} must point at a drill_id_d* target, got {}",
                row.id,
                row.proof_command[4]
            );
        }
    }

    #[test]
    fn id_m1_has_no_permanent_rows() {
        assert!(id_m1_required_rows().iter().all(|r| !r.permanent));
    }

    #[test]
    fn id_m1_all_rows_proven_is_green() {
        let mut card = Scorecard::new(Band::M1Identity);
        for r in id_m1_required_rows() {
            card.record(RowResult::pass(
                r.id,
                format!("[2026-06-19] PASS {}", r.id),
                "2026-06-19",
            ));
        }
        assert!(card.is_green(), "every Id-M1 row proven ⇒ green");
        assert!(card.missing_required().is_empty());
        let md = card.render_markdown("2026-06-19");
        assert!(md.contains("GREEN - M2 may start"));
        assert!(
            md.contains("P-ID-31"),
            "the M5-hardening floor (P-ID-31/P-ID-35) must be named"
        );
        assert!(md.contains("ID-D9"), "the 30× surge floor must be named");
    }

    #[test]
    fn id_m1_dropping_any_row_reds_the_gate() {
        for dropped in id_m1_required_rows() {
            let mut card = Scorecard::new(Band::M1Identity);
            for r in id_m1_required_rows()
                .into_iter()
                .filter(|r| r.id != dropped.id)
            {
                card.record(RowResult::pass(r.id, "[2026-06-19] PASS", "2026-06-19"));
            }
            assert_eq!(card.missing_required(), vec![dropped.id]);
            assert!(
                !card.is_green(),
                "dropping {} must RED the M1→M2 gate",
                dropped.id
            );
        }
    }

    #[test]
    fn id_m1_claimed_not_proven_row_reds_the_gate() {
        let mut card = Scorecard::new(Band::M1Identity);
        for r in id_m1_required_rows() {
            if r.id == "ID-D3" {
                card.record(RowResult::claimed_not_proven(
                    r.id,
                    "cross-tenant count != 0 - recorded honestly, never edited green",
                    "2026-06-19",
                ));
            } else {
                card.record(RowResult::pass(r.id, "[2026-06-19] PASS", "2026-06-19"));
            }
        }
        assert!(!card.is_green(), "a claimed-not-proven Id drill blocks M2");
        assert_eq!(card.not_proven().len(), 1);
        assert!(card
            .render_markdown("2026-06-19")
            .contains("RED - M2 is BLOCKED"));
    }

    #[test]
    fn bands_select_distinct_row_sets() {
        let m0: Vec<&str> = Band::M0.required_rows().iter().map(|r| r.id).collect();
        let id: Vec<&str> = Band::M1Identity
            .required_rows()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_ne!(m0, id);
        assert!(m0.contains(&"SUB-D1"));
        assert!(id.contains(&"ID-D3"));
    }

    #[test]
    fn infra_required_rows_cover_the_eight_drills_plus_two_floor_smokes() {
        let ids: Vec<&str> = infra_required_rows().iter().map(|r| r.id).collect();
        for must in [
            "STOR-D-OUTBOX",
            "STOR-D-RESTORE",
            "STOR-D-RLS",
            "ID-D-REBAC",
            "EB-D-PARTITION",
            "EB-D-RESIDENCY",
            "CP-D3/STOR-D5",
            "SRCH-D-LAYOUT",
            "SANDBOX-SMOKE",
            "LOAD-10X-SMOKE",
        ] {
            assert!(
                ids.contains(&must),
                "Infra gate is missing required row {must}"
            );
        }
        assert_eq!(
            ids.len(),
            10,
            "the Infra row set is frozen at 8 FULL real-backend drills + 2 floor smokes = 10 rows"
        );
        assert_eq!(
            Band::Infra
                .required_rows()
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            ids
        );
    }

    #[test]
    fn infra_proof_commands_are_features_integration() {
        for row in infra_required_rows() {
            assert!(
                row.proof_command.contains(&"--features")
                    && row.proof_command.contains(&"integration"),
                "{} must run --features integration (red-until-proven), got {:?}",
                row.id,
                row.proof_command
            );
        }
    }

    #[test]
    fn infra_only_the_two_floor_smokes_name_a_floor() {
        let floored: Vec<&str> = infra_required_rows()
            .into_iter()
            .filter(|r| r.floor.is_some())
            .map(|r| r.id)
            .collect();
        assert_eq!(floored, vec!["SANDBOX-SMOKE", "LOAD-10X-SMOKE"]);
    }

    #[test]
    fn infra_all_rows_proven_is_green_but_floors_stay_named() {
        let mut card = Scorecard::new(Band::Infra);
        for r in infra_required_rows() {
            card.record(RowResult::pass(
                r.id,
                format!("[2026-06-19] PASS {}", r.id),
                "2026-06-19",
            ));
        }
        assert!(card.is_green(), "every Infra row proven ⇒ green");
        let md = card.render_markdown("2026-06-19");
        assert!(
            md.contains("SANDBOX-ESCAPE"),
            "the gVisor/microVM floor must stay named"
        );
        assert!(
            md.contains("WORLD-SCALE 30×"),
            "the real-hardware load floor must stay named"
        );
        assert!(md.contains("red-until-proven") || md.contains("Red-until-proven"));
    }

    #[test]
    fn infra_dropping_any_row_reds_the_gate() {
        for dropped in infra_required_rows() {
            let mut card = Scorecard::new(Band::Infra);
            for r in infra_required_rows()
                .into_iter()
                .filter(|r| r.id != dropped.id)
            {
                card.record(RowResult::pass(r.id, "[2026-06-19] PASS", "2026-06-19"));
            }
            assert_eq!(card.missing_required(), vec![dropped.id]);
            assert!(
                !card.is_green(),
                "dropping {} must RED the Infra gate",
                dropped.id
            );
        }
    }

    #[test]
    fn infra_unproven_drill_reds_the_gate() {
        let mut card = Scorecard::new(Band::Infra);
        for r in infra_required_rows() {
            if r.id == "STOR-D-OUTBOX" {
                card.record(RowResult::claimed_not_proven(
                    r.id,
                    "the live stack was not up - `cargo test --features integration` failed (red-until-proven)",
                    "2026-06-19",
                ));
            } else {
                card.record(RowResult::pass(r.id, "[2026-06-19] PASS", "2026-06-19"));
            }
        }
        assert!(
            !card.is_green(),
            "an unproven integration drill blocks the infra gate"
        );
        assert_eq!(card.not_proven().len(), 1);
    }

    #[test]
    fn m2_required_rows_cover_the_reactive_layer_families() {
        let ids: Vec<&str> = m2_required_rows().iter().map(|r| r.id).collect();
        for must in [
            "BUS-D1",
            "BUS-D3",
            "BUS-D6",
            "BUS-D5",
            "BUS-D8",
            "REF-CDC",
            "SRCH-D1",
            "SRCH-D2",
            "SRCH-D3",
            "SRCH-D4",
            "SRCH-D7",
            "NOTIF-D1",
            "NOTIF-D2",
            "NOTIF-D3",
            "NOTIF-D4",
            "NOTIF-D7",
            "NOTIF-D8",
            "NOTIF-D9",
            "NOTIF-D10",
            "NOTIF-D11",
            "NOTIF-snooze",
            "AG-D1/2/3",
            "AG-D5-batch",
            "AG-D5-loop",
            "AG-D7",
            "AG-D8",
            "AG-D11",
            "AG-D4",
            "FLOW-D1",
            "FLOW-D3",
            "FLOW-D4-hitl",
            "FLOW-D4-per-effect",
            "FLOW-D5",
            "FLOW-D6",
            "FLOW-D7",
            "FLOW-mergeq",
            "contract-coverage",
        ] {
            assert!(
                ids.contains(&must),
                "M2 gate is missing required row {must}"
            );
        }
        assert_eq!(
            Band::M2Reactive
                .required_rows()
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            ids
        );
    }

    #[test]
    fn m2_ag_d4_is_the_only_permanent_row_and_runs_integration() {
        let perm: Vec<&str> = m2_required_rows()
            .into_iter()
            .filter(|r| r.permanent)
            .map(|r| r.id)
            .collect();
        assert_eq!(
            perm,
            vec!["AG-D4"],
            "AG-D4 is the only re-run-forever M2 row"
        );
        let ag_d4 = m2_required_rows()
            .into_iter()
            .find(|r| r.id == "AG-D4")
            .unwrap();
        assert!(
            ag_d4.proof_command.contains(&"--features")
                && ag_d4.proof_command.contains(&"integration"),
            "AG-D4 must run --features integration (the real-kernel escape drill), got {:?}",
            ag_d4.proof_command
        );
        assert!(ag_d4.proof_command.contains(&"escape_drill_test"));
    }

    #[test]
    fn m2_all_rows_proven_is_green_with_residuals_and_floor_named() {
        let mut card = Scorecard::new(Band::M2Reactive);
        for r in m2_required_rows() {
            card.record(RowResult::pass(
                r.id,
                format!("[2026-06-21] PASS {}", r.id),
                "2026-06-21",
            ));
        }
        assert!(card.is_green(), "every M2 row proven ⇒ green");
        assert!(card.missing_required().is_empty());
        let md = card.render_markdown("2026-06-21");
        assert!(md.contains("GREEN - M3 may start"));
        assert!(
            md.contains("MYELIN_REQUIRE_KVM"),
            "AG-D4 non-vacuous mechanism must be named"
        );
        assert!(md.contains("PROVEN-ON-REAL-HARDWARE"));
        assert!(md.contains("THIS kernel"), "residual (a) must be named");
        assert!(md.contains("Scaleway"), "residual (b) must be named");
        assert!(
            md.contains("single-box ≠ fleet"),
            "residual (c) must be named"
        );
        assert!(
            md.contains("world-scale 30× LOAD"),
            "the one true 30× floor must be named"
        );
        assert!(md.contains("M5"), "the floor is deferred to M5");
        assert!(
            md.contains("CI-P28"),
            "the gVisor second-backend residual must be named"
        );
    }

    #[test]
    fn m2_dropping_any_row_reds_the_gate() {
        for dropped in m2_required_rows() {
            let mut card = Scorecard::new(Band::M2Reactive);
            for r in m2_required_rows()
                .into_iter()
                .filter(|r| r.id != dropped.id)
            {
                card.record(RowResult::pass(r.id, "[2026-06-21] PASS", "2026-06-21"));
            }
            assert_eq!(card.missing_required(), vec![dropped.id]);
            assert!(
                !card.is_green(),
                "dropping {} must RED the M2→M3 gate",
                dropped.id
            );
        }
    }

    #[test]
    fn m2_claimed_not_proven_row_reds_the_gate() {
        let mut card = Scorecard::new(Band::M2Reactive);
        for r in m2_required_rows() {
            if r.id == "AG-D4" {
                card.record(RowResult::claimed_not_proven(
                    r.id,
                    "MYELIN_REQUIRE_KVM=1 but no microVM booted - a vacuous green is refused, recorded RED",
                    "2026-06-21",
                ));
            } else {
                card.record(RowResult::pass(r.id, "[2026-06-21] PASS", "2026-06-21"));
            }
        }
        assert!(!card.is_green(), "a claimed-not-proven M2 row blocks M3");
        assert_eq!(card.not_proven().len(), 1);
        assert!(card
            .render_markdown("2026-06-21")
            .contains("RED - M3 is BLOCKED"));
    }

    #[test]
    fn m3_required_rows_cover_the_producer_families() {
        let ids: Vec<&str> = m3_required_rows().iter().map(|r| r.id).collect();
        for must in [
            "GIT-D1",
            "GIT-D2",
            "GIT-D3",
            "GIT-D7",
            "GIT-D8",
            "GIT-D9",
            "GIT-D9-seam",
            "GIT-D10",
            "GIT-D11",
            "GIT-D11-int",
            "KN-D1",
            "KN-D3",
            "KN-D4",
            "KN-D5",
            "KN-D6",
            "KN-D7",
            "KN-D9",
            "KN-D10",
            "KN-D11",
            "KN-D12",
            "KN-D13",
            "contract-coverage",
        ] {
            assert!(
                ids.contains(&must),
                "M3 gate is missing required row {must}"
            );
        }
        assert!(ids.iter().any(|id| id.starts_with("GIT-")));
        assert!(ids.iter().any(|id| id.starts_with("KN-")));
        assert_eq!(
            Band::M3Producers
                .required_rows()
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            ids
        );
    }

    #[test]
    fn m3_integration_rows_are_features_integration() {
        let integration: Vec<&str> = m3_required_rows()
            .into_iter()
            .filter(|r| {
                r.proof_command.contains(&"--features") && r.proof_command.contains(&"integration")
            })
            .map(|r| r.id)
            .collect();
        assert_eq!(
            integration,
            vec![
                "GIT-D10",
                "GIT-D11-int",
                "KN-D5",
                "KN-D7",
                "KN-D9",
                "KN-D10"
            ],
            "the M3 integration set is frozen at the six live-stack producer drills"
        );
    }

    #[test]
    fn m3_only_kn_d3_names_a_floor() {
        let floored: Vec<&str> = m3_required_rows()
            .into_iter()
            .filter(|r| r.floor.is_some())
            .map(|r| r.id)
            .collect();
        assert_eq!(floored, vec!["KN-D3"]);
    }

    #[test]
    fn m3_all_rows_proven_is_green_with_floor_and_surge_named() {
        let mut card = Scorecard::new(Band::M3Producers);
        for r in m3_required_rows() {
            card.record(RowResult::pass(
                r.id,
                format!("[2026-06-22] PASS {}", r.id),
                "2026-06-22",
            ));
        }
        assert!(card.is_green(), "every M3 row proven ⇒ green");
        assert!(card.missing_required().is_empty());
        let md = card.render_markdown("2026-06-22");
        assert!(md.contains("GREEN - M4 may start"));
        assert!(
            md.contains("CRDT/OT"),
            "the KN-D3 full-convergence follow-on must be named"
        );
        assert!(
            md.contains("NAMED FLOOR"),
            "the KN-D3 named floor must be printed"
        );
        assert!(
            md.contains("world-scale 30× LOAD"),
            "the 30× surge floor must be named"
        );
        assert!(md.contains("M5"), "the surge floor is deferred to M5");
        assert!(md.contains("RED-until-proven"));
    }

    #[test]
    fn m3_dropping_any_row_reds_the_gate() {
        for dropped in m3_required_rows() {
            let mut card = Scorecard::new(Band::M3Producers);
            for r in m3_required_rows()
                .into_iter()
                .filter(|r| r.id != dropped.id)
            {
                card.record(RowResult::pass(r.id, "[2026-06-22] PASS", "2026-06-22"));
            }
            assert_eq!(card.missing_required(), vec![dropped.id]);
            assert!(
                !card.is_green(),
                "dropping {} must RED the M3→M4 gate",
                dropped.id
            );
        }
    }

    #[test]
    fn m3_claimed_not_proven_row_reds_the_gate() {
        let mut card = Scorecard::new(Band::M3Producers);
        for r in m3_required_rows() {
            if r.id == "KN-D7" {
                card.record(RowResult::claimed_not_proven(
                    r.id,
                    "the live stack was not up - `cargo test --features integration` failed (red-until-proven)",
                    "2026-06-22",
                ));
            } else {
                card.record(RowResult::pass(r.id, "[2026-06-22] PASS", "2026-06-22"));
            }
        }
        assert!(!card.is_green(), "a claimed-not-proven M3 row blocks M4");
        assert_eq!(card.not_proven().len(), 1);
        assert!(card
            .render_markdown("2026-06-22")
            .contains("RED - M4 is BLOCKED"));
    }

    #[test]
    fn m4_required_rows_cover_the_consumer_families() {
        let ids: Vec<&str> = m4_required_rows().iter().map(|r| r.id).collect();
        for must in [
            "CI-D9",
            "CI-D1",
            "CI-D5",
            "CI-D8/GIT-D10",
            "CI-D11",
            "CI-D6",
            "CI-D4",
            "CI-D7",
            "AG-D4/CI-T1",
            "STOR-D1/D2",
            "ISS-P06",
            "ISS-D2",
            "ISS-D3",
            "ISS-D4",
            "ISS-D5",
            "ISS-D6-calendar",
            "ISS-D6-escalation",
            "ISS-D7",
            "ISS-D8-rollup",
            "ISS-D8-olap",
            "ISS-D9",
            "ISS-D11",
            "ISS-D13",
            "CHAT-D5-unfurl",
            "CHAT-D5-humanise",
            "CHAT-D6/D7/D18",
            "CHAT-D8",
            "CHAT-D9",
            "CHAT-D10",
            "CHAT-D11",
            "CHAT-D12",
            "CHAT-D15",
            "CHAT-D16",
            "CHAT-D17",
            "contract-coverage",
        ] {
            assert!(
                ids.contains(&must),
                "M4 gate is missing required row {must}"
            );
        }
        assert!(ids.iter().any(|id| id.starts_with("CI-")));
        assert!(ids.iter().any(|id| id.starts_with("ISS-")));
        assert!(ids.iter().any(|id| id.starts_with("CHAT-")));
        assert_eq!(
            Band::M4Consumers
                .required_rows()
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            ids
        );
    }

    #[test]
    fn m4_permanent_rows_are_the_three_integration_gates() {
        let perm: Vec<&str> = m4_required_rows()
            .into_iter()
            .filter(|r| r.permanent)
            .map(|r| r.id)
            .collect();
        assert_eq!(
            perm,
            vec!["CI-D11", "AG-D4/CI-T1", "STOR-D1/D2"],
            "the M4 permanent set is exactly durable reconnect + prod-image escape + restore-verify"
        );
        let integration: Vec<&str> = m4_required_rows()
            .into_iter()
            .filter(|r| {
                r.proof_command.contains(&"--features") && r.proof_command.contains(&"integration")
            })
            .map(|r| r.id)
            .collect();
        assert_eq!(
            integration,
            vec!["CI-D11", "AG-D4/CI-T1", "STOR-D1/D2"],
            "exactly the three permanent rows run --features integration (red-until-proven)"
        );
        let ag = m4_required_rows()
            .into_iter()
            .find(|r| r.id == "AG-D4/CI-T1")
            .unwrap();
        assert!(ag
            .proof_command
            .contains(&"escape_drill_ci_committed_gate_reconfirm_test"));
    }

    #[test]
    fn m4_has_no_floor_rows() {
        assert!(m4_required_rows().iter().all(|r| r.floor.is_none()));
    }

    #[test]
    fn m4_all_rows_proven_is_green_with_residuals_and_floor_named() {
        let mut card = Scorecard::new(Band::M4Consumers);
        for r in m4_required_rows() {
            card.record(RowResult::pass(
                r.id,
                format!("[2026-06-24] PASS {}", r.id),
                "2026-06-24",
            ));
        }
        assert!(card.is_green(), "every M4 row proven ⇒ green");
        assert!(card.missing_required().is_empty());
        let md = card.render_markdown("2026-06-24");
        assert!(md.contains("GREEN - M5 may start"));
        assert!(
            md.contains("MYELIN_REQUIRE_KVM"),
            "AG-D4/CI-T1 non-vacuous mechanism must be named"
        );
        assert!(md.contains("PROVEN-ON-REAL-HARDWARE"));
        assert!(md.contains("THIS kernel"), "residual (a) must be named");
        assert!(md.contains("Scaleway"), "residual (b) must be named");
        assert!(
            md.contains("single-box ≠ fleet"),
            "residual (c) must be named"
        );
        assert!(
            md.contains("restore"),
            "the STOR-D1/D2 permanent restore gate must be named"
        );
        assert!(
            md.contains("world-scale 30× LOAD"),
            "the one true 30× / surge floor must be named"
        );
        assert!(md.contains("M5"), "the floor is deferred to M5");
        assert!(
            md.contains("CI-P28"),
            "the gVisor second-backend residual must be named"
        );
    }

    #[test]
    fn m4_dropping_any_row_reds_the_gate() {
        for dropped in m4_required_rows() {
            let mut card = Scorecard::new(Band::M4Consumers);
            for r in m4_required_rows()
                .into_iter()
                .filter(|r| r.id != dropped.id)
            {
                card.record(RowResult::pass(r.id, "[2026-06-24] PASS", "2026-06-24"));
            }
            assert_eq!(card.missing_required(), vec![dropped.id]);
            assert!(
                !card.is_green(),
                "dropping {} must RED the M4→M5 gate",
                dropped.id
            );
        }
    }

    #[test]
    fn m4_claimed_not_proven_row_reds_the_gate() {
        let mut card = Scorecard::new(Band::M4Consumers);
        for r in m4_required_rows() {
            if r.id == "AG-D4/CI-T1" {
                card.record(RowResult::claimed_not_proven(
                    r.id,
                    "MYELIN_REQUIRE_KVM=1 but no microVM booted - a vacuous green is refused, recorded RED",
                    "2026-06-24",
                ));
            } else {
                card.record(RowResult::pass(r.id, "[2026-06-24] PASS", "2026-06-24"));
            }
        }
        assert!(!card.is_green(), "a claimed-not-proven M4 row blocks M5");
        assert_eq!(card.not_proven().len(), 1);
        assert!(card
            .render_markdown("2026-06-24")
            .contains("RED - M5 is BLOCKED"));
    }

    #[test]
    fn m5_required_rows_cover_the_world_scale_families() {
        let ids: Vec<&str> = m5_required_rows().iter().map(|r| r.id).collect();
        for must in [
            "SUB-D3",
            "ID-D9",
            "BUS-D7",
            "REF-D10",
            "SRCH-D6",
            "NOTIF-D5",
            "AG-D6",
            "FLOW-D8",
            "GIT-D6",
            "CI-D2",
            "CHAT-D3/D4",
            "GIT-D4",
            "GIT-D5",
            "KN-D1-re-green",
            "KN-D8",
            "GA-D1",
            "GA-D8",
            "CP-D7",
            "CP-D8",
            "E2E-1",
            "E2E-2",
            "E2E-3",
            "E2E-4",
            "STOR-D2-cell",
            "contract-coverage",
        ] {
            assert!(
                ids.contains(&must),
                "M5 gate is missing required row {must}"
            );
        }
        assert!(ids.contains(&"AG-D6"));
        assert!(ids.iter().any(|id| id.starts_with("GIT-")));
        assert!(ids.iter().any(|id| id.starts_with("KN-")));
        assert!(ids
            .iter()
            .any(|id| id.starts_with("GA-") || id.starts_with("CP-")));
        assert!(ids.iter().any(|id| id.starts_with("E2E-")));
        assert_eq!(
            Band::M5World
                .required_rows()
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            ids
        );
    }

    #[test]
    fn m5_permanent_row_is_the_cell_scale_restore_gate() {
        let perm: Vec<&str> = m5_required_rows()
            .into_iter()
            .filter(|r| r.permanent)
            .map(|r| r.id)
            .collect();
        assert_eq!(
            perm,
            vec!["STOR-D2-cell"],
            "the M5 permanent set is exactly the cell-scale restore gate"
        );
        assert!(
            m5_required_rows()
                .iter()
                .all(|r| !r.proof_command.contains(&"--features")),
            "no M5 row needs --features integration"
        );
    }

    #[test]
    fn m5_has_no_floor_rows() {
        assert!(m5_required_rows().iter().all(|r| r.floor.is_none()));
    }

    #[test]
    fn m5_all_rows_proven_is_green_with_floor_named() {
        let mut card = Scorecard::new(Band::M5World);
        for r in m5_required_rows() {
            card.record(RowResult::pass(
                r.id,
                format!("[2026-06-25] PASS {}", r.id),
                "2026-06-25",
            ));
        }
        assert!(card.is_green(), "every M5 row proven ⇒ green");
        assert!(card.missing_required().is_empty());
        let md = card.render_markdown("2026-06-25");
        assert!(md.contains("GREEN - M6 may start"));
        assert!(md.contains("SINGLE-BOX SCALED"));
        assert!(md.contains("FLEET"));
        assert!(
            md.contains("permanent restore gate"),
            "the STOR-D2 permanent restore gate must be named"
        );
        assert!(
            md.contains("P-544"),
            "the M7 production-exec floor must be named"
        );
        assert!(
            md.contains("M4-C1"),
            "the ScyllaDB hot-tier trigger floor must be named"
        );
        assert!(
            md.contains("OQ-L"),
            "the comment-threading trigger floor must be named"
        );
    }

    #[test]
    fn m5_dropping_any_row_reds_the_gate() {
        for dropped in m5_required_rows() {
            let mut card = Scorecard::new(Band::M5World);
            for r in m5_required_rows()
                .into_iter()
                .filter(|r| r.id != dropped.id)
            {
                card.record(RowResult::pass(r.id, "[2026-06-25] PASS", "2026-06-25"));
            }
            assert_eq!(card.missing_required(), vec![dropped.id]);
            assert!(
                !card.is_green(),
                "dropping {} must RED the M5→M6 gate",
                dropped.id
            );
        }
    }

    #[test]
    fn m5_claimed_not_proven_row_reds_the_gate() {
        let mut card = Scorecard::new(Band::M5World);
        for r in m5_required_rows() {
            if r.id == "STOR-D2-cell" {
                card.record(RowResult::claimed_not_proven(
                    r.id,
                    "RPO/RTO exceeded the budget at cell scale under world-scale load - recorded RED",
                    "2026-06-25",
                ));
            } else {
                card.record(RowResult::pass(r.id, "[2026-06-25] PASS", "2026-06-25"));
            }
        }
        assert!(!card.is_green(), "a claimed-not-proven M5 row blocks M6");
        assert_eq!(card.not_proven().len(), 1);
        assert!(card
            .render_markdown("2026-06-25")
            .contains("RED - M6 is BLOCKED"));
    }

    #[test]
    fn m6_required_rows_cover_the_dogfood_families() {
        let ids: Vec<&str> = m6_required_rows().iter().map(|r| r.id).collect();
        for must in [
            "ISS-D14",
            "CHAT-D19",
            "GIT-OQ-12",
            "KN-switch",
            "REF-switch",
            "SRCH-switch",
            "CI-P35-switch",
            "self-hosting-CI",
            "FLOW-P29",
            "AG-P26",
            "CP-D23-selfhost",
            "STOR-D37",
            "GA-P511",
            "REF-P28",
            "SRCH-P33",
            "KN-P34",
            "GIT-P35",
            "GA-truth-up",
            "contract-coverage",
        ] {
            assert!(
                ids.contains(&must),
                "M6 gate is missing required row {must}"
            );
        }
        assert!(ids
            .iter()
            .any(|id| id.ends_with("-switch") || *id == "ISS-D14"));
        assert!(ids.contains(&"self-hosting-CI"));
        assert!(ids
            .iter()
            .any(|id| id.ends_with("-P29") || id.ends_with("-P26")));
        assert!(ids.contains(&"GA-truth-up"));
        assert_eq!(
            Band::M6Dogfood
                .required_rows()
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            ids
        );
    }

    #[test]
    fn m6_permanent_row_is_the_dogfood_restore_gate() {
        let perm: Vec<&str> = m6_required_rows()
            .into_iter()
            .filter(|r| r.permanent)
            .map(|r| r.id)
            .collect();
        assert_eq!(
            perm,
            vec!["STOR-D37"],
            "the M6 permanent set is exactly the dogfood restore gate"
        );
        assert!(
            m6_required_rows()
                .iter()
                .all(|r| !r.proof_command.contains(&"--features")),
            "no M6 row needs --features integration"
        );
    }

    #[test]
    fn m6_has_no_floor_rows() {
        assert!(m6_required_rows().iter().all(|r| r.floor.is_none()));
    }

    #[test]
    fn m6_all_rows_proven_is_green() {
        let mut card = Scorecard::new(Band::M6Dogfood);
        for r in m6_required_rows() {
            card.record(RowResult::pass(
                r.id,
                format!("[2026-06-26] PASS {}", r.id),
                "2026-06-26",
            ));
        }
        assert!(card.is_green(), "every M6 row proven ⇒ green");
        assert!(card.missing_required().is_empty());
        let md = card.render_markdown("2026-06-26");
        assert!(md.contains("GREEN - M7 may start"));
        assert!(md.contains("DOGFOODING"));
        assert!(
            md.contains("self-hosting CI graph is green"),
            "the live self-hosting CI graph must be named"
        );
        assert!(
            md.contains("STOR-D37"),
            "the STOR-D37 permanent restore gate must be named"
        );
        assert!(
            md.contains("truth-up pass holds"),
            "the truth-up pass must be named"
        );
        assert!(
            md.contains("P-546"),
            "the M7 fail-closed production-release gate must be named"
        );
        assert!(
            md.contains("dogfood-complete") && md.contains("NOT production-ready"),
            "the dogfood-complete-NOT-production-ready framing must be printed"
        );
    }

    #[test]
    fn m6_dropping_any_row_reds_the_gate() {
        for dropped in m6_required_rows() {
            let mut card = Scorecard::new(Band::M6Dogfood);
            for r in m6_required_rows()
                .into_iter()
                .filter(|r| r.id != dropped.id)
            {
                card.record(RowResult::pass(r.id, "[2026-06-26] PASS", "2026-06-26"));
            }
            assert_eq!(card.missing_required(), vec![dropped.id]);
            assert!(
                !card.is_green(),
                "dropping {} must RED the M6→M7 gate",
                dropped.id
            );
        }
    }

    #[test]
    fn m6_claimed_not_proven_row_reds_the_gate() {
        let mut card = Scorecard::new(Band::M6Dogfood);
        for r in m6_required_rows() {
            if r.id == "STOR-D37" {
                card.record(RowResult::claimed_not_proven(
                    r.id,
                    "restore-verify on Myelin's own commits did not reach a consistent point - recorded RED",
                    "2026-06-26",
                ));
            } else {
                card.record(RowResult::pass(r.id, "[2026-06-26] PASS", "2026-06-26"));
            }
        }
        assert!(!card.is_green(), "a claimed-not-proven M6 row blocks M7");
        assert_eq!(card.not_proven().len(), 1);
        assert!(card
            .render_markdown("2026-06-26")
            .contains("RED - M7 is BLOCKED"));
    }
}

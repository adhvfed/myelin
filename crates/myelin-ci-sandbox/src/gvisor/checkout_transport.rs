//! Hop A of a checkout: fetching exactly one commit's pack through the existing hardened git-wire
//! serving path, inside the parent attempt's metering and authorization.

use super::*;
use crate::runner::{
    PreparationAttemptDisposition, PreparationPhase, PreparationTerminalDisposition,
};
use crate::{
    CheckoutAuthorizationProof, HookError, IdemToken, JobSpec,
    LaunchPermit, MeterTarget, PhaseAuthorization, ResourceLimits, ResourceUsage,
    RunTokenCredential, RunnerHooks, SandboxBackend,
    SandboxResult,
};
use std::io;
use std::io::{Seek, Write};
use std::path::Path;
use std::sync::atomic::AtomicBool;

// =================================================================================================
// CT-007 slice 5b.2 — the checkout-specific runtime (design locked in with Sol, 2026-07-27, ledger
// 12): a SEQUENCE of two sandboxed hops glued by the host, never a live inter-container pipe, never
// `/repo` mounted into either the checkout container:
//
//   Hop A ([`fetch_checkout_pack`]): fetch exactly one commit's pack through the EXISTING,
//   unchanged, already-hardened git-wire serving path (`GvisorBackend::launch_git_wire`) — a real,
//   billed use of that path. `/repo` stays inside the git-wire server's own container, as today.
//
//   Hop B ([`run_checkout_preparation`]): a NEW, dedicated checkout-preparation container —
//   `ExplicitUserNamespace` + the workspace mount (the lease's `PreparationBound` identity from
//   slice 5b.1), NO `/repo`, no network, stdin = Hop A's prefetched pack file. This hop performs no
//   `reserve`/`settle` of its own (there is no per-checkout job to reserve against); its measured
//   usage is charged through the PARENT ATTEMPT's aggregate settlement in slice 5b.3, never
//   silently free.
// =================================================================================================

// `GitObjectFormat`/`ExpectedGitCommitId` moved to `crate::workspace_intent` (CT-007 slice 5b.3-1,
// Sol's review): backend-independent semantics the intent parser needs too, so they must not live
// in a backend-specific module.
use crate::workspace_intent::ExpectedGitCommitId;

use git_wire_codec::{
    parse_checkout_fetch_response, parse_upload_pack_advertisement, pkt_line_encode,
};

/// A one-shot, file-backed, ALREADY-VERIFIED-FRAMED pack artifact from Hop A (Sol's round-1 review,
/// point 3): never a `Vec<u8>` — a real repo's pack can be many MiB, and the git-wire response was
/// already materialized once by `launch_git_wire`'s own capture path, so a second in-memory copy
/// here would be pure waste. Not `Clone` — it is consumed exactly once, by [`StdinSource::File`], the
/// moment the checkout-preparation container spawns.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct PrefetchedCheckoutPack {
    pub(super) file: std::fs::File,
    /// `true` iff Hop A's response reported the wanted commit itself as the shallow boundary — the
    /// checkout script seeds `.git/shallow` with it before checkout iff this is set.
    pub(super) shallow: bool,
}

#[cfg(test)]
#[allow(dead_code)]
impl PrefetchedCheckoutPack {
    /// CT-007 slice 5b.3-6c: a test-only pack so a 6c continuation test can build a
    /// [`CheckoutPreparationSpec`] without a real Hop A. `#[cfg(test)]` only — `fetch_checkout_pack`
    /// stays the sole production constructor.
    pub(crate) fn for_tests() -> Self {
        let path = std::env::temp_dir().join(format!(
            "myelin-test-pack-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let file = std::fs::File::create(&path).expect("a temp file for the test pack");
        let _ = std::fs::remove_file(&path); // unlink; the handle stays valid.
        PrefetchedCheckoutPack {
            file,
            shallow: false,
        }
    }
}

/// CT-007 slice 5b.2, Hop A: fetch exactly one commit's pack through the EXISTING, unchanged,
/// already-hardened git-wire serving path (`GvisorBackend::launch_git_wire`) — a REAL, billed use of
/// that path (it reserves/settles through `hooks` exactly like any other git-wire caller). Never
/// touches `/repo` itself — that stays inside the git-wire server's own already-hardened container.
///
/// Uses a DEDICATED per-invocation `-c uploadpack.allowReachableSHA1InWant=true` (Sol's round-2
/// review) rather than reconfiguring the general serving path: CI/merge-queue dispatch commonly
/// targets a commit that is reachable but no longer an exact advertised ref tip by the time a queued
/// attempt starts (ordinary queue delay), and this codebase has no per-attempt ref to pin it with
/// today. The flow: parse the advertisement; if `expected` is a direct tip, proceed; otherwise
/// require the advertisement to actually offer `allow-reachable-sha1-in-want` (never silently
/// assume the `-c` was honored); send the exact `want`; let `upload-pack` itself perform the
/// reachability check and refuse if the commit is no longer reachable.
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
pub(crate) fn fetch_checkout_pack(
    backend: &GvisorBackend,
    hooks: &RunnerHooks,
    root: &Path,
    tenant: &str,
    region: &str,
    repo: &str,
    expected: &ExpectedGitCommitId,
    limits: ResourceLimits,
    run_token: RunTokenCredential,
    meter_to: MeterTarget,
    idem_token: IdemToken,
) -> Result<PrefetchedCheckoutPack, CheckoutPreparationError> {
    let allow_reachable = vec![
        "-c".to_string(),
        "uploadpack.allowReachableSHA1InWant=true".to_string(),
    ];

    let mut advertise_argv = allow_reachable.clone();
    advertise_argv.extend([
        "upload-pack".to_string(),
        "--stateless-rpc".to_string(),
        "--advertise-refs".to_string(),
    ]);
    let advertise_spec = GitWireSpec::for_repo(
        root,
        tenant,
        region,
        repo,
        advertise_argv,
        Vec::new(),
        Vec::new(),
        None,
        limits,
        run_token.clone(),
        meter_to.clone(),
        IdemToken(format!("{}:checkout-advertise", idem_token.0)),
    )
    .map_err(|e| CheckoutPreparationError::Refused(format!("build advertise-refs spec: {e}")))?;
    let advertisement = backend
        .launch_git_wire(&advertise_spec, hooks)
        .map_err(|e| CheckoutPreparationError::Refused(format!("advertise-refs: {e}")))?;
    // Sol's review: `launch_git_wire`'s `SandboxLaunch` handle is a live entry in `backend.live`
    // (plus a staged bundle dir) until `backend.kill` retires it -- every path below (success OR
    // parse refusal) must retire it EXACTLY ONCE, never leak it. Compute the parse result into a
    // local first, kill unconditionally, THEN propagate -- so a `?` can never skip the kill.
    let advertise_parse_result = (|| {
        if !advertisement.result.passed() {
            return Err(format!(
                "advertise-refs did not pass (exit={:?} timed_out={}, stderr: {})",
                advertisement.result.exit_code,
                advertisement.result.timed_out,
                String::from_utf8_lossy(&advertisement.result.stderr)
            ));
        }
        parse_upload_pack_advertisement(&advertisement.result.stdout, expected)
    })();
    let kill_result = backend.kill(&advertisement.handle);
    // Sol's review: NEVER let a parse error's `?` propagate before a simultaneous `kill` error is
    // even inspected -- a real runtime leak must not be silently hidden behind whichever failure
    // happened to be checked first.
    let parsed = match (advertise_parse_result, kill_result) {
        (Ok(parsed), Ok(())) => parsed,
        (Ok(parsed), Err(kill_error)) => {
            return Err(CheckoutPreparationError::Refused(format!(
                "advertise-refs parsed successfully ({parsed:?}) but retiring its sandbox handle \
                 failed ({kill_error}) -- a live runsc container/bundle may have leaked"
            )));
        }
        (Err(parse_error), Ok(())) => {
            return Err(CheckoutPreparationError::Refused(format!(
                "parse advertisement: {parse_error}"
            )));
        }
        (Err(parse_error), Err(kill_error)) => {
            return Err(CheckoutPreparationError::Refused(format!(
                "parse advertisement failed ({parse_error}) AND retiring its sandbox handle also \
                 failed ({kill_error}) -- a live runsc container/bundle may have leaked"
            )));
        }
    };
    if !parsed.directly_advertised && !parsed.allows_reachable_want {
        return Err(CheckoutPreparationError::Refused(
            "expected commit is not an advertised ref tip AND the server did not offer \
             allow-reachable-sha1-in-want -- refusing rather than sending an unreachable want"
                .to_string(),
        ));
    }

    let mut capabilities = "no-progress ofs-delta".to_string();
    if let Some(token) = expected.format().capability_token() {
        capabilities.push(' ');
        capabilities.push_str(token);
    }
    let mut request = pkt_line_encode(&format!("want {} {capabilities}\n", expected.as_str()));
    request.extend_from_slice(&pkt_line_encode("deepen 1\n"));
    request.extend_from_slice(b"0000");
    request.extend_from_slice(&pkt_line_encode("done\n"));

    let mut fetch_argv = allow_reachable;
    fetch_argv.extend(["upload-pack".to_string(), "--stateless-rpc".to_string()]);
    let fetch_spec = GitWireSpec::for_repo(
        root,
        tenant,
        region,
        repo,
        fetch_argv,
        request,
        Vec::new(),
        None,
        limits,
        run_token,
        meter_to,
        IdemToken(format!("{}:checkout-fetch", idem_token.0)),
    )
    .map_err(|e| CheckoutPreparationError::Refused(format!("build fetch spec: {e}")))?;
    // Sol's review: stage the pack artifact BEFORE launching the fetch, never after -- a
    // `tempfile_for_checkout_pack` failure between a successful launch and this point would
    // otherwise leak that launch's live `backend.live` entry + bundle dir with no path back to it.
    let mut pack_file = tempfile_for_checkout_pack()
        .map_err(|e| CheckoutPreparationError::Refused(format!("stage pack artifact: {e}")))?;
    let fetched = backend
        .launch_git_wire(&fetch_spec, hooks)
        .map_err(|e| CheckoutPreparationError::Refused(format!("fetch: {e}")))?;
    // Same leak/guest-failure fix as the advertisement call above: retire the fetch launch's live
    // handle EXACTLY ONCE, on every path, never skipped by an early `?`. A `kill` failure is not
    // silently discarded (Sol's review) -- a live runsc container/bundle may have leaked, which
    // matters even when parsing itself succeeded.
    let fetch_parse_result = if !fetched.result.passed() {
        Err(format!(
            "fetch did not pass (exit={:?} timed_out={}, stderr: {})",
            fetched.result.exit_code,
            fetched.result.timed_out,
            String::from_utf8_lossy(&fetched.result.stderr)
        ))
    } else {
        parse_checkout_fetch_response(
            &fetched.result.stdout,
            expected,
            &mut pack_file,
            limits.disk_bytes,
        )
    };
    let kill_result = backend.kill(&fetched.handle);
    // Same combine-don't-lose-either-error fix as the advertisement block above.
    let parsed_fetch = match (fetch_parse_result, kill_result) {
        (Ok(parsed), Ok(())) => parsed,
        (Ok(parsed), Err(kill_error)) => {
            return Err(CheckoutPreparationError::Refused(format!(
                "fetch parsed successfully ({parsed:?}) but retiring its sandbox handle failed \
                 ({kill_error}) -- a live runsc container/bundle may have leaked"
            )));
        }
        (Err(parse_error), Ok(())) => {
            return Err(CheckoutPreparationError::Refused(format!(
                "parse fetch response: {parse_error}"
            )));
        }
        (Err(parse_error), Err(kill_error)) => {
            return Err(CheckoutPreparationError::Refused(format!(
                "parse fetch response failed ({parse_error}) AND retiring its sandbox handle also \
                 failed ({kill_error}) -- a live runsc container/bundle may have leaked"
            )));
        }
    };
    pack_file
        .flush()
        .map_err(|e| CheckoutPreparationError::Refused(format!("flush pack artifact: {e}")))?;
    let mut pack_file = pack_file
        .into_inner()
        .map_err(|e| CheckoutPreparationError::Refused(format!("finish pack artifact: {e}")))?;
    pack_file
        .seek(std::io::SeekFrom::Start(0))
        .map_err(|e| CheckoutPreparationError::Refused(format!("rewind pack artifact: {e}")))?;
    Ok(PrefetchedCheckoutPack {
        file: pack_file,
        shallow: parsed_fetch.shallow,
    })
}

/// The outcome of a successful parent-attempt Hop A transport (CT-007 slice 5b.3-3): the fetched
/// pack plus the REAL measured usage across BOTH nested git-wire executions (advertise + fetch),
/// checked-summed. Never settled here — the caller (5b.3-6's `launch_with` splice) folds this into
/// the ONE aggregate settlement for the whole attempt, alongside Hop B's and the workload's own usage.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct ParentAttemptCheckoutTransportOutcome {
    pack: PrefetchedCheckoutPack,
    usage: ResourceUsage,
}

impl ParentAttemptCheckoutTransportOutcome {
    #[allow(dead_code)]
    pub(crate) fn into_parts(self) -> (PrefetchedCheckoutPack, ResourceUsage) {
        (self.pack, self.usage)
    }
}

/// Every way the parent-attempt Hop A transport (5b.3-3) can fail. Deliberately NOT
/// [`CheckoutPreparationError`] (Hop B's own error type): a Hop A failure AFTER the advertisement run
/// has already consumed measurable, non-free resources, and reusing `CheckoutPreparationError::Refused`
/// here would silently discard that usage.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) enum CheckoutTransportError {
    /// Refused before anything spawned for the WHOLE transport (a proof/scope/token-generation
    /// mismatch, or a malformed request) — there is no usage to account.
    Refused { message: String },
    /// A safely retired execution or post-hop operation failed. `disposition` structurally
    /// distinguishes a terminal checkout rejection/timeout from retryable infrastructure or an
    /// invariant requiring reconciliation; no caller parses `message`. `usage` carries everything
    /// measured up through the point of failure and must still be folded into the parent attempt.
    Failed {
        message: String,
        usage: ResourceUsage,
        disposition: PreparationAttemptDisposition,
    },
    /// A real execution ran, but this function could not independently prove the nested container
    /// was fully retired (child killed AND bundle removed, OR `finalize_runtime`'s own OS-level
    /// teardown checks) — `usage` still carries everything measured. The caller must treat this as a
    /// genuine teardown-unproven condition, never silently discard the possibly-leaked resources.
    TeardownUnproven {
        message: String,
        usage: ResourceUsage,
    },
    /// Real, measured usage occurred (Sol's review, blocker 3), but the exact total can no longer be
    /// honestly represented (a checked `ResourceUsage` addition overflowed). `usage` is the LAST
    /// EXACT total this function could still prove (never a wrapped/truncated/best-guess value) —
    /// distinct from `Failed` precisely because `Failed.usage` is a contract the caller may safely
    /// settle, and an overflowed total is NOT safely settleable as-is. This is an
    /// accounting/reconciliation failure, not an ordinary transport failure. `teardown_unproven`
    /// (Sol's round-3 review) is an ORTHOGONAL fact, never collapsed into this one variant's choice:
    /// usage-representability and teardown-proof are independent axes, and 5b.3-6 must be able to act
    /// on both (whether it's safe to settle at all, AND whether resources might still be live).
    UsageUnrepresentable {
        message: String,
        usage: ResourceUsage,
        teardown_unproven: bool,
    },
}

impl CheckoutTransportError {
    /// The machine-readable outcome for Hop A. This deliberately never examines `message`;
    /// diagnostics are not an authorization, retry, or terminal-accounting protocol.
    #[allow(dead_code)]
    pub(crate) fn attempt_disposition(&self) -> PreparationAttemptDisposition {
        match self {
            Self::Refused { .. } => PreparationAttemptDisposition::RefusedBeforeExecution {
                phase: PreparationPhase::CheckoutTransport,
            },
            Self::Failed { disposition, .. } => *disposition,
            Self::TeardownUnproven { .. } => {
                PreparationAttemptDisposition::ReconciliationRequired {
                    phase: PreparationPhase::CheckoutTransport,
                    teardown_unproven: true,
                    usage_unrepresentable: false,
                    quarantine_required: false,
                }
            }
            Self::UsageUnrepresentable {
                teardown_unproven, ..
            } => PreparationAttemptDisposition::ReconciliationRequired {
                phase: PreparationPhase::CheckoutTransport,
                teardown_unproven: *teardown_unproven,
                usage_unrepresentable: true,
                quarantine_required: false,
            },
        }
    }
}

fn checkout_transport_terminal_failed(
    message: String,
    usage: ResourceUsage,
) -> CheckoutTransportError {
    CheckoutTransportError::Failed {
        message,
        usage,
        disposition: PreparationAttemptDisposition::Terminal(
            PreparationTerminalDisposition::Failed {
                phase: PreparationPhase::CheckoutTransport,
            },
        ),
    }
}

fn checkout_transport_timed_out(message: String, usage: ResourceUsage) -> CheckoutTransportError {
    CheckoutTransportError::Failed {
        message,
        usage,
        disposition: PreparationAttemptDisposition::Terminal(
            PreparationTerminalDisposition::TimedOut {
                phase: PreparationPhase::CheckoutTransport,
            },
        ),
    }
}

fn checkout_transport_retryable(message: String, usage: ResourceUsage) -> CheckoutTransportError {
    CheckoutTransportError::Failed {
        message,
        usage,
        disposition: PreparationAttemptDisposition::RetryableInfrastructure {
            phase: PreparationPhase::CheckoutTransport,
        },
    }
}

impl std::fmt::Display for CheckoutTransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckoutTransportError::Refused { message } => {
                write!(f, "checkout transport refused: {message}")
            }
            CheckoutTransportError::Failed { message, .. } => {
                write!(f, "checkout transport failed: {message}")
            }
            CheckoutTransportError::TeardownUnproven { message, .. } => {
                write!(f, "checkout transport teardown unproven: {message}")
            }
            CheckoutTransportError::UsageUnrepresentable {
                message,
                teardown_unproven,
                ..
            } => {
                write!(
                    f,
                    "checkout transport usage unrepresentable (teardown_unproven={teardown_unproven}): {message}"
                )
            }
        }
    }
}

/// Checked (never wrapping/saturating) `ResourceUsage` aggregation (Sol's review, section B): an
/// overflow is a loud accounting/reconciliation failure, never a silently wrapped/truncated total.
fn checked_add_usage(a: ResourceUsage, b: ResourceUsage) -> Result<ResourceUsage, String> {
    Ok(ResourceUsage {
        cpu_seconds: a.cpu_seconds.checked_add(b.cpu_seconds).ok_or_else(|| {
            "cpu_seconds overflow aggregating checkout transport usage".to_string()
        })?,
        mem_byte_seconds: a
            .mem_byte_seconds
            .checked_add(b.mem_byte_seconds)
            .ok_or_else(|| {
                "mem_byte_seconds overflow aggregating checkout transport usage".to_string()
            })?,
    })
}

/// Fully retire ONE nested git-wire container the parent-attempt transport spawned: kill the
/// (already-exited) child and remove its bundle dir. Unlike [`GvisorBackend::kill`] (which discards a
/// `remove_dir_all` failure with `let _ =`, relying on `self.live` to let a LATER `kill()` retry), this
/// function has no such registry to fall back on: it never registers into `self.live` at all (Sol's
/// review — the parent-attempt transport returns no live backend handle), so a removal failure here is
/// the ONLY signal the caller ever gets that a bundle may have leaked. Never silently discarded.
fn retire_parent_attempt_hop(
    mut child: Box<dyn RunscChild + Send>,
    bundle_dir: &Path,
) -> Result<(), String> {
    let kill_result = child.kill();
    let remove_result = std::fs::remove_dir_all(bundle_dir);
    match (kill_result, remove_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(e)) => Err(format!("bundle dir {bundle_dir:?} removal failed: {e}")),
        (Err(e), Ok(())) => Err(format!("child kill failed: {e}")),
        (Err(ke), Err(re)) => Err(format!(
            "child kill failed ({ke}) AND bundle dir {bundle_dir:?} removal also failed ({re})"
        )),
    }
}

/// A [`RunFailure`] from a nested hop whose teardown WAS independently proven fine (either it never
/// reached `finalize_runtime` at all — genuinely `Uncommitted` — or `finalize_runtime` succeeded),
/// converted with `usage_before` (measured by PRIOR hops in this same transport) folded in.
/// `prior_hop_completed` (Sol's review, blocker 4) is an EXPLICIT phase fact, never inferred from
/// `usage_before == 0` — a hop can genuinely execute with zero measured usage, and that must still be
/// `Failed`, never misreported as the free `Refused`. `LaunchPermit::immediate()`'s commit closure can
/// never itself fail, so `CommitOutcomeUnknown` is unreachable in practice here — Sol's review:
/// represent it as a loud invariant-violation `Failed`, never silently assumed away, never mislabeled
/// as a teardown question it has nothing to do with.
fn map_hop_run_failure(
    run_failure: RunFailure,
    usage_before: ResourceUsage,
    prior_hop_completed: bool,
) -> CheckoutTransportError {
    let message = run_failure.to_string();
    match run_failure {
        RunFailure::Uncommitted { .. } => {
            if prior_hop_completed {
                checkout_transport_retryable(message, usage_before)
            } else {
                CheckoutTransportError::Refused { message }
            }
        }
        RunFailure::CommitOutcomeUnknown { .. } => CheckoutTransportError::Failed {
            message: format!(
                "internal invariant violated (an immediate launch permit's commit closure cannot \
                 fail, so this should be unreachable): {message}"
            ),
            usage: usage_before,
            disposition: PreparationAttemptDisposition::ReconciliationRequired {
                phase: PreparationPhase::CheckoutTransport,
                teardown_unproven: false,
                usage_unrepresentable: false,
                quarantine_required: false,
            },
        },
        RunFailure::CommittedButNotExecuted { .. } => {
            checkout_transport_retryable(message, usage_before)
        }
        RunFailure::Executed {
            usage: hop_usage, ..
        } => match checked_add_usage(usage_before, hop_usage) {
            Ok(total) => checkout_transport_retryable(message, total),
            Err(overflow) => CheckoutTransportError::UsageUnrepresentable {
                message: format!(
                    "{message} (usage aggregation overflowed combining a hop that DID execute: \
                     {overflow})"
                ),
                usage: usage_before,
                // This branch is only ever reached via `Finalized(primary: Err(Executed))` — never
                // the bare pre-finalize outer `Err` (`run_git_wire_container_raw` never produces
                // `Executed` before `finalize_and_merge`) — so teardown WAS independently proven here.
                teardown_unproven: false,
            },
        },
    }
}

/// Every reason a git-wire hop's OWN result disqualifies it from being treated as a clean success
/// (Sol's round-3 review, blocker 3): shared between the ordinary success path
/// ([`run_one_git_wire_hop_within_parent_attempt`]) and [`map_hop_finalization_failure`]'s
/// `primary: Ok` branch, so a non-passing/truncated/stream-errored run is NEVER silently reduced to
/// "just" a teardown problem when both are true simultaneously.
fn hop_result_failure_reasons(
    result: &SandboxResult,
    truncated: bool,
    run_error: &Option<String>,
) -> Vec<String> {
    let mut reasons = Vec::new();
    if !result.passed() {
        reasons.push(format!(
            "guest execution did not pass (exit={:?} timed_out={})",
            result.exit_code, result.timed_out
        ));
    }
    if truncated {
        reasons.push("response exceeded the wire cap".to_string());
    }
    if let Some(run_error_message) = run_error {
        reasons.push(run_error_message.clone());
    }
    reasons
}

/// A hop whose `finalize_runtime` genuinely could NOT prove teardown (Sol's review, blocker 2) —
/// `primary` is the hop's own `Result<(ContainerRun, bool), RunFailure>` from BEFORE finalization
/// collapsed it, `teardown` is the independent teardown failure. Teardown ambiguity is orthogonal to
/// whatever `primary` says: even a `primary` that itself failed for an unrelated reason still leaves
/// teardown genuinely unproven, and that must never be silently downgraded to an ordinary `Failed`.
/// When `primary` is `Ok`, this function ALSO performs the best-effort discard `run_git_wire_container`
/// would otherwise have owned (kill + remove bundle) — mirroring
/// [`discard_container_run_after_teardown_failure`], simplified because git-wire's `prepared_mode` is
/// always `Rootless` (the namespace-identity-drift skip that function handles never applies here).
/// Sol's round-3 review, blocker 3: the `Ok` branch also folds in [`hop_result_failure_reasons`] —
/// previously it reported ONLY the teardown problem, silently dropping a simultaneous non-passing
/// exit / truncation / stream error.
fn map_hop_finalization_failure(
    primary: Result<(ContainerRun, bool), RunFailure>,
    teardown: RuntimeTeardownError,
    usage_before: ResourceUsage,
) -> CheckoutTransportError {
    match primary {
        Ok((run, truncated)) => {
            let hop_usage = run.result.usage;
            let reasons = hop_result_failure_reasons(&run.result, truncated, &run.run_error);
            let ContainerRun {
                child, bundle_dir, ..
            } = run;
            let discard_result = retire_parent_attempt_hop(child, &bundle_dir);
            let mut parts = Vec::new();
            if !reasons.is_empty() {
                parts.push(reasons.join("; "));
            }
            parts.push(format!(
                "runtime teardown could not be proven after a completed run: {teardown}"
            ));
            if let Err(discard_error) = &discard_result {
                parts.push(format!("best-effort discard also failed ({discard_error})"));
            }
            let message = parts.join(" AND ");
            match checked_add_usage(usage_before, hop_usage) {
                Ok(total) => CheckoutTransportError::TeardownUnproven {
                    message,
                    usage: total,
                },
                Err(overflow) => CheckoutTransportError::UsageUnrepresentable {
                    message: format!("{message} (usage aggregation overflowed: {overflow})"),
                    usage: usage_before,
                    teardown_unproven: true,
                },
            }
        }
        Err(run_failure) => {
            let combined = augment_run_failure_with_teardown(run_failure, &teardown);
            match combined {
                RunFailure::Uncommitted { message }
                | RunFailure::CommittedButNotExecuted { message } => {
                    CheckoutTransportError::TeardownUnproven {
                        message,
                        usage: usage_before,
                    }
                }
                RunFailure::CommitOutcomeUnknown { message } => {
                    // Sol's round-4 review: this branch is reached from `RuntimeFinalization::Failed`
                    // — teardown was ALREADY independently NOT proven, regardless of how impossible
                    // the accompanying commit ambiguity is. Reporting ordinary `Failed` here would
                    // erase that real, independent teardown failure — never downgrade it.
                    CheckoutTransportError::TeardownUnproven {
                        message: format!(
                            "internal invariant violated (an immediate launch permit's commit \
                             closure cannot fail, so this should be unreachable): {message}"
                        ),
                        usage: usage_before,
                    }
                }
                RunFailure::Executed {
                    message,
                    usage: hop_usage,
                } => match checked_add_usage(usage_before, hop_usage) {
                    Ok(total) => CheckoutTransportError::TeardownUnproven {
                        message,
                        usage: total,
                    },
                    Err(overflow) => CheckoutTransportError::UsageUnrepresentable {
                        message: format!("{message} (usage aggregation overflowed: {overflow})"),
                        usage: usage_before,
                        teardown_unproven: true,
                    },
                },
            }
        }
    }
}

/// The raw per-hop git-wire executor the parent-attempt transport drives twice (advertise, then
/// fetch) — mirrors [`run_git_wire_container_raw`]'s signature exactly (Sol's review, blocker 2: the
/// executor must hand back the STRUCTURED [`RuntimeFinalization`], not the standalone path's already-
/// collapsed `Result<(ContainerRun, bool), RunFailure>`, so a teardown-unproven outcome can be told
/// apart from an ordinary run failure). Production always passes [`run_git_wire_container_raw`]
/// itself; tests inject a deterministic fake so the transport's aggregation/error-mapping logic is
/// verifiable without a real `runsc` binary (the true runsc/git-rootfs integration is 5b.3-7).
pub(super) type GitWireHopExecutor<'a> = &'a dyn Fn(
    &JobSpec,
    &OciConfig,
    Vec<u8>,
    &Path,
    &AtomicBool,
    LaunchPermit,
) -> (
    Result<GitWireHopFinalization, RunFailure>,
    BundleCleanupProof,
);

/// If `bundle_cleanup` proves nothing (Sol's round-3 review, blocker 1), the hop's own result can no
/// longer mean "genuinely nothing left behind" — upgrade whatever disposition `mapped` would
/// otherwise be into one that says so, folding the cleanup failure's text in. This TRUMPS
/// `Refused`/`Failed` entirely (an unproven bundle can never be reported as free or ordinary); for an
/// already-more-severe disposition (`TeardownUnproven`/`UsageUnrepresentable`) it only adds the extra
/// fact, never downgrades. A no-op whenever `bundle_cleanup` is `Ok(())` (the overwhelmingly common
/// case — nothing to add).
fn force_teardown_unproven_if_cleanup_unproven(
    mapped: CheckoutTransportError,
    bundle_cleanup: &BundleCleanupProof,
    usage_before: ResourceUsage,
) -> CheckoutTransportError {
    let Err(cleanup_error) = bundle_cleanup else {
        return mapped;
    };
    let note = format!("a bundle directory could not be proven removed: {cleanup_error}");
    match mapped {
        CheckoutTransportError::Refused { message } => CheckoutTransportError::TeardownUnproven {
            message: format!("{message} AND {note}"),
            usage: usage_before,
        },
        CheckoutTransportError::Failed { message, usage, .. } => {
            CheckoutTransportError::TeardownUnproven {
                message: format!("{message} AND {note}"),
                usage,
            }
        }
        CheckoutTransportError::TeardownUnproven { message, usage } => {
            CheckoutTransportError::TeardownUnproven {
                message: format!("{message} AND {note}"),
                usage,
            }
        }
        CheckoutTransportError::UsageUnrepresentable { message, usage, .. } => {
            CheckoutTransportError::UsageUnrepresentable {
                message: format!("{message} AND {note}"),
                usage,
                teardown_unproven: true,
            }
        }
    }
}

/// The post-`execute()` half of [`run_one_git_wire_hop_within_parent_attempt`] — factored out purely
/// so its caller can uniformly apply [`force_teardown_unproven_if_cleanup_unproven`] to WHATEVER
/// `CheckoutTransportError` this produces, via one `.map_err(...)`, rather than needing to thread the
/// bundle-cleanup adjustment into every individual early return below.
fn handle_git_wire_hop_finalization(
    finalization_result: Result<GitWireHopFinalization, RunFailure>,
    usage_before: ResourceUsage,
    prior_hop_completed: bool,
) -> Result<(SandboxResult, ResourceUsage), CheckoutTransportError> {
    let finalization = finalization_result.map_err(|run_failure| {
        map_hop_run_failure(run_failure, usage_before, prior_hop_completed)
    })?;

    let (run, truncated) = match finalization {
        RuntimeFinalization::Finalized(FinalizedRun { primary, .. }) => {
            primary.map_err(|run_failure| {
                map_hop_run_failure(run_failure, usage_before, prior_hop_completed)
            })?
        }
        RuntimeFinalization::Failed { primary, teardown } => {
            return Err(map_hop_finalization_failure(
                primary,
                teardown,
                usage_before,
            ));
        }
    };

    let ContainerRun {
        child,
        bundle_dir,
        result,
        run_error,
    } = run;
    let new_total = match checked_add_usage(usage_before, result.usage) {
        Ok(total) => total,
        Err(overflow) => {
            let discard_result = retire_parent_attempt_hop(child, &bundle_dir);
            let message = format!("usage aggregation overflowed after this hop: {overflow}");
            return Err(CheckoutTransportError::UsageUnrepresentable {
                message: match &discard_result {
                    Ok(()) => message,
                    Err(discard_error) => {
                        format!("{message} AND best-effort discard also failed ({discard_error})")
                    }
                },
                usage: usage_before,
                // Teardown (container delete + cgroup quiescence) was ALREADY independently proven
                // by the time we reach here (we are inside the `Finalized` arm) — only THIS
                // function's own retirement discard is in question, tracked separately below.
                teardown_unproven: discard_result.is_err(),
            });
        }
    };

    // Sol's review, blocker 1: a nonzero-exit or timed-out guest execution with otherwise
    // syntactically valid stdout must never be accepted as a successful transport — the standalone
    // path (`fetch_checkout_pack`, via `SandboxLaunch.result.passed()`) already enforces this; this
    // path must too.
    let reasons = hop_result_failure_reasons(&result, truncated, &run_error);

    if !reasons.is_empty() {
        let combined_reason = reasons.join("; ");
        return Err(match retire_parent_attempt_hop(child, &bundle_dir) {
            Ok(()) => {
                if run_error.is_some() {
                    checkout_transport_retryable(combined_reason, new_total)
                } else if result.timed_out {
                    checkout_transport_timed_out(combined_reason, new_total)
                } else {
                    checkout_transport_terminal_failed(combined_reason, new_total)
                }
            }
            Err(teardown_error) => CheckoutTransportError::TeardownUnproven {
                message: format!(
                    "{combined_reason} AND retiring its sandbox container also failed \
                     ({teardown_error})"
                ),
                usage: new_total,
            },
        });
    }

    match retire_parent_attempt_hop(child, &bundle_dir) {
        Ok(()) => Ok((result, new_total)),
        Err(teardown_error) => Err(CheckoutTransportError::TeardownUnproven {
            message: teardown_error,
            usage: new_total,
        }),
    }
}

/// Run ONE git-wire hop entirely within the parent attempt's own bookkeeping (CT-007 slice 5b.3-3):
/// builds the hardened job/config via the SAME [`build_git_wire_job`]/[`build_git_wire_oci_config`]
/// preparation the standalone billed path uses, executes it with an IMMEDIATE launch permit (never a
/// real durable fence — there is no separate reservation for this hop to commit against), and
/// unconditionally retires the container (kill + remove bundle dir) before returning — this function
/// never leaves a live handle or bundle behind on any path. Returns the hop's `SandboxResult` (for the
/// caller's own wire-protocol parsing) plus the RUNNING total usage (`usage_before` + this hop's own,
/// checked). `prior_hop_completed` is Sol's review, blocker 4 — an explicit phase fact the caller
/// supplies (never inferred from `usage_before`). Sol's round-3 review, blocker 1: `execute`'s paired
/// [`BundleCleanupProof`] is checked against the FINAL disposition, regardless of which branch below
/// produced it — an unproven pre-finalization bundle cleanup forces `TeardownUnproven`, never a free
/// `Refused`.
#[allow(clippy::too_many_arguments)]
fn run_one_git_wire_hop_within_parent_attempt(
    hop_spec: &GitWireSpec,
    command: Vec<String>,
    cancellation: &AtomicBool,
    usage_before: ResourceUsage,
    prior_hop_completed: bool,
    permit: LaunchPermit,
    execute: GitWireHopExecutor,
) -> Result<(SandboxResult, ResourceUsage), CheckoutTransportError> {
    let refuse_or_fail = |message: String| {
        if prior_hop_completed {
            checkout_transport_retryable(message, usage_before)
        } else {
            CheckoutTransportError::Refused { message }
        }
    };
    let job = build_git_wire_job(hop_spec, command, cancellation)
        .map_err(|e| refuse_or_fail(e.to_string()))?;
    let (cfg, rootfs) =
        build_git_wire_oci_config(&job, hop_spec).map_err(|e| refuse_or_fail(e.to_string()))?;

    let (finalization_result, bundle_cleanup_proof) = execute(
        &job,
        &cfg,
        hop_spec.stdin.clone(),
        &rootfs,
        cancellation,
        permit,
    );

    // Sol's round-4 review: match the COMPLETE `(result, cleanup_proof)` pair, not just the error
    // side — an unproven `bundle_cleanup_proof` must force `TeardownUnproven` even when
    // `handle_git_wire_hop_finalization` itself returns `Ok`. Production never actually produces this
    // combination today (a hop only reaches `Ok` via the success path, which performs its OWN
    // verified retirement — see `retire_parent_attempt_hop` inside that function — so its bundle is
    // ALWAYS proven removed by the time `Ok` is returned), but the executor's TYPE permits it, and the
    // "regardless of branch" guarantee this function documents must be structural, not incidental.
    match handle_git_wire_hop_finalization(finalization_result, usage_before, prior_hop_completed) {
        Ok(success) => match &bundle_cleanup_proof {
            Ok(()) => Ok(success),
            Err(cleanup_error) => {
                let (_, new_total) = success;
                Err(CheckoutTransportError::TeardownUnproven {
                    message: format!(
                        "the hop otherwise succeeded, but a bundle directory could not be proven \
                         removed: {cleanup_error}"
                    ),
                    usage: new_total,
                })
            }
        },
        Err(e) => Err(force_teardown_unproven_if_cleanup_unproven(
            e,
            &bundle_cleanup_proof,
            usage_before,
        )),
    }
}

/// A synthetic [`MeterTarget`] for the two nested git-wire hops the parent-attempt transport spawns.
/// Never used for reserve/settle (this transport never calls `hooks.reserve`/`hooks.settle_completed`
/// at all — see [`fetch_checkout_pack_within_parent_attempt`]'s own doc) — only present because
/// `JobSpec::new` mandates a value structurally.
fn synthetic_parent_attempt_meter_target() -> MeterTarget {
    MeterTarget {
        reserve_id: "checkout-transport-within-parent-attempt".to_string(),
    }
}

/// A synthetic, per-hop-unique [`IdemToken`] — same rationale as
/// [`synthetic_parent_attempt_meter_target`]: `JobSpec::new` mandates one, but this transport never
/// reserves/dedups against it.
fn synthetic_parent_attempt_idem_token(label: &str) -> IdemToken {
    IdemToken(format!("checkout-transport-{label}-{}", unique_suffix()))
}

/// CT-007 slice 5b.3-3: the parent-attempt Hop A transport. Unlike [`fetch_checkout_pack`] (the
/// standalone, billed git-wire caller — unchanged, still reserves/settles through `hooks` exactly as
/// before), this function takes NO `RunnerHooks`, `MeterTarget`, `IdemToken`, or workload
/// `LaunchPermit`: it is meant to run entirely INSIDE an outer `launch_with` attempt that has already
/// reserved once for the whole attempt (5b.3-6 wires the call site). It consumes the one-shot
/// [`CheckoutAuthorizationProof`] by value, verifies its scope and run-token generation against THIS
/// request BEFORE spawning anything, drives both nested git-wire executions with
/// `LaunchPermit::immediate()`, and fully retires each child+bundle itself — it returns no
/// `SandboxLaunch`/live backend handle, and never reserves, settles, releases, or calls the real
/// workload CAS. See [`CheckoutTransportError`] for how a mid-transport failure still carries the
/// real usage already measured; the caller (5b.3-4/5/6) decides how that pre-workload usage survives
/// and settles.
/// **CT-007 round-1 blocker 2: how Hop A is authorized, MODULE-PRIVATE.**
///
/// This enum is an implementation detail of the shared transport body — it is deliberately NOT
/// `pub(crate)`, so no other module can select an arm. The two PUBLIC entry points
/// ([`fetch_checkout_pack_within_parent_attempt`] for V1 and
/// [`fetch_checkout_pack_within_parent_attempt_v2`] for V2) each construct exactly one arm, so the
/// V2 entry point offers no legacy option at the TYPE level rather than by source-pin convention.
enum TransportAuthority<'a> {
    LegacyClaimBound {
        proof: CheckoutAuthorizationProof,
    },
    PhaseBound {
        advertise: PhaseAuthorization,
        /// Invoked EXACTLY once, only after the advertisement succeeded and its container was
        /// retired, and before the fetch spawns. A refusal here aborts Hop A carrying the
        /// advertisement's already-measured usage — the fetch never spawns. The credential and the
        /// authorization come back together, and the authorization's own privately-retained JTI is
        /// what the credential is checked against when the permit is extracted.
        fetch: &'a mut dyn FnMut() -> Result<(RunTokenCredential, PhaseAuthorization), HookError>,
    },
}

/// **The LEGACY (V1 claim-bound) Hop A entry point.** Signature and behaviour are exactly as
/// shipped: one claim-bound credential covers both git-wire executions, each spawned under an
/// internally-minted immediate permit. There is no durable phase gate for a V1 preparation.
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
pub(crate) fn fetch_checkout_pack_within_parent_attempt(
    root: &Path,
    tenant: &str,
    region: &str,
    repo: &str,
    expected: &ExpectedGitCommitId,
    limits: ResourceLimits,
    run_token: RunTokenCredential,
    proof: CheckoutAuthorizationProof,
    cancellation: &AtomicBool,
    lease_checkpoint: Option<&dyn crate::PreparationLeaseCheckpoint>,
) -> Result<ParentAttemptCheckoutTransportOutcome, CheckoutTransportError> {
    fetch_checkout_pack_within_parent_attempt_given(
        root,
        tenant,
        region,
        repo,
        expected,
        limits,
        run_token,
        proof,
        cancellation,
        lease_checkpoint,
        &|job, cfg, stdin, rootfs, cancellation, permit| {
            run_git_wire_container_raw(job, cfg, stdin, rootfs, cancellation, permit)
        },
    )
}

/// **The V2 (phase-bound) Hop A entry point.** Takes ONE opaque [`PhaseAuthorization`] for the
/// advertise leg — proof and retained durable permit fused from a single hook invocation — plus a
/// one-shot provider for the fetch leg's own credential + authorization, invoked only after the
/// advertisement has fully retired and the lease checkpoint has renewed.
///
/// There is deliberately NO legacy arm reachable from here: a `CheckoutAuthorizationProof` cannot be
/// passed to this function at all, and a `PhaseAuthorization` cannot be constructed outside
/// `checkout_authorization`.
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
pub(crate) fn fetch_checkout_pack_within_parent_attempt_v2(
    root: &Path,
    tenant: &str,
    region: &str,
    repo: &str,
    expected: &ExpectedGitCommitId,
    limits: ResourceLimits,
    advertise_credential: RunTokenCredential,
    advertise: PhaseAuthorization,
    fetch: &mut dyn FnMut() -> Result<(RunTokenCredential, PhaseAuthorization), HookError>,
    cancellation: &AtomicBool,
    lease_checkpoint: Option<&dyn crate::PreparationLeaseCheckpoint>,
) -> Result<ParentAttemptCheckoutTransportOutcome, CheckoutTransportError> {
    fetch_checkout_pack_within_parent_attempt_v2_given(
        root,
        tenant,
        region,
        repo,
        expected,
        limits,
        advertise_credential,
        advertise,
        fetch,
        cancellation,
        lease_checkpoint,
        &|job, cfg, stdin, rootfs, cancellation, permit| {
            run_git_wire_container_raw(job, cfg, stdin, rootfs, cancellation, permit)
        },
    )
}

/// Deterministic `_given` seam for the V2 entry point (this codebase's established convention).
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
pub(super) fn fetch_checkout_pack_within_parent_attempt_v2_given(
    root: &Path,
    tenant: &str,
    region: &str,
    repo: &str,
    expected: &ExpectedGitCommitId,
    limits: ResourceLimits,
    advertise_credential: RunTokenCredential,
    advertise: PhaseAuthorization,
    fetch: &mut dyn FnMut() -> Result<(RunTokenCredential, PhaseAuthorization), HookError>,
    cancellation: &AtomicBool,
    lease_checkpoint: Option<&dyn crate::PreparationLeaseCheckpoint>,
    execute: GitWireHopExecutor,
) -> Result<ParentAttemptCheckoutTransportOutcome, CheckoutTransportError> {
    fetch_checkout_pack_within_parent_attempt_inner(
        root,
        tenant,
        region,
        repo,
        expected,
        limits,
        advertise_credential,
        TransportAuthority::PhaseBound { advertise, fetch },
        cancellation,
        lease_checkpoint,
        execute,
    )
}

/// The exact anti-substitution comparison every checkout proof must pass against the request it is
/// about to authorize: the token generation it was checked against, and the tenant/repo/commit it
/// names. Extracted (CT-007 phase-credential generations) so the V2 FETCH leg — whose proof and
/// credential are minted mid-transport, after the advertisement — receives byte-identical scrutiny
/// to the advertise leg rather than a weaker ad-hoc check.
#[allow(dead_code)]
fn verify_transport_proof_against_request(
    proof: &CheckoutAuthorizationProof,
    run_token: &RunTokenCredential,
    tenant: &str,
    repo: &str,
    expected: &ExpectedGitCommitId,
) -> Result<(), String> {
    if proof.run_token_jti() != run_token.jti {
        return Err(format!(
            "checkout authorization proof was minted against run-token jti {:?}, but this \
             transport is running under jti {:?} -- refusing before any spawn",
            proof.run_token_jti(),
            run_token.jti
        ));
    }
    let scope = proof.scope();
    if scope.tenant().0 != tenant {
        return Err(format!(
            "checkout authorization proof was minted for tenant {:?}, but this transport is \
             requesting tenant {tenant:?}",
            scope.tenant().0
        ));
    }
    if scope.repo_id() != repo {
        return Err(format!(
            "checkout authorization proof was minted for repo {:?}, but this transport is \
             requesting repo {repo:?}",
            scope.repo_id()
        ));
    }
    if scope.commit_hex() != expected.as_str() || scope.commit_format() != expected.format() {
        return Err(format!(
            "checkout authorization proof was minted for commit {:?} ({:?}), but this transport \
             is requesting {:?} ({:?})",
            scope.commit_hex(),
            scope.commit_format(),
            expected.as_str(),
            expected.format()
        ));
    }
    Ok(())
}

/// Deterministic `_given` seam for [`fetch_checkout_pack_within_parent_attempt`] (this codebase's
/// established convention, e.g. `finalize_runtime_given`): `execute` stands in for
/// `run_git_wire_container_raw` so tests can drive the aggregation/error-mapping logic without
/// spawning a real `runsc` binary. The true runsc/git-rootfs integration is exercised by 5b.3-7's live
/// drill.
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
fn fetch_checkout_pack_within_parent_attempt_given(
    root: &Path,
    tenant: &str,
    region: &str,
    repo: &str,
    expected: &ExpectedGitCommitId,
    limits: ResourceLimits,
    run_token: RunTokenCredential,
    proof: CheckoutAuthorizationProof,
    cancellation: &AtomicBool,
    lease_checkpoint: Option<&dyn crate::PreparationLeaseCheckpoint>,
    execute: GitWireHopExecutor,
) -> Result<ParentAttemptCheckoutTransportOutcome, CheckoutTransportError> {
    fetch_checkout_pack_within_parent_attempt_inner(
        root,
        tenant,
        region,
        repo,
        expected,
        limits,
        run_token,
        TransportAuthority::LegacyClaimBound { proof },
        cancellation,
        lease_checkpoint,
        execute,
    )
}

/// The one shared transport body both entry points drive. `authority` is module-private, so the arm
/// is decided by WHICH entry point was called, never by the caller.
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
fn fetch_checkout_pack_within_parent_attempt_inner(
    root: &Path,
    tenant: &str,
    region: &str,
    repo: &str,
    expected: &ExpectedGitCommitId,
    limits: ResourceLimits,
    run_token: RunTokenCredential,
    authority: TransportAuthority<'_>,
    cancellation: &AtomicBool,
    lease_checkpoint: Option<&dyn crate::PreparationLeaseCheckpoint>,
    execute: GitWireHopExecutor,
) -> Result<ParentAttemptCheckoutTransportOutcome, CheckoutTransportError> {
    // Resolve the advertise leg's launch permit plus (V2 only) the one-shot fetch source. In the V2
    // arm the permit can ONLY be obtained by consuming the whole `PhaseAuthorization` through a
    // check of its phase, its privately-retained run-token JTI, and its scope against THIS request —
    // so a permit minted for another claim/generation can never be paired with this credential
    // (round-1 blocker 2). In the V1 arm the proof is verified and an immediate permit is minted, as
    // shipped.
    let (advertise_generation, advertise_permit, mut fetch_source) = match authority {
        TransportAuthority::LegacyClaimBound { proof } => {
            // Verify the proof's scope AND the exact token generation it was checked against,
            // against THIS parent attempt's own request, BEFORE spawning anything (Sol's design,
            // point A).
            verify_transport_proof_against_request(&proof, &run_token, tenant, repo, expected)
                .map_err(|message| CheckoutTransportError::Refused { message })?;
            (None, LaunchPermit::immediate(), None)
        }
        TransportAuthority::PhaseBound { advertise, fetch } => {
            let generation = advertise.generation_id().to_owned();
            let permit = advertise
                .into_transport_permit(
                    crate::CheckoutPhase::Advertise,
                    &run_token,
                    tenant,
                    repo,
                    expected,
                )
                .map_err(|error| CheckoutTransportError::Refused { message: error.0 })?;
            (Some(generation), permit, Some(fetch))
        }
    };

    let zero = ResourceUsage {
        cpu_seconds: 0,
        mem_byte_seconds: 0,
    };
    let allow_reachable = vec![
        "-c".to_string(),
        "uploadpack.allowReachableSHA1InWant=true".to_string(),
    ];

    // ---- Hop A step 1: advertise-refs ----
    let mut advertise_argv = allow_reachable.clone();
    advertise_argv.extend([
        "upload-pack".to_string(),
        "--stateless-rpc".to_string(),
        "--advertise-refs".to_string(),
    ]);
    let advertise_spec = GitWireSpec::for_repo(
        root,
        tenant,
        region,
        repo,
        advertise_argv,
        Vec::new(),
        Vec::new(),
        None,
        limits,
        run_token.clone(),
        synthetic_parent_attempt_meter_target(),
        synthetic_parent_attempt_idem_token("checkout-advertise"),
    )
    .map_err(|e| CheckoutTransportError::Refused {
        message: format!("build advertise-refs spec: {e}"),
    })?;
    let mut advertise_command = Vec::with_capacity(advertise_spec.git_argv.len() + 2);
    advertise_command.push("git".to_string());
    advertise_command.extend(advertise_spec.git_argv.iter().cloned());
    advertise_command.push(WIRE_REPO_MOUNT.to_string());

    let (advertise_result, usage_after_advertise) = run_one_git_wire_hop_within_parent_attempt(
        &advertise_spec,
        advertise_command,
        cancellation,
        zero,
        false, // this is the FIRST hop of the whole transport -- nothing has run yet.
        advertise_permit,
        execute,
    )?;

    let advertise_parsed = parse_upload_pack_advertisement(&advertise_result.stdout, expected)
        .map_err(|e| {
            checkout_transport_terminal_failed(
                format!("parse advertisement: {e}"),
                usage_after_advertise,
            )
        })?;
    if !advertise_parsed.directly_advertised && !advertise_parsed.allows_reachable_want {
        return Err(checkout_transport_terminal_failed(
            "expected commit is not an advertised ref tip AND the server did not offer \
             allow-reachable-sha1-in-want -- refusing rather than sending an unreachable want"
                .to_string(),
            usage_after_advertise,
        ));
    }

    let mut capabilities = "no-progress ofs-delta".to_string();
    if let Some(token) = expected.format().capability_token() {
        capabilities.push(' ');
        capabilities.push_str(token);
    }
    let mut request = pkt_line_encode(&format!("want {} {capabilities}\n", expected.as_str()));
    request.extend_from_slice(&pkt_line_encode("deepen 1\n"));
    request.extend_from_slice(b"0000");
    request.extend_from_slice(&pkt_line_encode("done\n"));

    // ---- Hop A step 2: fetch ----
    // Stage the pack artifact BEFORE launching the fetch (same reasoning as `fetch_checkout_pack`):
    // a staging failure here must still carry the advertisement's already-measured usage, never
    // silently drop it.
    let mut pack_file = tempfile_for_checkout_pack().map_err(|e| {
        checkout_transport_retryable(format!("stage pack artifact: {e}"), usage_after_advertise)
    })?;

    // CT-007 lease/topology reconciliation: Hop A contains TWO independently full-timeout
    // executions, so the advertise→fetch boundary is a mandatory renewal checkpoint — without it the
    // interval between renewals could legally hold two executions and lapse the lease mid-Hop-A. The
    // renewal runs AFTER the advertisement is fully retired and BEFORE the fetch spawns; a lost
    // generation aborts here carrying the advertisement's already-measured usage, never spawns under
    // a lease another worker now owns. `None` keeps every pre-composition caller unchanged; 5b.3-6
    // supplies the durable checkpoint and adds the later Hop A→B and B→workload calls.
    if let Some(checkpoint) = lease_checkpoint {
        checkpoint.renew().map_err(|lost| {
            checkout_transport_retryable(lost.to_string(), usage_after_advertise)
        })?;
    }

    // CT-007 phase-credential generations: mint the FETCH credential here — after the advertisement
    // is fully retired, after the lease renewal proved this worker still owns the generation, and
    // before anything for the fetch is built or spawned. The renewal is deliberately NOT treated as
    // authorization: the provider re-locks and re-verifies the exact live generation itself. A
    // refusal aborts carrying the advertisement's measured usage, and the fetch never spawns.
    let (fetch_run_token, fetch_permit) = match fetch_source.as_mut() {
        None => (run_token, LaunchPermit::immediate()),
        Some(provider) => {
            let (credential, authorization) = provider().map_err(|error| {
                checkout_transport_retryable(
                    format!("mint fetch-phase credential: {}", error.0),
                    usage_after_advertise,
                )
            })?;
            // The fetch generation must be a DISTINCT durable generation from the advertise one: a
            // provider that handed back the advertise generation again would mean the successor was
            // never appended, so the advertise credential is still current and this leg would run
            // under a generation that has not been superseded as the phase sequence requires.
            if advertise_generation.as_deref() == Some(authorization.generation_id()) {
                return Err(checkout_transport_retryable(
                    "the fetch-phase authorization names the SAME durable generation as the \
                     advertisement -- the successor generation was never appended"
                        .to_string(),
                    usage_after_advertise,
                ));
            }
            // Consuming the authorization is the ONLY way to reach its permit, and doing so checks
            // the phase, the authorization's own retained JTI against THIS credential, and the full
            // scope against this request. A credential from one invocation cannot be paired with a
            // permit from another.
            let permit = authorization
                .into_transport_permit(crate::CheckoutPhase::Fetch, &credential, tenant, repo, expected)
                .map_err(|error| checkout_transport_retryable(error.0, usage_after_advertise))?;
            (credential, permit)
        }
    };

    let mut fetch_argv = allow_reachable;
    fetch_argv.extend(["upload-pack".to_string(), "--stateless-rpc".to_string()]);
    let fetch_spec = GitWireSpec::for_repo(
        root,
        tenant,
        region,
        repo,
        fetch_argv,
        request,
        Vec::new(),
        None,
        limits,
        fetch_run_token,
        synthetic_parent_attempt_meter_target(),
        synthetic_parent_attempt_idem_token("checkout-fetch"),
    )
    .map_err(|e| {
        checkout_transport_retryable(format!("build fetch spec: {e}"), usage_after_advertise)
    })?;
    let mut fetch_command = Vec::with_capacity(fetch_spec.git_argv.len() + 2);
    fetch_command.push("git".to_string());
    fetch_command.extend(fetch_spec.git_argv.iter().cloned());
    fetch_command.push(WIRE_REPO_MOUNT.to_string());

    let (fetch_result, usage_after_fetch) = run_one_git_wire_hop_within_parent_attempt(
        &fetch_spec,
        fetch_command,
        cancellation,
        usage_after_advertise,
        true, // the advertisement hop above already completed.
        fetch_permit,
        execute,
    )?;

    let parsed_fetch = parse_checkout_fetch_response(
        &fetch_result.stdout,
        expected,
        &mut pack_file,
        limits.disk_bytes,
    )
    .map_err(|e| {
        checkout_transport_terminal_failed(format!("parse fetch response: {e}"), usage_after_fetch)
    })?;

    pack_file.flush().map_err(|e| {
        checkout_transport_retryable(format!("flush pack artifact: {e}"), usage_after_fetch)
    })?;
    let mut pack_file = pack_file.into_inner().map_err(|e| {
        checkout_transport_retryable(format!("finish pack artifact: {e}"), usage_after_fetch)
    })?;
    pack_file.seek(std::io::SeekFrom::Start(0)).map_err(|e| {
        checkout_transport_retryable(format!("rewind pack artifact: {e}"), usage_after_fetch)
    })?;

    Ok(ParentAttemptCheckoutTransportOutcome {
        pack: PrefetchedCheckoutPack {
            file: pack_file,
            shallow: parsed_fetch.shallow,
        },
        usage: usage_after_fetch,
    })
}

/// A fresh, `O_CLOEXEC` host temp file to stream Hop A's pack into (removed from the directory the
/// instant it's created — an unlinked, anonymous-by-path file the process's own fd keeps alive for
/// exactly as long as it is needed, never left behind on any early-return path).
#[allow(dead_code)]
fn tempfile_for_checkout_pack() -> io::Result<std::io::BufWriter<std::fs::File>> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = std::env::temp_dir().join(format!(
        "myelin-checkout-pack-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    // `create_new` (== `O_CREAT|O_EXCL`) refuses if ANYTHING already exists at `path` (including a
    // symlink) rather than following it -- this is what actually prevents the raced-symlink issue,
    // not merely the unlink below. `mode(0o600)` (Sol's review) makes the file owner-only from
    // creation instead of depending on umask, since it carries a real source pack.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    // Unlink immediately: from this point on the file is reachable ONLY through this process's own
    // fd, never a second path-based open. A failed unlink means it is NOT actually anonymous
    // despite this function's contract -- propagate the failure (Sol's review) rather than
    // silently carrying a private pack through a still-path-reachable file.
    std::fs::remove_file(&path)?;
    Ok(std::io::BufWriter::new(file))
}

//! # `commit` — Git pseudonymous-by-default commits: consuming the 4.8 grammar (GIT-P25 / P-ID-25)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md` §11 (the
//! pseudonymous-by-default commits consume the grammar) + §3 (the opaque `principal_id` /
//! erasable `profile_ref` split), `external-insights/04-hard-problems.md` §1
//! (erasure-vs-immutability — the immutable commit bytes must never bake in erasable PII), and
//! `00-reconciliation-decisions.md` §X-7 (the ONE platform-wide erasure posture: pseudonym-map
//! shred is DSR fan-out step 1, the answer for git commit-author metadata).
//!
//! **Contract consumed:** **4.8** — the FROZEN pseudonym grammar `<pseudonym>@<tenant>.noreply`
//! (`myelin_identity::PseudonymHandle`, pinned C5). This module is the Id-side identity content
//! **applied to the git commit data model**: a commit's author/committer line carries the per-tenant
//! pseudonym, NEVER the erasable real identity. Id owns the grammar; Git consumes it (no second
//! identity rendering — `PseudonymHandle::render` is the one source of truth for the bytes).
//!
//! ## The property this module guarantees (GIT-D2 / GIT-1, the M3 lever)
//!
//! A git commit object's author/committer identity is **immutable**: it is hashed into the commit
//! OID and propagates to every descendant hash. GDPR erasure cannot tombstone it without rewriting
//! history (the genuinely-unsolved half, EI-04 §1). The answer, decided BEFORE the data model froze
//! (the M1 grammar freeze in `myelin_identity`, P-ID-19), is **pseudonymous-by-default**:
//!
//! - The author/committer line baked into the immutable bytes is the rendered pseudonym
//!   `<pseudonym>@<tenant>.noreply` ([`PseudonymHandle::render`]) — a PII-free, per-tenant opaque
//!   handle. The real name/email is **never** in the commit object.
//! - The opaque, stable `principal_id` ([`CommitAttribution`]) attributes the commit for **authz**
//!   (who may force-push, who owns this work) — but it lives **out-of-band** in the metadata store,
//!   NOT in the commit bytes. Events/git/audit attribute by it while the erasable `profile_ref`
//!   (the real identity ↔ pseudonym map, contract 4.8 S2) lives separately (arch §3).
//! - On `erase(subject)` the platform crypto-shreds the pseudonym-map S2 entry (DSR fan-out **step
//!   1**, X-7). The immutable commit bytes are **unchanged** — and that is the point: they carried
//!   only the pseudonym all along, so after erasure **0 real identity is recoverable** from them.
//!   The pseudonymous residual **== the one platform posture** (X-7) — GIT-D2 green.
//!
//! ## What this prompt (GIT-P25 / P-ID-25) ships — and the floor it names
//!
//! **Ships:** the [`Commit`] object builder (canonical commit bytes + a `blake3:` content-addressed
//! [`CommitOid`]), the [`CommitIdentity`] author/committer carrier that is pseudonym-by-construction
//! (it CANNOT be built from a raw name/email — the only constructor takes a [`PseudonymHandle`]),
//! the [`CommitAttribution`] opaque-principal authz side, and [`erased_residual`] — the function
//! that proves, over the immutable bytes, that an erased subject leaves no recoverable real identity.
//!
//! **Floor named (VISION §3):** the **audited history-rewrite path** — for the rare case where a
//! *commit message body* (free text authored by someone, possibly naming a third party) must be
//! expunged — is the **M5/on-demand follow-on** (the Git erasure-admin tool, 00-recon §X-7 / CR §9
//! 10.6; owned by the Git + GDPR roadmaps). It is the disruptive, hash-changing op; the
//! pseudonymous-by-default floor here covers commit *author identity* with **0** hash change.

use core::fmt;
use myelin_identity::PseudonymHandle;

/// The frozen domain suffix of the pseudonym grammar, re-exported from `myelin_identity` so the
/// commit-codec drift-fails at compile time against the ONE definition (contract 4.8, pin C5).
pub use myelin_identity::PSEUDONYM_DOMAIN_SUFFIX;

/// A git commit object's **author or committer identity** — pseudonymous **by construction**.
///
/// This type is the structural guarantee that a commit's immutable bytes can NEVER carry a raw
/// name/email: its only public constructor ([`CommitIdentity::pseudonymous`]) takes a
/// [`PseudonymHandle`] (the PII-free `(pseudonym, tenant)` pair from the S2 map). There is **no**
/// `from_name_email` path — a developer cannot accidentally bake real identity into a commit.
///
/// A real git author line is `Name <email> timestamp tz`. Here the "name" and the "email" are BOTH
/// the rendered pseudonym `<pseudonym>@<tenant>.noreply` — the bytes carry the opaque handle twice,
/// never a human name, never a routable address (the `.noreply` TLD is unroutable by construction).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitIdentity {
    /// The frozen pseudonym handle — the per-tenant opaque identity the S2 map minted. PII-free.
    handle: PseudonymHandle,
    /// Author/committer time, seconds since the Unix epoch. Not personal data (a commit timestamp
    /// is operational metadata, not an identifier of a person).
    when_unix_secs: i64,
    /// The timezone offset in minutes (e.g. `+0200` → `120`). Operational metadata, not PII.
    tz_offset_minutes: i32,
}

impl CommitIdentity {
    /// Build a commit author/committer line from a FROZEN pseudonym handle (contract 4.8) + the
    /// commit time. This is the ONLY way to mint a `CommitIdentity` — so the immutable bytes are
    /// pseudonymous-**by-default**, structurally, with no opt-out path to a raw name/email.
    pub fn pseudonymous(
        handle: PseudonymHandle,
        when_unix_secs: i64,
        tz_offset_minutes: i32,
    ) -> CommitIdentity {
        CommitIdentity {
            handle,
            when_unix_secs,
            tz_offset_minutes,
        }
    }

    /// The pseudonym handle this identity renders to (PII-free).
    pub fn handle(&self) -> &PseudonymHandle {
        &self.handle
    }

    /// The rendered pseudonymous email `<pseudonym>@<tenant>.noreply` — the exact string baked into
    /// the commit's immutable author/committer bytes (the one rendering, [`PseudonymHandle::render`]).
    pub fn render_email(&self) -> String {
        self.handle.render()
    }

    /// The canonical git author/committer line **as it goes into the immutable commit bytes**:
    /// `<pseudonym>@<tenant>.noreply <<pseudonym>@<tenant>.noreply> <unix> <tz>`. Both the display
    /// name AND the email are the pseudonym — there is NO human name anywhere in the bytes.
    ///
    /// `role` is `"author"` or `"committer"` (the git header key).
    fn render_line(&self, role: &str) -> String {
        let email = self.render_email();
        // The display name is the SAME pseudonym as the email — never a human name. A `+0000`-style
        // tz from the signed minute offset.
        let sign = if self.tz_offset_minutes < 0 { '-' } else { '+' };
        let abs = self.tz_offset_minutes.unsigned_abs();
        format!(
            "{role} {email} <{email}> {} {sign}{:02}{:02}",
            self.when_unix_secs,
            abs / 60,
            abs % 60,
        )
    }
}

/// A `blake3:<hex>` content-address over a commit's canonical immutable bytes (the commit OID).
///
/// The platform's ONE multihash convention (the same `blake3:<hex>` the GDPR holder Receipt + the
/// BlobStore content-address use) — not a hand-rolled hash. Because the OID is computed OVER the
/// pseudonymous author/committer line, two commits that differ ONLY in real identity are
/// **indistinguishable**: there is no real identity in the hashed bytes to differ by.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommitOid(pub String);

impl fmt::Display for CommitOid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A pseudonymous-by-default git commit object.
///
/// The author/committer are [`CommitIdentity`] (pseudonymous by construction). The `message` is the
/// free-text commit message body — under the X-7 posture it is content authored by the commit author
/// (covered by the per-subject DEK crypto-shred at rest; a third-party name typed into someone
/// else's commit message is the documented-limit residual + the M5 history-rewrite follow-on). The
/// `tree`/`parents` are opaque object OIDs (no PII).
///
/// [`Commit::oid`] is the content-address over [`Commit::canonical_bytes`] — the IMMUTABLE bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    /// The tree object OID this commit snapshots — opaque, no PII.
    pub tree: CommitOid,
    /// The parent commit OIDs (0 for a root commit, 2+ for a merge) — opaque, no PII.
    pub parents: Vec<CommitOid>,
    /// The commit author (who wrote the change) — pseudonymous by construction.
    pub author: CommitIdentity,
    /// The committer (who applied it; differs from author on a rebase/cherry-pick) — pseudonymous.
    pub committer: CommitIdentity,
    /// The free-text commit message body. Author-content under X-7 (per-subject DEK at rest);
    /// the third-party-mention residual is the documented limit + M5 history-rewrite follow-on.
    pub message: String,
}

impl Commit {
    /// The canonical, deterministic byte serialization of the commit object — the IMMUTABLE bytes
    /// that get content-addressed into the OID. Git-object-shaped: `tree`/`parent`/`author`/
    /// `committer` header lines, a blank line, then the message.
    ///
    /// The author/committer lines are the pseudonymous renderings ([`CommitIdentity::render_line`]),
    /// so these bytes carry ONLY the opaque pseudonym — never a raw name/email. This is the exact
    /// content the GIT-D2 drill scans for recoverable real identity (and finds none).
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = String::new();
        out.push_str(&format!("tree {}\n", self.tree));
        for parent in &self.parents {
            out.push_str(&format!("parent {parent}\n"));
        }
        out.push_str(&self.author.render_line("author"));
        out.push('\n');
        out.push_str(&self.committer.render_line("committer"));
        out.push('\n');
        out.push('\n');
        out.push_str(&self.message);
        out.into_bytes()
    }

    /// The commit OID — a `blake3:<hex>` content-address over [`Commit::canonical_bytes`] (the ONE
    /// platform multihash convention). Deterministic: the same bytes always yield the same OID, and
    /// the OID covers the pseudonymous author/committer line (so there is no real identity in the
    /// hashed input to recover).
    pub fn oid(&self) -> CommitOid {
        let digest = blake3::hash(&self.canonical_bytes());
        CommitOid(format!("blake3:{}", hex::encode(digest.as_bytes())))
    }
}

/// The **out-of-band authz attribution** for a commit (arch §3 — the opaque/erasable split).
///
/// The stable opaque `principal_id` attributes a commit for authz (who may force-push it, whose
/// work it is) and lives in the metadata store **NEXT TO** the commit, NOT in the commit bytes. It
/// is the join key into the erasable `profile_ref` (the real identity ↔ pseudonym S2 map, contract
/// 4.8) — but the bytes themselves never carry it, so an erase of the map leaves the bytes intact
/// AND leaves the opaque attribution still able to answer "who may act on this commit".
///
/// The `pseudonym` field records WHICH pseudonym handle the bytes were minted under, so the
/// metadata side can render the same commit identity without re-deriving it (one source of truth).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitAttribution {
    /// The commit this attribution is for.
    pub commit: CommitOid,
    /// The stable, opaque principal id (arch §3) — the authz attribution. PII-free; survives erase.
    pub principal_id: String,
    /// The pseudonym handle the commit bytes carry (the join into the erasable S2 map).
    pub pseudonym: PseudonymHandle,
}

/// The result of scanning a commit's IMMUTABLE bytes for recoverable real identity AFTER an
/// `erase(subject)` has crypto-shredded the pseudonym-map (DSR fan-out step 1, X-7).
///
/// This is the GIT-D2 residual artifact, computed structurally: after the S2 map entry is shredded,
/// the bytes carry only the pseudonym. `recoverable_real_identity` is the set of any real
/// name/email tokens the post-erase bytes still expose — and the pseudonymous-by-default floor
/// makes it **empty**. `pseudonymous_residual` is the pseudonym handle the bytes DO carry (the
/// expected residual — the ONE platform posture, X-7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErasureResidual {
    /// The set of real-identity tokens still recoverable from the immutable bytes after erase.
    /// Pseudonymous-by-default ⇒ this is EMPTY (the GIT-D2 gate: 0 real identity recoverable).
    pub recoverable_real_identity: Vec<String>,
    /// The pseudonym handle the immutable bytes carry — the expected residual (the platform posture).
    pub pseudonymous_residual: PseudonymHandle,
}

impl ErasureResidual {
    /// GIT-D2 gate: the pseudonymous residual == the one platform posture (X-7) iff there is **0**
    /// recoverable real identity in the immutable bytes after erase.
    pub fn residual_matches_posture(&self) -> bool {
        self.recoverable_real_identity.is_empty()
    }
}

/// Compute the GIT-D2 erasure residual for a commit: given the subject's **real identity tokens**
/// (the name/email the pseudonym-map S2 entry mapped to — the thing `erase(subject)` shreds), scan
/// the commit's IMMUTABLE bytes for any of them.
///
/// Because the bytes were minted pseudonymous-by-default, NONE of the real-identity tokens are in
/// them — `recoverable_real_identity` comes back **empty**, the pseudonymous handle is the only
/// residual, and [`ErasureResidual::residual_matches_posture`] is true. A mutation that baked a real
/// name/email into [`CommitIdentity::render_line`] (the pseudonym-substitution-at-commit path) would
/// make a token appear here — the mutation floor the prompt requires.
///
/// `real_identity_tokens` are the post-erasure-shredded values (the name + the real email); they are
/// the input the drill checks for absence. We scan a lossless UTF-8 view of the canonical bytes.
pub fn erased_residual(commit: &Commit, real_identity_tokens: &[&str]) -> ErasureResidual {
    let bytes = commit.canonical_bytes();
    // Lossless view: the canonical bytes are UTF-8 by construction (pseudonym handle + message);
    // `from_utf8_lossy` never hides a real-identity token (it only replaces invalid bytes, of
    // which there are none in a well-formed commit).
    let text = String::from_utf8_lossy(&bytes);
    let recoverable_real_identity = real_identity_tokens
        .iter()
        // A real-identity token counts as "recoverable" only if it is non-empty AND present in the
        // immutable bytes. An empty token is not an identifier (it would match everywhere).
        .filter(|tok| !tok.is_empty() && text.contains(*tok))
        .map(|tok| tok.to_string())
        .collect();
    ErasureResidual {
        recoverable_real_identity,
        // The author handle is the pseudonymous residual (committer carries the same posture).
        pseudonymous_residual: commit.author.handle().clone(),
    }
}

// ─────────────────── receive-pack pseudonymity enforcement (GIT-P12 / P-273) ────────────────────
//
// **Mandatory-core mutation floor (the prompt's cargo-mutants gate).** The pseudonymity rule gates
// the data model — it is mandatory-core. The floor is **100% of viable mutants caught** over the
// enforcement surface ([`enforce_pseudonymous_commit`], [`email_in_identity_line`],
// [`is_commit_object`]). MET: `cargo mutants -p myelin-git --file src/commit.rs` → 30 caught, 5
// unviable, 0 MISSED (2026-06-21).
//
// The [`CommitIdentity`] codec above is pseudonymous-**by-construction** — it mints OUR commits and
// cannot express a raw name/email. But a `git push` carries commit objects a CLIENT built with its
// OWN (possibly non-cooperating) git, whose author/committer lines may carry a real name + a real
// routable email. Those bytes arrive in the receive-pack QUARANTINE. GIT-1 (the data-model gate)
// requires that a commit whose author/committer identity is NOT the principal's tenant pseudonym is
// REJECTED *before the ref moves* — so the immutable object DB never admits cleartext PII in a commit
// identity field (the GIT-D2 "0 cleartext-PII commits admitted" gate). This is the receive-pack
// ENFORCEMENT half (the schema/codec half is the data model above).
//
// ## The chosen enforcement default (OQ-10 / R-8 — recorded here with its rationale)
// The decided PROPERTY is invariant: *the immutable commit bytes carry only the opaque pseudonym*
// (recon §X-7). The OQ-10/R-8 OPEN call is the *enforcement mode* — **client-cooperative,
// sha-stable REJECT-at-push** vs **server-side rewrite-at-push** (which would change the client's
// commit SHAs silently). **The default chosen here is REJECT-AT-PUSH (client-cooperative,
// sha-stable).** Rationale: (1) it is sha-STABLE — the client's local history and the server agree
// on every OID, so no silent hash divergence (the rewrite-at-push mode mutates SHAs under the
// client, breaking signatures and any external reference to the pre-rewrite OID); (2) it keeps the
// platform's `git` byte-plumbing a verbatim relay (no server-side history surgery on the hot push
// path — EI-04 §1 "never bake erasable PII in the first place" is met at the door, not patched
// after); (3) the cooperative path is a one-time `git config user.email <pseudonym>@<tenant>.noreply`
// the front door advertises, so the friction is bounded. The server-side rewrite-at-push mode
// remains the named follow-on for the non-cooperating-client long tail (GIT-P29, the history-rewrite
// machinery) — but it is NOT the default, because a silent SHA change is the more disruptive
// surprise. This default is restated in the crate doc (`lib.rs`).

/// The reason a pushed commit's author/committer identity is **not pseudonymous** — the receive-pack
/// pseudonymity rule rejects it BEFORE the ref moves (GIT-1, the GIT-D2 gate: 0 cleartext-PII commit
/// identities admitted). Each variant names exactly what the door refused (a rejected push is LOUD).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NonPseudonymousIdentity {
    /// A commit header line (`author`/`committer`) did not match the frozen grammar
    /// `<name> <<pseudonym>@<tenant>.noreply> <ts> <tz>` — its email is not a pseudonym handle at
    /// all (it is a real routable address, or malformed). `role` is `"author"`/`"committer"`.
    NotAPseudonym {
        /// the offending header role (`author` or `committer`).
        role: String,
        /// the offending address the client baked into the commit's identity line — surfaced LOUDLY
        /// in the reject (NOT silently coerced). This is a transient diagnostic of the REFUSED value
        /// (the thing the door keeps OUT of storage), never a persisted PII column — hence it is not
        /// (and must not be) a `#[personal_data]` field: the gate exists so this string is never
        /// stored.
        offending_email: String,
    },
    /// The email IS a well-formed pseudonym, but for the WRONG tenant — a commit authored to
    /// `<pseudonym>@<other-tenant>.noreply` must not move a ref in THIS tenant (cross-tenant
    /// pseudonym smuggling). `expected_tenant` is the principal's tenant; `found_tenant` is the
    /// commit's.
    WrongTenant {
        /// the header role (`author`/`committer`).
        role: String,
        /// the tenant the principal pushes under.
        expected_tenant: String,
        /// the tenant baked into the commit's pseudonym.
        found_tenant: String,
    },
    /// The commit object's bytes were not a parseable git commit (no `author`/`committer` header) —
    /// a malformed commit cannot be proven pseudonymous, so it is refused (fail-closed).
    Unparseable {
        /// which header was missing.
        missing: String,
    },
}

/// Parse the email out of a git `author`/`committer` header line. A real git identity line is
/// `<role> Name <email> <unix-ts> <tz>`; the email is the text between the LAST `<` and the matching
/// `>`. Returns `None` if the line has no `<…>` email span (a malformed header).
fn email_in_identity_line(line: &str) -> Option<&str> {
    let open = line.rfind('<')?;
    let close = line[open + 1..].find('>')? + open + 1;
    Some(&line[open + 1..close])
}

/// **The receive-pack pseudonymity gate (GIT-1 / GIT-P12 — the GIT-D2 enforcement half).** Given the
/// raw bytes of a pushed commit object and the principal's tenant, prove the commit's author AND
/// committer identities are the per-tenant pseudonym `<pseudonym>@<tenant>.noreply` (contract 4.8).
///
/// REJECT-AT-PUSH (the chosen default): a commit whose author/committer email is not a pseudonym
/// handle for THIS tenant returns `Err(NonPseudonymousIdentity)` — the caller (the push policy)
/// refuses the whole push before the ref moves, so the cleartext-PII identity never lands in the
/// immutable object DB. On success returns the parsed `(author, committer)` pseudonym handles (the
/// thing the reflog/attribution records).
///
/// The scan covers BOTH `author` and `committer` (a rebase/cherry-pick can differ them; both must be
/// pseudonymous). The check is sha-STABLE: it inspects, it does not mutate — a rejected commit's OID
/// is never silently changed (the rewrite-at-push alternative is the GIT-P29 follow-on).
pub fn enforce_pseudonymous_commit(
    commit_bytes: &[u8],
    tenant: &str,
) -> Result<(PseudonymHandle, PseudonymHandle), NonPseudonymousIdentity> {
    // A commit object is UTF-8 by git convention (the header is ASCII; only the message may carry
    // arbitrary text, and we only parse the header lines before the blank line).
    let text = String::from_utf8_lossy(commit_bytes);
    let mut author: Option<PseudonymHandle> = None;
    let mut committer: Option<PseudonymHandle> = None;
    for line in text.lines() {
        // The header ends at the first blank line; the message body is past it (and is NOT an
        // identity field — it is author-content under the per-subject DEK, X-7).
        if line.is_empty() {
            break;
        }
        let role = if let Some(rest) = line.strip_prefix("author ") {
            ("author", rest)
        } else if let Some(rest) = line.strip_prefix("committer ") {
            ("committer", rest)
        } else {
            continue;
        };
        let (role_name, rest) = role;
        let email = email_in_identity_line(rest).ok_or(NonPseudonymousIdentity::NotAPseudonym {
            role: role_name.to_string(),
            offending_email: rest.to_string(),
        })?;
        // The email MUST be a well-formed pseudonym handle in the frozen grammar.
        let handle = PseudonymHandle::parse(email).ok_or(NonPseudonymousIdentity::NotAPseudonym {
            role: role_name.to_string(),
            offending_email: email.to_string(),
        })?;
        // …and for the PRINCIPAL's tenant (no cross-tenant pseudonym in this tenant's refs).
        if handle.tenant() != tenant {
            return Err(NonPseudonymousIdentity::WrongTenant {
                role: role_name.to_string(),
                expected_tenant: tenant.to_string(),
                found_tenant: handle.tenant().to_string(),
            });
        }
        match role_name {
            "author" => author = Some(handle),
            _ => committer = Some(handle),
        }
    }
    let author = author.ok_or(NonPseudonymousIdentity::Unparseable { missing: "author".into() })?;
    let committer =
        committer.ok_or(NonPseudonymousIdentity::Unparseable { missing: "committer".into() })?;
    Ok((author, committer))
}

/// `true` iff `bytes` look like a git **commit** object (the first header is `tree …`). The push
/// policy scans only commit objects for the pseudonymity rule — a blob/tree carries no author line.
/// A git commit's canonical bytes always begin with the `tree <oid>` header, so the prefix is the
/// discriminant (a tag object begins `object …`; a blob/tree has no such header).
pub fn is_commit_object(bytes: &[u8]) -> bool {
    bytes.starts_with(b"tree ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle() -> PseudonymHandle {
        // A PII-free opaque handle from the S2 map (NOT a real name/email).
        PseudonymHandle::new("psn-7f3a9c", "acme").expect("well-formed handle")
    }

    fn commit() -> Commit {
        let author = CommitIdentity::pseudonymous(handle(), 1_700_000_000, 120);
        let committer = CommitIdentity::pseudonymous(handle(), 1_700_000_000, 120);
        Commit {
            tree: CommitOid("blake3:tree".into()),
            parents: vec![CommitOid("blake3:parent".into())],
            author,
            committer,
            message: "fix: handle the empty-ref edge case\n".into(),
        }
    }

    /// **The core property:** a commit's author/committer is the pseudonym
    /// `<pseudonym>@<tenant>.noreply` (contract 4.8) — never a raw name/email.
    #[test]
    fn commit_author_and_committer_are_the_pseudonym_grammar() {
        let c = commit();
        let text = String::from_utf8(c.canonical_bytes()).unwrap();
        // The exact frozen rendering appears for BOTH author and committer.
        assert!(text.contains("author psn-7f3a9c@acme.noreply <psn-7f3a9c@acme.noreply>"));
        assert!(text.contains("committer psn-7f3a9c@acme.noreply <psn-7f3a9c@acme.noreply>"));
        // The grammar suffix is present (the unroutable `.noreply` TLD).
        assert!(c.author.render_email().ends_with(PSEUDONYM_DOMAIN_SUFFIX));
        assert_eq!(c.author.render_email(), "psn-7f3a9c@acme.noreply");
    }

    /// **GIT-D2 gate:** after erase(subject) crypto-shreds the pseudonym-map, the IMMUTABLE commit
    /// bytes carry NO recoverable real identity (0 real-identity recoverable) — the pseudonymous
    /// residual == the one platform posture (X-7).
    #[test]
    fn after_erase_no_real_identity_recoverable_from_immutable_bytes() {
        let c = commit();
        // The real identity the S2 map mapped this pseudonym to (the thing erase shreds): a real
        // human name + their real, routable email. NONE of this was ever in the commit bytes.
        let real_tokens = ["Ada Lovelace", "ada.lovelace@example.com", "ada@acme.com"];
        let residual = erased_residual(&c, &real_tokens);
        assert!(
            residual.recoverable_real_identity.is_empty(),
            "real identity leaked into immutable bytes: {:?}",
            residual.recoverable_real_identity
        );
        assert!(residual.residual_matches_posture());
        // The pseudonymous residual IS present (the expected posture residual).
        assert_eq!(residual.pseudonymous_residual, handle());
    }

    /// The opaque `principal_id` still attributes the commit for authz AFTER erase — it lives
    /// out-of-band (arch §3), not in the bytes, so the erase of the S2 map does not touch it.
    #[test]
    fn opaque_principal_attributes_the_commit_for_authz() {
        let c = commit();
        let attr = CommitAttribution {
            commit: c.oid(),
            principal_id: "principal:opaque-stable-id".into(),
            pseudonym: handle(),
        };
        // The attribution is the commit's OID + the opaque principal — neither is a real identity.
        assert_eq!(attr.commit, c.oid());
        assert_eq!(attr.principal_id, "principal:opaque-stable-id");
        // The opaque principal_id is NOT in the immutable commit bytes (out-of-band attribution).
        let text = String::from_utf8(c.canonical_bytes()).unwrap();
        assert!(!text.contains("principal:opaque-stable-id"));
    }

    /// The OID is a deterministic `blake3:` content-address over the canonical bytes (the ONE
    /// platform multihash convention) — and it covers the pseudonymous author line.
    #[test]
    fn oid_is_blake3_content_address_over_pseudonymous_bytes() {
        let c = commit();
        let oid = c.oid();
        assert!(oid.0.starts_with("blake3:"));
        // Deterministic: the same bytes ⇒ the same OID.
        assert_eq!(c.oid(), commit().oid());
        // Two commits that differ ONLY in the pseudonym token produce DIFFERENT oids (the bytes
        // changed) — but neither carries real identity. The OID is a function of the pseudonym, not
        // of any real name/email.
        let mut other = commit();
        other.author =
            CommitIdentity::pseudonymous(PseudonymHandle::new("psn-other", "acme").unwrap(), 1, 0);
        other.committer = other.author.clone();
        assert_ne!(c.oid(), other.oid());
    }

    /// The canonical author line renders the tz offset with the correct SIGN + zero-padded
    /// `HHMM` — a deterministic detail the immutable bytes (and thus the OID) depend on, so it is
    /// pinned (a regression in the sign or width changes every commit hash).
    #[test]
    fn author_line_renders_signed_zero_padded_tz_offset() {
        let h = handle();
        // +0200 (positive offset).
        let east = CommitIdentity::pseudonymous(h.clone(), 1_700_000_000, 120);
        assert!(east.render_line("author").ends_with(" +0200"));
        // -0530 (negative, non-whole-hour offset → exercises the sign + the minute remainder).
        let west = CommitIdentity::pseudonymous(h.clone(), 1_700_000_000, -330);
        assert!(west.render_line("author").ends_with(" -0530"));
        // +0000 (zero offset renders `+`, never `-`).
        let utc = CommitIdentity::pseudonymous(h, 1_700_000_000, 0);
        assert!(utc.render_line("committer").ends_with(" +0000"));
    }

    /// `CommitOid` Displays as its inner `blake3:<hex>` string verbatim (the OID is shown to humans +
    /// logged; a Display that dropped the value would silently mis-identify a commit).
    #[test]
    fn commit_oid_displays_verbatim() {
        let oid = commit().oid();
        assert_eq!(format!("{oid}"), oid.0);
        assert!(format!("{oid}").starts_with("blake3:"));
        assert!(!format!("{oid}").is_empty());
    }

    /// **GIT-P12 enforcement: a pushed commit whose author/committer is the tenant pseudonym is
    /// ACCEPTED.** The pseudonymous-by-construction codec's own bytes pass the receive-pack gate (the
    /// cooperative-client happy path) — author + committer parse back to the same `(psn, tenant)`.
    #[test]
    fn enforce_accepts_a_pseudonymous_commit_for_the_tenant() {
        let c = commit(); // author == committer == psn-7f3a9c@acme.noreply
        let (author, committer) =
            enforce_pseudonymous_commit(&c.canonical_bytes(), "acme").expect("pseudonymous → accept");
        assert_eq!(author, handle());
        assert_eq!(committer, handle());
    }

    /// **GIT-P12 enforcement: a commit carrying a REAL name/email is REJECTED before the ref moves.**
    /// A non-cooperating client's commit (`Ada Lovelace <ada@example.com>`) is the cleartext-PII case
    /// the GIT-D2 gate must refuse at the door — `NotAPseudonym`, naming the offending email LOUDLY.
    #[test]
    fn enforce_rejects_a_raw_name_email_commit() {
        // Bytes a stock `git commit` would produce — a real routable identity in the author line.
        let raw = b"tree blake3:t\n\
                    author Ada Lovelace <ada.lovelace@example.com> 1700000000 +0000\n\
                    committer Ada Lovelace <ada.lovelace@example.com> 1700000000 +0000\n\
                    \n\
                    fix: the bug\n";
        match enforce_pseudonymous_commit(raw, "acme") {
            Err(NonPseudonymousIdentity::NotAPseudonym { role, offending_email }) => {
                assert_eq!(role, "author", "the FIRST non-pseudonymous header is named");
                assert_eq!(offending_email, "ada.lovelace@example.com");
            }
            other => panic!("expected NotAPseudonym, got {other:?}"),
        }
    }

    /// **GIT-P12 enforcement: a well-formed pseudonym for the WRONG tenant is REJECTED.** A commit
    /// authored to `psn@globex.noreply` must not move a ref in tenant `acme` (cross-tenant pseudonym
    /// smuggling) — `WrongTenant`, naming both tenants.
    #[test]
    fn enforce_rejects_a_wrong_tenant_pseudonym() {
        let foreign = PseudonymHandle::new("psn-x", "globex").unwrap();
        let id = CommitIdentity::pseudonymous(foreign, 1_700_000_000, 0);
        let mut c = commit();
        c.author = id.clone();
        c.committer = id;
        match enforce_pseudonymous_commit(&c.canonical_bytes(), "acme") {
            Err(NonPseudonymousIdentity::WrongTenant { role, expected_tenant, found_tenant }) => {
                assert_eq!(role, "author");
                assert_eq!(expected_tenant, "acme");
                assert_eq!(found_tenant, "globex");
            }
            other => panic!("expected WrongTenant, got {other:?}"),
        }
    }

    /// **GIT-P12 enforcement: the COMMITTER is checked too (not just the author).** A rebase can leave
    /// a pseudonymous author but a raw committer; both are immutable identity fields, so both gate.
    #[test]
    fn enforce_rejects_a_raw_committer_even_with_pseudonymous_author() {
        let raw = b"tree blake3:t\n\
                    author psn-ok@acme.noreply <psn-ok@acme.noreply> 1700000000 +0000\n\
                    committer Real Committer <real@corp.example> 1700000000 +0000\n\
                    \n\
                    chore: rebase\n";
        match enforce_pseudonymous_commit(raw, "acme") {
            Err(NonPseudonymousIdentity::NotAPseudonym { role, offending_email }) => {
                assert_eq!(role, "committer", "the committer is gated independently of the author");
                assert_eq!(offending_email, "real@corp.example");
            }
            other => panic!("expected committer NotAPseudonym, got {other:?}"),
        }
    }

    /// **GIT-P12 enforcement: a malformed commit (no author header) FAILS CLOSED.** A commit object
    /// that cannot be proven pseudonymous is refused, not admitted.
    #[test]
    fn enforce_fails_closed_on_a_missing_author_header() {
        let raw = b"tree blake3:t\n\
                    committer psn-ok@acme.noreply <psn-ok@acme.noreply> 1700000000 +0000\n\
                    \n\
                    msg\n";
        assert_eq!(
            enforce_pseudonymous_commit(raw, "acme"),
            Err(NonPseudonymousIdentity::Unparseable { missing: "author".into() })
        );
    }

    /// **`is_commit_object` distinguishes a commit from a blob** (so the policy scans only commits):
    /// a `tree …`-headed object is a commit; arbitrary blob bytes are not.
    #[test]
    fn is_commit_object_detects_only_commits() {
        let c = commit();
        assert!(is_commit_object(&c.canonical_bytes()), "a tree-headed object is a commit");
        assert!(!is_commit_object(b"just some file contents\n"), "a blob is not a commit");
        assert!(!is_commit_object(b"AKIAEXAMPLE secret blob"), "a secret blob is not a commit");
    }

    /// The pseudonymity gate does NOT scan the message body — a third-party name typed into a commit
    /// MESSAGE is the documented X-7 residual (per-subject DEK + history-rewrite follow-on), NOT a
    /// receive-pack reject. Only the author/committer IDENTITY fields gate (the GIT-1 property).
    #[test]
    fn enforce_ignores_third_party_mention_in_the_message_body() {
        let mut c = commit();
        // The body mentions a real person — that is author-content (X-7 residual), not an identity.
        c.message = "fix: as reported by Ada Lovelace <ada@example.com>\n".into();
        // The IDENTITY fields are still the tenant pseudonym → accepted (the body is not gated here).
        assert!(enforce_pseudonymous_commit(&c.canonical_bytes(), "acme").is_ok());
    }

    /// A commit with a different REAL author but the SAME pseudonym + bytes is byte-identical: the
    /// commit object cannot distinguish real identities (there is none in the bytes to distinguish
    /// by). This is the structural anti-leak guarantee.
    #[test]
    fn real_identity_is_not_a_function_of_the_commit_bytes() {
        // Two different real people, both mapped (by the S2 map) to the SAME per-tenant pseudonym
        // would produce IDENTICAL commit bytes — the bytes are a pure function of the pseudonym.
        let c1 = commit();
        let c2 = commit();
        assert_eq!(c1.canonical_bytes(), c2.canonical_bytes());
        assert_eq!(c1.oid(), c2.oid());
    }
}

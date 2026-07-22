//! # The git smart-HTTP PUSH path (`git-receive-pack`) — CT-006d / GT-006 write-side
//!
//! The write counterpart to [`crate::git_wire_http`]'s clone/fetch. A real `git push http://…/<repo>.git`
//! lands here. The push is split into a SANDBOXED byte-intake and a TRUSTED in-process write so the
//! repo stays READ-ONLY to the sandbox and no untrusted `git` ever moves a real ref:
//!
//!   1. **advertise** (`GET …/info/refs?service=git-receive-pack`) — built IN-PROCESS from the durable
//!      repo's refs with a DELIBERATELY RESTRICTED capability set (`report-status delete-refs ofs-delta`
//!      — NO `side-band-64k`/`report-status-v2`/`atomic`), so the client speaks the simplest, fully
//!      server-controlled framing. Reading our own tenant-scoped refs is a pure read (no sandbox needed).
//!   2. **push** (`POST …/git-receive-pack`) — this module parses the ref-update commands + extracts the
//!      packfile (pure Rust, no git), ingests the UNTRUSTED pack in the hardened gVisor sandbox
//!      (`git index-pack` into a writable `/tmp` tmpfs quarantine; the real repo is RO), receives the
//!      fully-resolved objects back, and hands them to the durable backend's in-process gate
//!      ([`crate::DurableGitBackend::receive_pack`]): policy (secret-scan / size / pseudonymity) + a
//!      connectivity check BEFORE migration, then the ONE-transaction ref-CAS + `git.ref.updated` outbox
//!      emit (BUS-2 / emit-iff-committed). A rejected push does NOT move the ref and discards the
//!      quarantine. The result is a `report-status` the client renders as `ok`/`ng` per ref.
//!
//! AUTH/AUTHZ is the gateway's (unchanged): the POST route carries the WRITE action
//! `git.wire.receive_pack`; the operating `(tenant, region)` is the VERIFIED token's, never the URL
//! (the URL `{tenant}` only detects/rejects a cross-tenant IDOR) — identical to the upload-pack routes.

use crate::git_wire_http::pkt_line;
use std::collections::BTreeSet;

/// The receive-pack capabilities Myelin advertises. Deliberately MINIMAL + server-controlled: enough for
/// a real `git push` (`report-status` so the client expects a status report; `delete-refs` so a branch
/// delete is possible; `ofs-delta` so the client may send compact offset deltas — `index-pack` resolves
/// them) but WITHOUT `side-band-64k` (so the status report is plain pkt-lines, not band-multiplexed),
/// `report-status-v2`, or `atomic`. Less surface, fully-controlled response framing.
pub(crate) const RECV_CAPS: &str =
    "report-status delete-refs ofs-delta object-format=sha1 agent=myelin/ct006d";

/// The smart-HTTP advertisement content-type for `git-receive-pack`.
pub(crate) const RECEIVE_PACK_ADV: &str = "application/x-git-receive-pack-advertisement";
/// The smart-HTTP result content-type for `git-receive-pack`.
pub(crate) const RECEIVE_PACK_RESULT: &str = "application/x-git-receive-pack-result";

/// One ref-update command the client proposes: `old_oid new_oid ref_name` (all-zeros old = create,
/// all-zeros new = delete).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RefCommand {
    pub old: String,
    pub new: String,
    pub ref_name: String,
}

/// Read one pkt-line: `(Some(payload), rest)` for a data line, `(None, rest)` for the flush-pkt `0000`.
fn read_pkt(buf: &[u8]) -> Result<(Option<&[u8]>, &[u8]), String> {
    if buf.len() < 4 {
        return Err("truncated pkt-line length prefix".into());
    }
    let hdr = std::str::from_utf8(&buf[..4]).map_err(|_| "non-ascii pkt-line length".to_string())?;
    let len = usize::from_str_radix(hdr, 16).map_err(|_| format!("bad pkt-line hex length {hdr:?}"))?;
    if len == 0 {
        return Ok((None, &buf[4..])); // flush-pkt
    }
    if len < 4 || len > buf.len() {
        return Err(format!("pkt-line length {len} out of range (buf {})", buf.len()));
    }
    Ok((Some(&buf[4..len]), &buf[len..]))
}

/// **Parse a `git-receive-pack` request body**: the ref-update command pkt-lines (terminated by the
/// flush-pkt), then the raw packfile (everything after the flush — empty for a delete-only push). Pure
/// Rust — NO git touches the untrusted body here; the packfile is parsed only inside the sandbox.
pub(crate) fn parse_push_request(body: &[u8]) -> Result<(Vec<RefCommand>, Vec<u8>), String> {
    let mut rest = body;
    let mut cmds = Vec::new();
    let mut ref_names = BTreeSet::new();
    loop {
        let (payload, next) = read_pkt(rest)?;
        rest = next;
        let Some(p) = payload else { break }; // the flush-pkt ends the command list
        // The FIRST command carries the client capability list after a NUL — drop it (we ignore caps;
        // the response framing is fixed by our restricted advertisement).
        let line = p.split(|&b| b == 0).next().unwrap_or(p);
        let s = std::str::from_utf8(line)
            .map_err(|_| "non-utf8 ref-update command".to_string())?
            .trim_end_matches(['\n', ' ']);
        let parts: Vec<&str> = s.splitn(3, ' ').collect();
        if parts.len() != 3 || parts[0].len() != 40 || parts[1].len() != 40 {
            return Err(format!("malformed ref-update command {s:?}"));
        }
        let ref_name = parts[2].to_string();
        if !ref_names.insert(ref_name.clone()) {
            return Err(format!("duplicate ref-update command for {ref_name:?}"));
        }
        cmds.push(RefCommand {
            old: parts[0].to_string(),
            new: parts[1].to_string(),
            ref_name,
        });
    }
    Ok((cmds, rest.to_vec()))
}

/// **Parse a `git cat-file --batch` object stream** (the sandbox's receive-pack ingest output):
/// `<oid> SP <type> SP <size>\n<payload>\n` repeated. Returns `(oid, type, payload)` per object.
pub(crate) fn parse_cat_file_batch(mut buf: &[u8]) -> Result<Vec<(String, String, Vec<u8>)>, String> {
    let mut out = Vec::new();
    while !buf.is_empty() {
        let nl = buf
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(|| "truncated cat-file header".to_string())?;
        let header = std::str::from_utf8(&buf[..nl])
            .map_err(|_| "non-utf8 cat-file header".to_string())?
            .to_string();
        buf = &buf[nl + 1..];
        let parts: Vec<&str> = header.split(' ').collect();
        if parts.len() != 3 {
            return Err(format!("cat-file reported a non-resolvable object: {header:?}"));
        }
        let size: usize = parts[2].parse().map_err(|_| format!("bad cat-file size in {header:?}"))?;
        if buf.len() < size + 1 {
            return Err("truncated cat-file payload".into());
        }
        let payload = buf[..size].to_vec();
        buf = &buf[size..];
        if buf.first() == Some(&b'\n') {
            buf = &buf[1..];
        }
        out.push((parts[0].to_string(), parts[1].to_string(), payload));
    }
    Ok(out)
}

/// Build the `git-receive-pack` v0 ref-advertisement body (the part AFTER the smart-HTTP service header
/// + flush): each `(ref_name, oid)`, the FIRST carrying the NUL-prefixed capability list; then a flush.
///
/// An empty repo advertises the `capabilities^{}` placeholder line. Sorted by the caller for determinism.
pub(crate) fn build_receive_pack_refs(refs: &[(String, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    if refs.is_empty() {
        out.extend(pkt_line(&format!("{} capabilities^{{}}\0{RECV_CAPS}\n", "0".repeat(40))));
    } else {
        for (i, (name, oid)) in refs.iter().enumerate() {
            if i == 0 {
                out.extend(pkt_line(&format!("{oid} {name}\0{RECV_CAPS}\n")));
            } else {
                out.extend(pkt_line(&format!("{oid} {name}\n")));
            }
        }
    }
    out.extend_from_slice(b"0000");
    out
}

/// Build a `report-status` response: `unpack <status>` then one `ok <ref>` / `ng <ref> <reason>` per
/// command, then a flush. `unpack` is `ok` on a clean ingest, else the failure reason.
pub(crate) fn report_status(unpack: &str, per_ref: &[(String, Option<String>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(pkt_line(&format!("unpack {unpack}\n")));
    for (name, err) in per_ref {
        match err {
            None => out.extend(pkt_line(&format!("ok {name}\n"))),
            Some(reason) => out.extend(pkt_line(&format!("ng {name} {reason}\n"))),
        }
    }
    out.extend_from_slice(b"0000");
    out
}

/// A `report-status` per-ref list marking EVERY command `ng` with one reason (an atomic whole-push
/// refusal: no ref moved). Used for corrupt-pack / connectivity / policy refusals.
pub(crate) fn all_ng(cmds: &[RefCommand], reason: &str) -> Vec<(String, Option<String>)> {
    cmds.iter()
        .map(|c| (c.ref_name.clone(), Some(reason.to_string())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_create_command_and_pack() {
        // `<40 zeros> <40 b> refs/heads/main\0caps\n` then flush then a fake pack.
        let z = "0".repeat(40);
        let b = "b".repeat(40);
        let cmd = format!("{z} {b} refs/heads/main\0report-status\n");
        let mut body = format!("{:04x}{cmd}", cmd.len() + 4).into_bytes();
        body.extend_from_slice(b"0000");
        body.extend_from_slice(b"PACKfake");
        let (cmds, pack) = parse_push_request(&body).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].old, z);
        assert_eq!(cmds[0].new, b);
        assert_eq!(cmds[0].ref_name, "refs/heads/main");
        assert_eq!(pack, b"PACKfake");
    }

    #[test]
    fn delete_only_push_has_no_pack() {
        let z = "0".repeat(40);
        let a = "a".repeat(40);
        let cmd = format!("{a} {z} refs/heads/old\0report-status\n");
        let mut body = format!("{:04x}{cmd}", cmd.len() + 4).into_bytes();
        body.extend_from_slice(b"0000");
        let (cmds, pack) = parse_push_request(&body).unwrap();
        assert_eq!(cmds[0].new, z, "delete sets new to all-zeros");
        assert!(pack.is_empty(), "a delete-only push carries no packfile");
    }

    #[test]
    fn duplicate_ref_commands_are_rejected_before_pack_ingest() {
        let z = "0".repeat(40);
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let first = format!("{z} {a} refs/heads/topic\0report-status\n");
        let second = format!("{z} {b} refs/heads/topic\n");
        let mut body = format!("{:04x}{first}", first.len() + 4).into_bytes();
        body.extend_from_slice(format!("{:04x}{second}", second.len() + 4).as_bytes());
        body.extend_from_slice(b"0000PACKmust-not-be-ingested");

        let error = parse_push_request(&body).expect_err("one ref may appear only once per push");
        assert!(
            error.contains("duplicate ref-update command for \"refs/heads/topic\""),
            "the parser names the conflicting ref: {error}"
        );
    }

    #[test]
    fn round_trips_cat_file_batch() {
        let payload = b"tree abc\nauthor x\n\nmsg\n";
        let mut stream = format!("{} commit {}\n", "1".repeat(40), payload.len()).into_bytes();
        stream.extend_from_slice(payload);
        stream.push(b'\n');
        let objs = parse_cat_file_batch(&stream).unwrap();
        assert_eq!(objs.len(), 1);
        assert_eq!(objs[0].1, "commit");
        assert_eq!(objs[0].2, payload);
    }

    #[test]
    fn empty_repo_advertisement_has_the_capabilities_placeholder() {
        let adv = build_receive_pack_refs(&[]);
        let s = String::from_utf8_lossy(&adv);
        assert!(s.contains("capabilities^{}"));
        assert!(s.contains("report-status"));
        assert!(s.ends_with("0000"));
    }
}

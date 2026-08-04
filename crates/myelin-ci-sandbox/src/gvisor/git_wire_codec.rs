//! Git wire-protocol codec (CT-007 slice 5b.2): fail-closed pkt-line reader/encoder and the
//! two specialized `upload-pack` response parsers (v0 advertise-refs + the single-shot shallow
//! fetch) the checkout transport relies on. Pure byte-buffer logic with no runtime coupling.

use crate::workspace_intent::{ExpectedGitCommitId, GitObjectFormat};
use std::io::Write;

/// One raw pkt-line parsed from a git wire byte stream (CT-007 slice 5b.2's fetch/advertisement
/// decoders).
#[derive(Debug)]
enum PktLine<'a> {
    Flush,
    Data(&'a [u8]),
}

/// Read exactly one pkt-line at `buf[*pos..]`, advancing `*pos` past it. Fail-closed (Sol's round-2
/// review nailed down these exact bounds): a non-hex length header, a length in the reserved
/// `0001`-`0003` range, a length exceeding git's own protocol maximum (65,520, i.e. `0xfff0`), or a
/// header claiming more bytes than remain in `buf`, all refuse rather than guess.
fn read_pkt_line<'a>(buf: &'a [u8], pos: &mut usize) -> Result<PktLine<'a>, String> {
    let header = buf
        .get(*pos..*pos + 4)
        .ok_or_else(|| "truncated pkt-line length header".to_string())?;
    let header_str = std::str::from_utf8(header)
        .map_err(|_| "pkt-line length header is not ASCII hex".to_string())?;
    let len = u16::from_str_radix(header_str, 16)
        .map_err(|_| format!("pkt-line length header {header_str:?} is not valid hex"))?;
    if len == 0 {
        *pos += 4;
        return Ok(PktLine::Flush);
    }
    if len < 4 {
        return Err(format!(
            "pkt-line length {len} is reserved (0001-0003 are invalid)"
        ));
    }
    if len > 0xfff0 {
        return Err(format!(
            "pkt-line length {len} exceeds git's protocol maximum (65520)"
        ));
    }
    let total = len as usize;
    let payload = buf
        .get(*pos + 4..*pos + total)
        .ok_or_else(|| "pkt-line declares more bytes than the stream contains".to_string())?;
    *pos += total;
    Ok(PktLine::Data(payload))
}

/// Pkt-line-encode one payload (`0009done\n`): a 4-hex-digit length prefix counting itself, then the
/// payload bytes verbatim.
pub(super) fn pkt_line_encode(payload: &str) -> Vec<u8> {
    let mut v = format!("{:04x}", payload.len() + 4).into_bytes();
    v.extend_from_slice(payload.as_bytes());
    v
}

/// The result of parsing a v0 `upload-pack --advertise-refs` response (CT-007 slice 5b.2, Hop A step
/// 1): whether `expected` is directly advertised as some ref's target, and whether the server offers
/// `allow-reachable-sha1-in-want` (needed when it is NOT a direct tip — Sol's round-2 review: CI
/// dispatch commonly targets a commit that is reachable but no longer an exact advertised tip by the
/// time a queued attempt starts, and this codebase has no per-attempt ref to pin it with today).
#[derive(Debug)]
#[allow(dead_code)]
pub(super) struct ParsedAdvertisement {
    pub(super) directly_advertised: bool,
    pub(super) allows_reachable_want: bool,
}

/// Parse a v0 `upload-pack --advertise-refs` response. Sol's review hardened this beyond the
/// original draft: the first ref line's capability section is now MANDATORY (a real repo's
/// advertisement always has one; its absence is a malformed/spoofed response, not merely a
/// capability-less repo), every capability THIS TRANSPORT'S fetch request always relies on
/// (`shallow` — required by the `deepen 1` it always sends; `no-progress`; `ofs-delta`) must
/// actually be advertised or the whole advertisement is refused, and the response must end EXACTLY
/// at the terminating flush (trailing bytes refused, never silently ignored).
#[allow(dead_code)]
pub(super) fn parse_upload_pack_advertisement(
    response: &[u8],
    expected: &ExpectedGitCommitId,
) -> Result<ParsedAdvertisement, String> {
    let mut pos = 0usize;
    let mut directly_advertised = false;
    let mut allows_reachable_want = false;
    let mut has_shallow_cap = false;
    let mut has_no_progress_cap = false;
    let mut has_ofs_delta_cap = false;
    let mut first_line = true;
    loop {
        match read_pkt_line(response, &mut pos)? {
            PktLine::Flush => break,
            PktLine::Data(data) => {
                let (line, caps) =
                    if first_line {
                        match data.iter().position(|&b| b == 0) {
                            Some(nul) => (&data[..nul], &data[nul + 1..]),
                            None => return Err(
                                "advertisement's first ref line is missing a capability section \
                                 (malformed or spoofed response)"
                                    .to_string(),
                            ),
                        }
                    } else {
                        (data, &[][..])
                    };
                first_line = false;
                let line = std::str::from_utf8(line)
                    .map_err(|_| "ref-advertisement line is not UTF-8".to_string())?;
                let line = line.strip_suffix('\n').unwrap_or(line);
                if let Some((oid, _refname)) = line.split_once(' ') {
                    if oid == expected.as_str() {
                        directly_advertised = true;
                    }
                }
                if !caps.is_empty() {
                    let caps_str = std::str::from_utf8(caps)
                        .map_err(|_| "capability list is not UTF-8".to_string())?;
                    for cap in caps_str.split_whitespace() {
                        match cap {
                            "allow-reachable-sha1-in-want" => allows_reachable_want = true,
                            "shallow" => has_shallow_cap = true,
                            "no-progress" => has_no_progress_cap = true,
                            "ofs-delta" => has_ofs_delta_cap = true,
                            _ => {}
                        }
                    }
                    let advertised_format = caps_str
                        .split_whitespace()
                        .find_map(|c| c.strip_prefix("object-format="));
                    match (advertised_format, expected.format()) {
                        (None, GitObjectFormat::Sha1) | (Some("sha1"), GitObjectFormat::Sha1) => {}
                        (Some("sha256"), GitObjectFormat::Sha256) => {}
                        (advertised, wanted) => {
                            return Err(format!(
                                "server advertises object-format {advertised:?}, expected \
                                 {wanted:?}"
                            ));
                        }
                    }
                }
            }
        }
    }
    if pos != response.len() {
        return Err(format!(
            "trailing bytes after the advertisement's terminating flush ({} bytes)",
            response.len() - pos
        ));
    }
    if !has_shallow_cap {
        return Err(
            "server does not advertise `shallow` -- required for the `deepen 1` this transport \
             always sends"
                .to_string(),
        );
    }
    if !has_no_progress_cap {
        return Err("server does not advertise `no-progress`".to_string());
    }
    if !has_ofs_delta_cap {
        return Err("server does not advertise `ofs-delta`".to_string());
    }
    Ok(ParsedAdvertisement {
        directly_advertised,
        allows_reachable_want,
    })
}

/// What [`parse_checkout_fetch_response`] found about the shallow boundary.
#[derive(Debug)]
#[allow(dead_code)]
pub(super) struct ParsedCheckoutFetch {
    /// `true` iff the server reported `expected` itself as the shallow boundary (its parents were
    /// not sent) — the checkout script must then seed `.git/shallow` with it before checkout.
    pub(super) shallow: bool,
}

/// Parse a v0 stateless-rpc `upload-pack` FETCH response for CT-007's single-shot
/// `want <expected> ... / deepen 1 / (flush) / done` request, streaming the raw pack straight into
/// `pack_out` under `pack_cap` bytes. Fail-closed at every step — Sol's round-2 review nailed down
/// the EXACT accepted grammar for this one specialized request (never the general git-protocol
/// grammar, which allows far more than this request ever produces):
///
/// ```text
/// [PKT("shallow <expected>")]   -- at most one, only naming `expected`, no `unshallow`
/// 0000                          -- the shallow-info section's flush -- ALWAYS present (a `deepen`
///                                  request was sent), even for a root commit with zero lines
/// PKT("NAK")                    -- exactly one -- no ACK (no `have` lines were ever sent)
/// PACK...                       -- raw pack bytes, immediately, to EOF -- never scanned for
/// ```
///
/// Anything else (an `ERR` line, a duplicate/foreign/`unshallow` declaration, an ACK, a
/// missing/misplaced `PACK` signature, a response exceeding `pack_cap`) is refused.
#[allow(dead_code)]
pub(super) fn parse_checkout_fetch_response(
    response: &[u8],
    expected: &ExpectedGitCommitId,
    pack_out: &mut impl Write,
    pack_cap: u64,
) -> Result<ParsedCheckoutFetch, String> {
    let mut pos = 0usize;
    let mut shallow = false;
    // -- shallow-info: zero or more shallow/unshallow lines, then EXACTLY ONE mandatory flush --
    loop {
        match read_pkt_line(response, &mut pos)? {
            PktLine::Flush => break,
            PktLine::Data(data) => {
                let line = std::str::from_utf8(data)
                    .map_err(|_| "shallow-info line is not UTF-8".to_string())?;
                let line = line.strip_suffix('\n').unwrap_or(line);
                if let Some(oid) = line.strip_prefix("shallow ") {
                    if shallow {
                        return Err("duplicate `shallow` line in shallow-info section".to_string());
                    }
                    if oid != expected.as_str() {
                        return Err(format!(
                            "shallow boundary names {oid:?}, expected {:?}",
                            expected.as_str()
                        ));
                    }
                    shallow = true;
                } else if line.starts_with("unshallow ") {
                    return Err(
                        "unexpected `unshallow` line -- this is a fresh, non-shallow fetch"
                            .to_string(),
                    );
                } else if let Some(msg) = line.strip_prefix("ERR ") {
                    return Err(format!("upload-pack refused: {msg}"));
                } else {
                    return Err(format!("unexpected line in shallow-info section: {line:?}"));
                }
            }
        }
    }
    // -- negotiation: exactly one NAK line, no flush after it --
    match read_pkt_line(response, &mut pos)? {
        PktLine::Data(data) => {
            let line = std::str::from_utf8(data)
                .map_err(|_| "negotiation line is not UTF-8".to_string())?;
            let line = line.strip_suffix('\n').unwrap_or(line);
            if let Some(msg) = line.strip_prefix("ERR ") {
                return Err(format!("upload-pack refused: {msg}"));
            }
            if line != "NAK" {
                return Err(format!(
                    "expected a single NAK (no `have` lines were sent), got {line:?}"
                ));
            }
        }
        PktLine::Flush => return Err("unexpected flush where NAK was expected".to_string()),
    }
    // -- pack: everything remaining, starting with the literal 4-byte "PACK" magic --
    let pack = response
        .get(pos..)
        .ok_or_else(|| "response ended before any pack data".to_string())?;
    if pack.len() as u64 > pack_cap {
        return Err(format!(
            "pack response ({} bytes) exceeds the {pack_cap}-byte cap",
            pack.len()
        ));
    }
    if pack.len() < 4 || &pack[..4] != b"PACK" {
        return Err(format!(
            "expected the raw pack to begin immediately with 'PACK' at byte {pos}, found {:?}",
            pack.get(..4.min(pack.len()))
        ));
    }
    pack_out
        .write_all(pack)
        .map_err(|e| format!("write pack to bounded output: {e}"))?;
    Ok(ParsedCheckoutFetch { shallow })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gvisor::checkout_transport_test_support::{
        advertisement, fake_pack, fetch_response, sha1_oid,
    };

    // ---- pkt-line reader ----

    #[test]
    fn read_pkt_line_reads_flush() {
        let mut pos = 0;
        assert!(matches!(
            read_pkt_line(b"0000rest", &mut pos).unwrap(),
            PktLine::Flush
        ));
        assert_eq!(pos, 4);
    }

    #[test]
    fn read_pkt_line_reads_data() {
        let encoded = pkt_line_encode("NAK");
        let mut pos = 0;
        match read_pkt_line(&encoded, &mut pos).unwrap() {
            PktLine::Data(d) => assert_eq!(d, b"NAK"),
            PktLine::Flush => panic!("expected data"),
        }
        assert_eq!(pos, encoded.len());
    }

    #[test]
    fn read_pkt_line_refuses_reserved_lengths() {
        for reserved in ["0001", "0002", "0003"] {
            let mut pos = 0;
            let err = read_pkt_line(reserved.as_bytes(), &mut pos).unwrap_err();
            assert!(
                err.contains("reserved"),
                "reserved length {reserved} refused"
            );
        }
    }

    #[test]
    fn read_pkt_line_refuses_lengths_beyond_the_protocol_maximum() {
        let mut pos = 0;
        let err = read_pkt_line(b"ffff", &mut pos).unwrap_err();
        assert!(err.contains("protocol maximum"));
    }

    #[test]
    fn read_pkt_line_refuses_a_truncated_header() {
        let mut pos = 0;
        assert!(read_pkt_line(b"00", &mut pos).is_err());
    }

    #[test]
    fn read_pkt_line_refuses_a_declared_length_beyond_the_buffer() {
        let mut pos = 0;
        assert!(read_pkt_line(b"00ffshort", &mut pos).is_err());
    }

    #[test]
    fn read_pkt_line_refuses_non_hex_header() {
        let mut pos = 0;
        assert!(read_pkt_line(b"zzzzrest", &mut pos).is_err());
    }

    // ---- advertisement parser ----

    #[test]
    fn advertisement_parser_finds_a_directly_advertised_oid() {
        let oid = sha1_oid(0x11);
        let first = format!("{oid} refs/heads/main\0no-progress ofs-delta shallow\n");
        let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
        let parsed =
            parse_upload_pack_advertisement(&advertisement(&first, &[]), &expected).unwrap();
        assert!(parsed.directly_advertised);
    }

    #[test]
    fn advertisement_parser_checks_every_ref_line_not_just_the_first() {
        let oid = sha1_oid(0x22);
        let first = format!(
            "{} refs/heads/main\0no-progress ofs-delta shallow\n",
            sha1_oid(0x33)
        );
        let second = format!("{oid} refs/heads/other\n");
        let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
        let parsed =
            parse_upload_pack_advertisement(&advertisement(&first, &[&second]), &expected)
                .unwrap();
        assert!(parsed.directly_advertised);
    }

    #[test]
    fn advertisement_parser_reports_allow_reachable_capability() {
        let oid = sha1_oid(0x44);
        let first = format!(
            "{} refs/heads/main\0no-progress ofs-delta shallow allow-reachable-sha1-in-want\n",
            sha1_oid(0x55)
        );
        let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
        let parsed =
            parse_upload_pack_advertisement(&advertisement(&first, &[]), &expected).unwrap();
        assert!(!parsed.directly_advertised);
        assert!(parsed.allows_reachable_want);
    }

    #[test]
    fn advertisement_parser_ignores_the_empty_repo_pseudo_ref() {
        let oid = sha1_oid(0x66);
        let first = format!(
            "{} capabilities^{{}}\0no-progress ofs-delta shallow\n",
            "0".repeat(40)
        );
        let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
        let parsed =
            parse_upload_pack_advertisement(&advertisement(&first, &[]), &expected).unwrap();
        assert!(!parsed.directly_advertised);
    }

    #[test]
    fn advertisement_parser_refuses_an_object_format_mismatch() {
        let oid = sha1_oid(0x76);
        let first = format!(
            "{} refs/heads/main\0no-progress ofs-delta shallow object-format=sha256\n",
            sha1_oid(0x77)
        );
        let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
        let err = parse_upload_pack_advertisement(&advertisement(&first, &[]), &expected)
            .unwrap_err();
        assert!(err.contains("object-format"));
    }

    #[test]
    fn advertisement_parser_refuses_a_first_line_with_no_capability_section() {
        let oid = sha1_oid(0x78);
        let first = format!("{} refs/heads/main\n", sha1_oid(0x79));
        let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
        let err = parse_upload_pack_advertisement(&advertisement(&first, &[]), &expected)
            .unwrap_err();
        assert!(err.contains("missing a capability section"));
    }

    #[test]
    fn advertisement_parser_refuses_a_missing_required_capability() {
        let oid = sha1_oid(0x7a);
        let first = format!(
            "{} refs/heads/main\0no-progress ofs-delta\n",
            sha1_oid(0x7b)
        );
        let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
        let err = parse_upload_pack_advertisement(&advertisement(&first, &[]), &expected)
            .unwrap_err();
        assert!(err.contains("does not advertise `shallow`"));
    }

    #[test]
    fn advertisement_parser_refuses_trailing_bytes_after_the_terminating_flush() {
        let oid = sha1_oid(0x7c);
        let first = format!(
            "{} refs/heads/main\0no-progress ofs-delta shallow\n",
            sha1_oid(0x7d)
        );
        let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
        let mut response = advertisement(&first, &[]);
        response.extend_from_slice(b"garbage-after-flush");
        let err = parse_upload_pack_advertisement(&response, &expected).unwrap_err();
        assert!(err.contains("trailing bytes"));
    }

    // ---- fetch-response parser ----

    #[test]
    fn fetch_response_parses_the_happy_path_with_no_shallow_line() {
        let oid = sha1_oid(0x88);
        let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
        let pack = fake_pack(b"root-commit-pack-bytes");
        let response = fetch_response(&[], "NAK", &pack);
        let mut out = Vec::new();
        let parsed =
            parse_checkout_fetch_response(&response, &expected, &mut out, 4096).unwrap();
        assert!(!parsed.shallow);
        assert_eq!(out, pack);
    }

    #[test]
    fn fetch_response_parses_the_happy_path_with_a_matching_shallow_line() {
        let oid = sha1_oid(0x99);
        let expected = ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
        let pack = fake_pack(b"shallow-pack-bytes");
        let response = fetch_response(&[format!("shallow {oid}\n")], "NAK", &pack);
        let mut out = Vec::new();
        let parsed =
            parse_checkout_fetch_response(&response, &expected, &mut out, 4096).unwrap();
        assert!(parsed.shallow);
        assert_eq!(out, pack);
    }

    #[test]
    fn fetch_response_refuses_a_shallow_line_naming_a_foreign_oid() {
        let oid = sha1_oid(0xaa);
        let other = sha1_oid(0xbb);
        let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
        let response = fetch_response(&[format!("shallow {other}\n")], "NAK", &fake_pack(b"x"));
        let mut out = Vec::new();
        let err =
            parse_checkout_fetch_response(&response, &expected, &mut out, 4096).unwrap_err();
        assert!(err.contains("shallow boundary names"));
    }

    #[test]
    fn fetch_response_refuses_a_duplicate_shallow_line() {
        let oid = sha1_oid(0xcc);
        let expected = ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
        let response = fetch_response(
            &[format!("shallow {oid}\n"), format!("shallow {oid}\n")],
            "NAK",
            &fake_pack(b"x"),
        );
        let mut out = Vec::new();
        let err =
            parse_checkout_fetch_response(&response, &expected, &mut out, 4096).unwrap_err();
        assert!(err.contains("duplicate"));
    }

    #[test]
    fn fetch_response_refuses_an_unshallow_line() {
        let oid = sha1_oid(0xdd);
        let expected = ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
        let response = fetch_response(&[format!("unshallow {oid}\n")], "NAK", &fake_pack(b"x"));
        let mut out = Vec::new();
        let err =
            parse_checkout_fetch_response(&response, &expected, &mut out, 4096).unwrap_err();
        assert!(err.contains("unshallow"));
    }

    #[test]
    fn fetch_response_refuses_an_err_line_in_shallow_info() {
        let oid = sha1_oid(0xee);
        let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
        let response = fetch_response(
            &["ERR upload-pack: not our ref\n".to_string()],
            "NAK",
            &fake_pack(b"x"),
        );
        let mut out = Vec::new();
        let err =
            parse_checkout_fetch_response(&response, &expected, &mut out, 4096).unwrap_err();
        assert!(err.contains("not our ref"));
    }

    #[test]
    fn fetch_response_refuses_an_ack_where_nak_is_expected() {
        let oid = sha1_oid(0xff);
        let expected = ExpectedGitCommitId::new(oid.clone(), GitObjectFormat::Sha1).unwrap();
        let response = fetch_response(&[], &format!("ACK {oid}"), &fake_pack(b"x"));
        let mut out = Vec::new();
        let err =
            parse_checkout_fetch_response(&response, &expected, &mut out, 4096).unwrap_err();
        assert!(err.contains("expected a single NAK"));
    }

    #[test]
    fn fetch_response_refuses_a_missing_pack_signature() {
        let oid = sha1_oid(0x12);
        let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
        let response = fetch_response(&[], "NAK", b"NOTAPACK...");
        let mut out = Vec::new();
        let err =
            parse_checkout_fetch_response(&response, &expected, &mut out, 4096).unwrap_err();
        assert!(err.contains("PACK"));
    }

    #[test]
    fn fetch_response_refuses_a_response_exceeding_the_cap() {
        let oid = sha1_oid(0x34);
        let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
        let pack = fake_pack(&[0u8; 100]);
        let response = fetch_response(&[], "NAK", &pack);
        let mut out = Vec::new();
        let err =
            parse_checkout_fetch_response(&response, &expected, &mut out, 10).unwrap_err();
        assert!(err.contains("exceeds"));
    }

    #[test]
    fn fetch_response_refuses_a_flush_where_nak_is_expected() {
        let oid = sha1_oid(0x56);
        let expected = ExpectedGitCommitId::new(oid, GitObjectFormat::Sha1).unwrap();
        let mut response = b"0000".to_vec(); // shallow-info flush
        response.extend_from_slice(b"0000"); // a second flush instead of NAK
        let mut out = Vec::new();
        let err =
            parse_checkout_fetch_response(&response, &expected, &mut out, 4096).unwrap_err();
        assert!(err.contains("unexpected flush"));
    }
}

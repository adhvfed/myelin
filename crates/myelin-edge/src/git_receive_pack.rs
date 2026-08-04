use crate::git_wire_http::pkt_line;
use std::collections::BTreeSet;

pub(crate) const RECV_CAPS: &str =
    "report-status delete-refs ofs-delta object-format=sha1 agent=myelin/ct006d";

pub(crate) const RECEIVE_PACK_ADV: &str = "application/x-git-receive-pack-advertisement";
pub(crate) const RECEIVE_PACK_RESULT: &str = "application/x-git-receive-pack-result";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RefCommand {
    pub old: String,
    pub new: String,
    pub ref_name: String,
}

fn read_pkt(buf: &[u8]) -> Result<(Option<&[u8]>, &[u8]), String> {
    if buf.len() < 4 {
        return Err("truncated pkt-line length prefix".into());
    }
    let hdr = std::str::from_utf8(&buf[..4]).map_err(|_| "non-ascii pkt-line length".to_string())?;
    let len = usize::from_str_radix(hdr, 16).map_err(|_| format!("bad pkt-line hex length {hdr:?}"))?;
    if len == 0 {
        return Ok((None, &buf[4..]));
    }
    if len < 4 || len > buf.len() {
        return Err(format!("pkt-line length {len} out of range (buf {})", buf.len()));
    }
    Ok((Some(&buf[4..len]), &buf[len..]))
}

pub(crate) fn parse_push_request(body: &[u8]) -> Result<(Vec<RefCommand>, Vec<u8>), String> {
    let mut rest = body;
    let mut cmds = Vec::new();
    let mut ref_names = BTreeSet::new();
    loop {
        let (payload, next) = read_pkt(rest)?;
        rest = next;
        let Some(p) = payload else { break };
        let mut line_and_caps = p.splitn(2, |&byte| byte == 0);
        let line = line_and_caps.next().unwrap_or(p);
        let capabilities = line_and_caps.next();
        if !cmds.is_empty() && capabilities.is_some() {
            return Err("capability list is only valid on the first ref-update command".into());
        }
        if capabilities.is_some_and(|caps| caps.contains(&0)) {
            return Err("ref-update command carries multiple NUL separators".into());
        }
        let s = std::str::from_utf8(line)
            .map_err(|_| "non-utf8 ref-update command".to_string())?
            .trim_end_matches('\n');
        let parts: Vec<&str> = s.splitn(3, ' ').collect();
        let valid_oid = |oid: &str| {
            oid.len() == 40 && oid.bytes().all(|byte| byte.is_ascii_hexdigit())
        };
        if parts.len() != 3
            || !valid_oid(parts[0])
            || !valid_oid(parts[1])
            || !parts[2].starts_with("refs/")
            || parts[2].bytes().any(|byte| byte.is_ascii_control() || byte == b' ')
        {
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
    fn malformed_oids_and_late_capability_lists_are_rejected() {
        let z = "0".repeat(40);
        let non_hex = "g".repeat(40);
        let malformed = format!("{z} {non_hex} refs/heads/topic\n");
        let mut body = format!("{:04x}{malformed}", malformed.len() + 4).into_bytes();
        body.extend_from_slice(b"0000PACKmust-not-be-ingested");
        assert!(
            parse_push_request(&body)
                .expect_err("non-hex object id must fail at the protocol boundary")
                .contains("malformed ref-update command")
        );

        let first = format!("{z} {} refs/heads/one\n", "1".repeat(40));
        let second = format!("{z} {} refs/heads/two\0report-status\n", "2".repeat(40));
        let mut body = format!("{:04x}{first}", first.len() + 4).into_bytes();
        body.extend_from_slice(format!("{:04x}{second}", second.len() + 4).as_bytes());
        body.extend_from_slice(b"0000");
        assert_eq!(
            parse_push_request(&body).unwrap_err(),
            "capability list is only valid on the first ref-update command"
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

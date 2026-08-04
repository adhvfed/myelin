use myelin_refs::{mint, ArtifactRef, ParseError, Sub, SubKind, SubKindRegistration};

pub const CHAT_SUBSYSTEM: &str = "chat";

pub const CHAT_OWNED_SUB_KINDS: &[SubKind] = &[SubKind::Message, SubKind::Thread];

pub fn register_chat_sub_kinds() -> Result<SubKindRegistration, myelin_refs::RegistrationError> {
    SubKindRegistration {
        subsystem: CHAT_SUBSYSTEM.to_string(),
        kinds: CHAT_OWNED_SUB_KINDS.to_vec(),
    }
    .validate()
}

fn message_root(tenant: &str, message_id: &str) -> Result<ArtifactRef, ParseError> {
    myelin_refs::parse(&format!("myelin://{tenant}/chat/message/{message_id}"))
}

fn thread_root(tenant: &str, thread_root_id: &str) -> Result<ArtifactRef, ParseError> {
    myelin_refs::parse(&format!("myelin://{tenant}/chat/thread/{thread_root_id}"))
}

pub fn mint_channel(tenant: &str, channel_id: &str) -> Result<ArtifactRef, ParseError> {
    myelin_refs::parse(&format!("myelin://{tenant}/chat/channel/{channel_id}"))
}

pub fn mint_message(tenant: &str, message_id: &str) -> Result<ArtifactRef, ParseError> {
    let root = message_root(tenant, message_id)?;
    mint(&root, Sub::Message(message_id.to_string()))
}

pub fn mint_thread(tenant: &str, thread_root_id: &str) -> Result<ArtifactRef, ParseError> {
    let root = thread_root(tenant, thread_root_id)?;
    mint(&root, Sub::Thread(thread_root_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_refs::{strip_sub, sub_kind};

    #[test]
    fn chat_sub_kind_registration_is_accepted_and_declares_only_chat_owned_kinds() {
        let reg = register_chat_sub_kinds().expect("Refs must accept chat's #sub registration");
        assert_eq!(reg.subsystem, "chat");
        assert_eq!(reg.kinds, vec![SubKind::Message, SubKind::Thread]);
        for foreign in [
            SubKind::Comment,
            SubKind::LineRange,
            SubKind::Block,
            SubKind::Heading,
            SubKind::Row,
            SubKind::Field,
            SubKind::Check,
            SubKind::Step,
        ] {
            assert!(
                !reg.kinds.contains(&foreign),
                "chat must not claim the foreign kind {foreign:?}"
            );
        }
    }

    #[test]
    fn chat_mints_produce_grammatical_round_tripping_sub_urns() {
        let m = mint_message("acme-eu", "01J0MSGULID").unwrap();
        assert_eq!(
            myelin_refs::format(&m),
            "myelin://acme-eu/chat/message/01J0MSGULID#message-01J0MSGULID"
        );
        assert_eq!(sub_kind(&m).map(|s| s.kind()), Some(SubKind::Message));
        assert_eq!(
            myelin_refs::format(&strip_sub(&m)),
            "myelin://acme-eu/chat/message/01J0MSGULID"
        );

        let t = mint_thread("acme-eu", "01J0THRROOT").unwrap();
        assert_eq!(
            myelin_refs::format(&t),
            "myelin://acme-eu/chat/thread/01J0THRROOT#thread-01J0THRROOT"
        );
        assert_eq!(sub_kind(&t).map(|s| s.kind()), Some(SubKind::Thread));
        assert_eq!(
            myelin_refs::format(&strip_sub(&t)),
            "myelin://acme-eu/chat/thread/01J0THRROOT"
        );
    }

    #[test]
    fn the_sub_is_stable_across_edits_because_the_id_is_immutable() {
        let before = mint_message("acme", "01J0STABLE").unwrap();
        let after = mint_message("acme", "01J0STABLE").unwrap();
        assert_eq!(
            before, after,
            "the #sub is stable across edits (the id is immutable)"
        );
    }

    #[test]
    fn empty_opaque_id_is_rejected_at_mint_time() {
        assert!(matches!(
            mint_message("acme", ""),
            Err(ParseError::EmptySegment { .. } | ParseError::UnknownSubKind { .. })
        ));
        assert!(matches!(
            mint_thread("acme", ""),
            Err(ParseError::EmptySegment { .. } | ParseError::UnknownSubKind { .. })
        ));
    }
}

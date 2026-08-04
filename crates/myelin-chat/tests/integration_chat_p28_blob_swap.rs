#![cfg(feature = "integration")]

use myelin_chat::{chat_cold_blob_store_parity, AuthorKind, ConversationId, Message, MessageId};
use myelin_config::MyelinConfig;
use myelin_storage::s3blob::S3BlobStore;
use myelin_storage::FsBlobStore;
use myelin_tenancy::TenantId;

fn sample_cold_batch(conv: &ConversationId) -> Vec<Message> {
    (0..8u128)
        .map(|i| Message {
            message_id: MessageId::from_u128(i),
            conv: conv.clone(),
            thread_root_id: if i % 3 == 0 {
                None
            } else {
                Some(MessageId::from_u128(0))
            },
            author: format!("subject-{i}"),
            author_kind: match i % 3 {
                0 => AuthorKind::Human,
                1 => AuthorKind::Agent,
                _ => AuthorKind::Service,
            },
            body_inline: format!("cold segment body {i} - the quick brown fox").into_bytes(),
            body_nodes: vec![i as u8, 0xAB, 0xCD],
            client_nonce: format!("nonce-{i}"),
            edited_seq: (i % 2) as i32,
            state: myelin_chat::MessageState::Active,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_p28_cold_segment_object_store_swap_is_byte_identical_to_the_fs_floor() {
    let cfg = MyelinConfig::dev();
    let endpoint = cfg.s3.endpoint.clone();
    let handle = tokio::runtime::Handle::current();

    let tenant = TenantId(format!("itest-chatp28-{}", std::process::id()));
    let conv = ConversationId::new(tenant.0.clone(), "fr-par", "01J0CONV");

    let verdict = {
        let tenant = tenant.clone();
        tokio::task::spawn_blocking(move || {
            let fs = FsBlobStore::new();
            let object = S3BlobStore::connect(&cfg.s3, handle.clone());
            let batch = sample_cold_batch(&conv);
            chat_cold_blob_store_parity(&fs, &object, &tenant, &batch)
                .expect("the parity check runs against the live object store")
        })
        .await
        .expect("the blocking parity task completes")
    };

    assert_eq!(
        verdict.fs_address, verdict.object_address,
        "the content address is IDENTICAL across the fs floor and the live object store \
         (BLAKE3-of-the-encoded-segment is backing-independent)"
    );
    assert!(
        verdict.byte_identical,
        "the cold-segment object-store swap is BYTE-IDENTICAL to the fs floor: same address, same \
         decoded message rows back from both stores (the swap is behaviour-preserving - the 11.2 \
         one-line backing change, CHAT-P28)"
    );

    println!(
        "[P-502 CHAT-P28 INTEGRATION GREEN] cold-segment object-store BlobStore swap PROVEN against \
         the live dev stack ({endpoint} RustFS): the sealed cold segment is content-addressed + \
         byte-identical fs↔S3. CHAT-P04/P05 fs floor for cold segments RESOLVED - the swap is a \
         one-line backing change behind the BlobStore trait (ColdSegments<B>)."
    );
}

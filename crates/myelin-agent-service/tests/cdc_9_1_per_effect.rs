use myelin_agent_service::per_effect_idem_key as fabric_key;
use myelin_flow::approval::per_effect_idem_key as engine_key;

#[test]
fn cdc_9_1_fabric_and_engine_per_effect_keys_agree() {
    let card_id = "card-7";
    assert_eq!(
        fabric_key(card_id, 0, 1),
        engine_key(card_id, 0, 1),
        "single-effect: the agent fabric and the durable engine derive the SAME key (the bare card id)"
    );
    assert_eq!(fabric_key(card_id, 0, 1), "card-7");

    for total in 2..=6usize {
        for idx in 0..total {
            assert_eq!(
                fabric_key(card_id, idx, total),
                engine_key(card_id, idx, total),
                "multi-effect (idx {idx} of {total}): the fabric and engine keys MUST agree (else a \
                 double-click slips a second apply / a partial approval couples effects)"
            );
        }
    }
    assert_eq!(fabric_key(card_id, 0, 3), "card-7:0");
    assert_eq!(fabric_key(card_id, 1, 3), "card-7:1");
    assert_eq!(fabric_key(card_id, 2, 3), "card-7:2");
}

#[test]
fn cdc_9_1_partial_approval_keys_are_three_independent_and_agree() {
    let card_id = "card-7";
    let total = 3;
    let keys: Vec<String> = (0..total)
        .map(|idx| fabric_key(card_id, idx, total))
        .collect();
    assert_eq!(keys.len(), 3);
    assert_ne!(keys[0], keys[1]);
    assert_ne!(keys[1], keys[2]);
    assert_ne!(keys[0], keys[2]);
    for (idx, k) in keys.iter().enumerate() {
        assert_eq!(
            *k,
            engine_key(card_id, idx, total),
            "partial-approval key {idx} agrees with the engine"
        );
    }
}

use myelin_chat::glue::{te21_harness_shim_obligation, Te21LanguagePin};

fn provider_te21_pin() -> Te21LanguagePin {
    te21_harness_shim_obligation()
}

fn consumer_shim_is_no_op(pin: Te21LanguagePin) -> bool {
    pin.is_no_op()
}

#[test]
fn cdc_1_7_chat_provider_pins_rust_consumer_shim_is_a_no_op() {
    let pin = provider_te21_pin();
    assert_eq!(pin, Te21LanguagePin::Rust, "the M2-C0 TE-21 pin is Rust");
    assert!(
        consumer_shim_is_no_op(pin),
        "the all-Rust default makes the 1.7 harness shim a NO-OP"
    );
    assert_eq!(Te21LanguagePin::PINNED, Te21LanguagePin::Rust);
}

#[test]
fn cdc_1_7_the_beam_hatch_carries_the_shim_obligation_when_selected() {
    assert!(
        !Te21LanguagePin::Beam.is_no_op(),
        "the BEAM hatch is written-but-closed - its 1.7 harness-shim obligations bind (CHAT-P26)"
    );
}

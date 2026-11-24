//! A box proof, as a node or a peer hands one over: walked and verified by the wallet's own reader with no
//! `catch_unwind` (`tachys::fuzzing::box_proof`). A panic here is a bound the walk missed, and on a phone, where a
//! panic aborts, it would end the core. Whatever is read must also be what the honest proof says: a proof cannot be
//! bent into showing another box.
#![no_main]
use libfuzzer_sys::fuzz_target;

mod honest_box_proof;
use honest_box_proof::{KEYS, ROOT};

fuzz_target!(|proof: &[u8]| {
    static HONEST: std::sync::OnceLock<Option<Vec<Option<Vec<u8>>>>> = std::sync::OnceLock::new();
    let honest = HONEST.get_or_init(|| {
        tachys::fuzzing::box_proof(&ROOT, include_bytes!("../seeds/box_proof/honest"), &KEYS)
    });
    assert!(honest.is_some(), "the honest proof is read");
    let got = tachys::fuzzing::box_proof(&ROOT, proof, &KEYS);
    assert!(
        got.is_none() || &got == honest,
        "a proof that is accepted says what the honest one says"
    );
});

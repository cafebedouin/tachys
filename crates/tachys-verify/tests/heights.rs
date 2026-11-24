//! A proof whose headers claim heights near `u32::MAX`. The height of the next difficulty recalculation after the
//! suffix head overflowed `u32` there (heights 4,294,967,169 and up, with mainnet's 128-block epochs): a debug build
//! panicked, and a release or phone build wrapped it to 1, so C9 asked for no epoch-boundary headers and a proof
//! without them was still reported `Bounded`. Such a proof is now refused: no next recalculation height, no C9.
//!
//! The proof is the real mainnet vector with the boundary headers C9 asks for removed from its prefix, relabelled to
//! heights an ordinary distance on, or to a suffix head at 4,294,967,200. Relabelled headers fail C5 (the proof of work
//! covers the height), so the same proofs are also checked with PoW checks off (version 2 starting above every height,
//! a parameter made up for the test), to show that, the other checks equal, the wrapped proof is no longer accepted.
use ergo_chain_types::{BlockId, Digest32, Header};
use ergo_nipopow::NipopowProof;
use sigma_ser::ScorexSerializable;
use tachys_verify::difficulty::*;
use tachys_verify::*;

const GENESIS: &str = "b0244dfc267baca974a4caee06120321562784303a8a688976ae56170e4d175b";
const NEAR_MAX: u32 = 4_294_967_200;

fn real() -> NipopowProof {
    NipopowProof::scorex_parse_bytes(include_bytes!("../../../vectors/mainnet-1915810.bin"))
        .unwrap()
}
fn hdr(p: &NipopowProof, h: u32) -> Header {
    p.prefix
        .iter()
        .map(|x| &x.header)
        .find(|x| x.height == h)
        .unwrap()
        .clone()
}
fn anchor(cp: &Header) -> Anchor {
    Anchor {
        genesis_id: BlockId(Digest32::try_from(GENESIS.to_string()).unwrap()),
        checkpoint_height: cp.height,
        checkpoint_difficulty: difficulty(cp),
        checkpoint_timestamp_ms: cp.timestamp,
        floor_divisor: 4,
        floor_halving_ms: 0,
        min_k: 10,
        max_tip_age_ms: 3 * 3600 * 1000,
        max_future_drift_ms: 20 * 60 * 1000,
    }
}
fn shifted(p: &NipopowProof, delta: u32) -> NipopowProof {
    let mut q = p.clone();
    for h in q.prefix.iter_mut().filter(|h| h.header.height > 1) {
        h.header.height += delta;
    }
    q.suffix_head.header.height += delta;
    for h in q.suffix_tail.iter_mut() {
        h.height += delta;
    }
    q
}
fn show(name: &str, r: &Report) {
    println!(
        "{name:44} accepted={} failed={:?} difficulty={:?}",
        r.accepted(),
        r.failed(),
        r.difficulty
    );
    for (c, m) in r.failures.iter().filter(|f| f.0 != Check::C5Work).take(3) {
        println!("      {c:?}: {m}");
    }
}

#[test]
fn the_next_recalculation_height_is_none_past_the_largest_height() {
    assert_eq!(next_recalculation_height(1_915_810, 128), Some(1_915_905));
    assert_eq!(next_recalculation_height(1_915_776, 128), Some(1_915_777));
    assert_eq!(
        next_recalculation_height(4_294_967_168, 128),
        Some(4_294_967_169)
    );
    // the last epoch that ends inside u32 ends at 4,294,967,168; after it there is no next recalculation height
    assert_eq!(next_recalculation_height(4_294_967_169, 128), None);
    assert_eq!(next_recalculation_height(NEAR_MAX, 128), None);
    assert_eq!(next_recalculation_height(u32::MAX, 128), None);
    assert_eq!(
        heights_for_next_recalculation(NEAR_MAX, &ChainParams::mainnet()),
        None
    );
}

#[test]
fn a_proof_near_the_largest_height_is_refused_not_wrapped() {
    let p = real();
    let cp = hdr(&p, 1_914_880);
    let a = anchor(&cp);
    let mainnet = ChainParams::mainnet();
    let sh = p.suffix_head.header.height;
    let mut stripped = p.clone();
    let needed: Vec<u32> = (1..=8).map(|i| sh - sh % 128 - (8 - i) * 128).collect();
    stripped
        .prefix
        .retain(|h| !needed.contains(&h.header.height));
    assert_eq!(
        stripped.prefix.len() + 8,
        p.prefix.len(),
        "the eight boundary headers C9 asks for are taken out: {needed:?}"
    );
    let now = p.suffix_tail.last().unwrap().timestamp + 600_000;
    let run = |name: &str, q: &NipopowProof, params: &ChainParams| {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            verify(q, &[], &a, params, now)
        }))
        .unwrap_or_else(|_| panic!("{name}: verify panicked"));
        show(name, &r);
        r
    };
    let no_pow = ChainParams {
        v2_activation_height: u32::MAX,
        ..ChainParams::mainnet()
    };
    for (params, which) in [(&mainnet, "mainnet"), (&no_pow, "no PoW check")] {
        let at_real = run(&format!("{which}: boundaries removed"), &stripped, params);
        let ordinary = run(
            &format!("{which}: boundaries removed, +128000"),
            &shifted(&stripped, 128 * 1000),
            params,
        );
        let near_max = run(
            &format!("{which}: boundaries removed, head at {NEAR_MAX}"),
            &shifted(&stripped, NEAR_MAX - sh),
            params,
        );
        for r in [&at_real, &ordinary, &near_max] {
            assert!(r.failed().contains(&Check::C9DifficultyHeaders) && !r.accepted());
        }
        assert!(
            near_max
                .failures
                .iter()
                .any(|f| f.0 == Check::C9DifficultyHeaders
                    && f.1.contains("past the largest height")),
            "{:?}",
            near_max.failures
        );
    }
    // the proof as served is still accepted
    assert!(run("mainnet: as served", &p, &mainnet).accepted());
}

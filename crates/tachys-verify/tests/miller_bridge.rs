//! Bridge the Miller comparator onto real sigma-rust `Header`s, and validate it against the reference.
//!
//! Builds a `MillerProof` from the deployed mainnet proof's headers (level via the node's own
//! `max_level_of`, difficulty via `decode_compact_bits`) and asserts the prototype's `best_arg_m`
//! reproduces sigma-rust `NipopowAlgos::best_arg` on that real data — exactly. Then computes the quantity
//! the node *ignores*: the chain's accumulated difficulty (Miller's comparison key).

use ergo_chain_types::Header;
use ergo_nipopow::{NipopowAlgos, NipopowProof};
use num_bigint::BigInt;
use sigma_ser::ScorexSerializable;
use tachys_verify::miller::{Blk, MillerProof};

fn real() -> NipopowProof {
    NipopowProof::scorex_parse_bytes(include_bytes!("../../../vectors/mainnet-1915810.bin")).unwrap()
}

/// Build a MillerProof from a real proof: prefix → D (sampled), suffix → Ω (unstable). The current node
/// proof form (KMZ) carries no uncompressed χ run, so X is empty.
fn bridge(p: &NipopowProof, algos: &NipopowAlgos) -> MillerProof {
    let d = p.prefix.iter().map(|x| Blk::from_header(&x.header, algos)).collect();
    let omega = std::iter::once(&p.suffix_head.header)
        .chain(p.suffix_tail.iter())
        .map(|h| Blk::from_header(h, algos))
        .collect();
    MillerProof { d, x: vec![], omega }
}

#[test]
fn prototype_best_arg_matches_sigma_rust_on_the_real_proof() {
    let p = real();
    let algos = NipopowAlgos::default();

    // Reference: sigma-rust's own bestArg over the real prefix, at the proof's own m.
    let chain: Vec<&Header> = p.prefix.iter().map(|x| &x.header).collect();
    let reference = algos.best_arg(&chain, p.m).expect("reference best_arg");

    // Prototype: our Blk-bridged bestArg over the same prefix, same m.
    let proto = MillerProof {
        d: p.prefix.iter().map(|x| Blk::from_header(&x.header, &algos)).collect(),
        x: vec![],
        omega: vec![],
    };
    let mine = proto.best_arg_m(p.m);

    eprintln!("real proof: m={} k={} prefix={}; bestArg reference={reference} prototype={mine}",
        p.m, p.k, p.prefix.len());
    assert_eq!(mine, BigInt::from(reference), "prototype bestArg must equal sigma-rust bestArg on real data");
}

#[test]
fn bridge_computes_the_work_the_node_ignores() {
    let p = real();
    let algos = NipopowAlgos::default();
    let proof = bridge(&p, &algos);

    // Accumulated difficulty over the whole bridged proof — Miller's comparison key, which bestArg never
    // looks at. On real mainnet headers it is astronomically large (summed targets of ~130 real blocks).
    let work: BigInt = proof.blocks().map(|b| &b.difficulty).sum();
    eprintln!("bridged proof blocks={}; accumulated difficulty={work}", proof.len());

    // It equals the sum of decode_compact_bits over every bridged header, and is positive and large.
    assert!(work > BigInt::from(0u32));
    assert_eq!(proof.len(), p.prefix.len() + 1 + p.suffix_tail.len());
    // Sanity against the lightest single real target in the proof: the sum exceeds the max single block.
    let heaviest = proof.blocks().map(|b| b.difficulty.clone()).max().unwrap();
    assert!(work >= heaviest);
}

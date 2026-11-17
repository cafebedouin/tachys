//! Interlinks bound to the header from the bytes a node serves, and what a proof ties to its suffix head.
use ergo_nipopow::NipopowProof;
use sigma_ser::ScorexSerializable;
use tachys_verify::*;

fn real() -> NipopowProof { NipopowProof::scorex_parse_bytes(include_bytes!("../../../vectors/mainnet-1915810.bin")).unwrap() }

fn heights(p: &NipopowProof, b: Binding) -> Vec<u32> {
    p.prefix.iter().chain(std::iter::once(&p.suffix_head)).filter(|h| interlinks_binding(h) == b).map(|h| h.header.height).collect()
}

fn describe(name: &str, p: &NipopowProof) -> Reach {
    let r = reach(p);
    println!("{name}: m={} k={} headers with interlinks {} tip {}", p.m, p.k, p.prefix.len() + 1,
        p.suffix_tail.last().map_or(p.suffix_head.header.height, |h| h.height));
    println!("  bound {}  genesis {}  unbound {:?}", heights(p, Binding::Bound).len(), heights(p, Binding::Genesis).len(), heights(p, Binding::Unbound));
    println!("  reached {}  of them unbound {:?}  unreached {:?}", r.reached.len(), r.unbound, r.unreached);
    r
}

#[test]
fn the_served_bytes_bind_every_header_but_the_first_of_a_voting_epoch() {
    let p = real();
    assert_eq!(heights(&p, Binding::Genesis), vec![1]);
    assert_eq!(heights(&p, Binding::Unbound), vec![1_914_880]);
    assert_eq!(1_914_880 % 1024, 0);
    assert_eq!(heights(&p, Binding::Bound).len(), p.prefix.len() + 1 - 2);
    // the node's own check passes all of them: it does not tell the two kinds apart
    assert!(p.prefix.iter().chain(std::iter::once(&p.suffix_head)).all(interlinks_proof_ok));
}

#[test]
fn interlinks_of_another_header_are_not_bound() {
    let mut p = real();
    let donor = p.prefix[p.prefix.len() - 3].clone();
    let last = p.prefix.len() - 1;
    assert_eq!(interlinks_binding(&p.prefix[last]), Binding::Bound);
    assert_ne!(donor.interlinks, p.prefix[last].interlinks);
    p.prefix[last].interlinks = donor.interlinks.clone();
    p.prefix[last].interlinks_proof = donor.interlinks_proof.clone();
    assert_eq!(interlinks_binding(&p.prefix[last]), Binding::Unbound);
}

#[test]
fn what_the_proof_ties_to_its_suffix_head() {
    let p = real();
    let r = describe("mainnet proof at 1,915,810", &p);
    assert_eq!(r.reached.len() + r.unreached.len(), p.prefix.len());
    assert!(r.reached.contains(&1), "genesis is reached");
    // every header left out is one the next difficulty rests on, at a multiple of the epoch length
    assert!(!r.unreached.is_empty());
    assert!(r.unreached.iter().all(|h| h % 128 == 0), "{:?}", r.unreached);
    // the one unbound header is among those left out, so no link of the chain shown runs through it
    assert!(r.unbound.is_empty(), "{:?}", r.unbound);
    assert!(r.unreached.contains(&1_914_880));
}

/// Other proofs, as JSON from a node's REST API: `TACHYS_PROOF_JSON=a.json:b.json cargo test -- --ignored`.
#[test]
#[ignore]
fn other_proofs() {
    let paths = std::env::var("TACHYS_PROOF_JSON").expect("TACHYS_PROOF_JSON");
    for path in paths.split(':') {
        let p: NipopowProof = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let r = describe(path.rsplit('/').next().unwrap(), &p);
        assert_eq!(r.reached.len() + r.unreached.len(), p.prefix.len());
    }
}

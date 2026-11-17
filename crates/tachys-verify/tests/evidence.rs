//! What one peer's answers tie to its tip, and how two peers' results are weighed.
use ergo_chain_types::{BlockId, Digest32, Header};
use ergo_nipopow::NipopowProof;
use sigma_ser::ScorexSerializable;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use tachys_verify::difficulty::ChainParams;
use tachys_verify::evidence::*;
use tachys_verify::run::RunReport;

fn real() -> NipopowProof { NipopowProof::scorex_parse_bytes(include_bytes!("../../../vectors/mainnet-1915810.bin")).unwrap() }
fn none() -> HashSet<BlockId> { HashSet::new() }

/// A header made for a test: a real one with another height, id and difficulty. Its work is not valid, so these
/// tests use parameters under which no header's work is checkable.
fn made(height: u32, tag: u8, n_bits: u32) -> Header {
    let mut h = real().suffix_head.header;
    let mut id = [tag; 32];
    id[..4].copy_from_slice(&height.to_be_bytes());
    h.height = height; h.id = BlockId(Digest32::from(id)); h.n_bits = n_bits;
    h
}
fn unchecked_work() -> ChainParams { ChainParams { v2_activation_height: u32::MAX, ..ChainParams::mainnet() } }

/// A result made by hand: `shared` heights carry tag 0, the side's own heights its own tag and difficulty.
fn side(shared: std::ops::RangeInclusive<u32>, own: std::ops::RangeInclusive<u32>, tag: u8, n_bits: u32, with_run: bool) -> Evidence {
    let tied: BTreeMap<u32, Header> = shared.clone().map(|h| (h, made(h, 0, 0x0500_0001))).chain(own.clone().map(|h| (h, made(h, tag, n_bits)))).collect();
    let tip = tied.values().next_back().unwrap().clone();
    let run = with_run.then(|| RunReport { first_height: *shared.start(), last_height: tip.height, recomputed: 0, as_parent: 0, within_bounds: 0,
        unchecked: 0, wrong: vec![], exact_from: Some(*shared.start()), work: 0.into(), work_exact: 0.into() });
    Evidence { tied, tip, run, unbound_on_path: vec![], unreached: vec![], work_unchecked: 0 }
}

#[test]
fn a_proof_alone_ties_what_its_suffix_head_reaches() {
    let p = real();
    let e = assemble(&p, &none(), &[], &ChainParams::mainnet()).expect("a served proof");
    let reach = tachys_verify::reach(&p);
    assert_eq!(e.tip.height, 1_915_819);
    assert_eq!(e.tied.len(), reach.reached.len() + p.k as usize);
    assert_eq!(e.unreached, reach.unreached);
    assert_eq!(e.unreached.len(), 7);
    assert!(e.unreached.iter().all(|h| !e.tied.contains_key(h)));
    assert!(e.unbound_on_path.is_empty());
    assert_eq!(e.work_unchecked, e.tied.keys().filter(|h| **h < 417_792).count());
    assert!(e.run.is_none());
}

#[test]
fn a_run_must_start_at_a_header_the_proof_ties_and_stay_on_its_chain() {
    let p = real();
    let params = unchecked_work();
    let tied = assemble(&p, &none(), &[], &params).unwrap().tied;
    // a real header of the proof whose parent the proof does not hold
    let lone = p.prefix.iter().map(|x| &x.header).find(|h| h.height > 1000 && tied.contains_key(&h.height) && !tied.contains_key(&(h.height - 1))).unwrap().clone();
    assert_eq!(assemble(&p, &none(), std::slice::from_ref(&lone), &params).unwrap_err(), Refused::RunNotTied);
    // a run from the suffix head's own child onward is tied
    let tail = p.suffix_tail.clone();
    let ok = assemble(&p, &none(), &tail, &params).expect("the proof's own suffix as a run");
    assert_eq!(ok.run.as_ref().map(|r| (r.first_height, r.last_height)), Some((tail[0].height, 1_915_819)));
    // the same run with another header at one height: consecutive, tied at its start, and not the proof's chain
    let mut other = tail.clone();
    other[4].id = made(other[4].height, 9, other[4].n_bits).id;
    other[5].parent_id = other[4].id;
    assert_eq!(assemble(&p, &none(), &other, &params).unwrap_err(), Refused::RunLeavesProof(other[4].height));
    // headers that do not follow one another are not a run
    assert_eq!(assemble(&p, &none(), &[tail[0].clone(), tail[2].clone()], &params).unwrap_err(), Refused::NotOneRun);
    // work is checked where it can be: the altered header fails under the real parameters
    assert_eq!(assemble(&p, &none(), &other, &ChainParams::mainnet()).unwrap_err(), Refused::InvalidWork(other[5].height));
}

#[test]
fn a_tip_on_the_other_chain_is_behind_it() {
    let (short, long) = (side(90..=100, 101..=103, 0, 0x0500_0001, true), side(90..=100, 101..=108, 0, 0x0500_0001, true));
    let c = compare(&short, &long, 6);
    assert_eq!((c.common, c.basis.clone(), c.better), (Some(103), Basis::SameChain, Ordering::Less));
    assert_eq!(compare(&long, &short, 6).better, Ordering::Greater);
    assert_eq!(compare(&long, &long, 6).better, Ordering::Equal);
    assert_eq!(pick(&[short.clone(), long.clone(), short], 6).map(|x| x.0), Some(1));
}

#[test]
fn two_runs_from_a_common_header_are_weighed_by_summed_difficulty() {
    // five headers at a low difficulty against three at a higher one: 0x05000100 states 256 times 0x05000001
    let (many, heavy) = (side(90..=100, 101..=105, 1, 0x0500_0001, true), side(90..=100, 101..=103, 2, 0x0500_0100, true));
    let c = compare(&many, &heavy, 6);
    assert_eq!((c.common, c.basis.clone()), (Some(100), Basis::Run { exact: true }));
    assert_eq!(c.better, Ordering::Less, "more headers of less work must lose: {} against {}", c.ours, c.theirs);
    assert_eq!(c.theirs.clone() * 5, c.ours.clone() * 3 * 256);
    assert_eq!(pick(&[many, heavy], 6).map(|x| x.0), Some(1));
}

#[test]
fn without_a_run_on_both_sides_headers_are_counted_and_too_few_decide_nothing() {
    // neither side has six headers above the common one, and one side has no run: nothing is claimed
    let (a, b) = (side(90..=100, 101..=105, 1, 0x0500_0001, true), side(90..=100, 101..=103, 2, 0x0500_0100, false));
    let c = compare(&a, &b, 6);
    assert_eq!((c.common, c.basis, c.better), (Some(100), Basis::Undecided, Ordering::Equal));
    // with a lower bar the five outweigh the three whatever difficulty they state: the count reads hits alone
    let c = compare(&a, &b, 1);
    assert_eq!(c.basis, Basis::Count);
    // two results that share no header are not compared
    let apart = side(10..=20, 21..=25, 3, 0x0500_0001, true);
    let mut shifted = apart.clone();
    shifted.tied = shifted.tied.into_iter().map(|(h, x)| (h, made(h, 7, x.n_bits))).collect();
    assert_eq!(compare(&apart, &shifted, 6).basis, Basis::NoCommonHeader);
}

/// `TACHYS_HEADERS_JSON=<stretch> TACHYS_PROOF_JSON=<a proof whose tip the stretch reaches> cargo test --test evidence -- --ignored --nocapture`
#[test]
#[ignore]
fn on_real_proofs_and_a_stretch_of_mainnet() {
    let p = ChainParams::mainnet();
    let mut headers: Vec<Header> = vec![];
    for path in std::env::var("TACHYS_HEADERS_JSON").expect("TACHYS_HEADERS_JSON").split(':') {
        headers.extend(serde_json::from_str::<Vec<Header>>(&std::fs::read_to_string(path).unwrap()).unwrap());
    }
    headers.sort_by_key(|h| h.height);
    let newer: NipopowProof = serde_json::from_str(&std::fs::read_to_string(std::env::var("TACHYS_PROOF_JSON").expect("TACHYS_PROOF_JSON")).unwrap()).unwrap();
    let tip = newer.suffix_tail.last().unwrap().height;
    let alone = assemble(&newer, &none(), &[], &p).expect("the newer proof");
    // the run a wallet would fetch: from the highest tied header far enough below the tip, to the tip
    let from = *alone.tied.keys().filter(|h| **h + tachys_verify::run::run_length_for(tip, &p) <= tip).next_back().expect("a tied header below the run");
    let run: Vec<Header> = headers.iter().filter(|h| h.height > from && h.height <= tip).cloned().collect();
    let with_run = assemble(&newer, &none(), &run, &p).expect("the newer proof with its run");
    let r = with_run.run.clone().unwrap();
    println!("newer proof: tip {tip}, tied alone {}, run from {} ({} headers), exact from {:?}, tied with the run {}",
        alone.tied.len(), from + 1, run.len(), r.exact_from, with_run.tied.len());
    assert!(r.exact_from.is_some_and(|f| f <= tip - 128));
    let c = compare(&alone, &with_run, 6);
    println!("the proof alone against the proof with its run: {:?} {:?}", c.basis, c.better);
    assert_eq!((c.basis, c.better), (Basis::SameChain, Ordering::Equal));

    let older = assemble(&real(), &none(), &[], &p).expect("the older proof");
    for (name, e) in [("alone", &alone), ("with its run", &with_run)] {
        let c = compare(e, &older, 6);
        println!("newer {name} against the proof at {}: common {:?} {:?} ours {} theirs {} {:?}", older.tip.height, c.common, c.basis, c.ours, c.theirs, c.better);
        assert_eq!((c.basis.clone(), c.better), (Basis::Count, Ordering::Greater));
    }
    let (best, _) = pick(&[older, with_run.clone(), alone], 6).unwrap();
    assert_eq!(best, 1);
}

//! The fixed-threshold count: on made-up numbers, and on demand on real mainnet headers and proofs.
use ergo_chain_types::autolykos_pow_scheme::{decode_compact_bits, order_bigint};
use ergo_chain_types::Header;
use ergo_nipopow::NipopowProof;
use num_bigint::BigUint;
use num_traits::ToPrimitive;
use std::collections::HashSet;
use tachys_verify::threshold::*;

fn q() -> BigUint { order_bigint().to_biguint().unwrap() }

#[test]
fn a_hit_is_under_the_thresholds_up_to_its_level() {
    assert_eq!(level_of_hit(&q()), Some(0));
    assert_eq!(level_of_hit(&(q() + 1u32)), None);
    assert_eq!(level_of_hit(&(q() >> 1)), Some(1));
    assert_eq!(level_of_hit(&((q() >> 1) + 1u32)), Some(0));
    assert_eq!(level_of_hit(&(q() >> 40)), Some(40));
    assert_eq!(level_of_hit(&((q() >> 40) + 1u32)), Some(39));
    // q is just under 2^256, so the smallest hit is under 255 thresholds
    assert_eq!(level_of_hit(&BigUint::from(1u32)), Some(255));
}

#[test]
fn the_score_is_the_best_threshold_with_enough_headers() {
    let levels = [5, 5, 5, 4, 4, 3];
    assert_eq!(counts(&levels).into_iter().collect::<Vec<_>>(), vec![(3, 6), (4, 5), (5, 3)]);
    let best = |k| score(&levels, k).map(|s| (s.threshold, s.count, s.value.to_u64().unwrap()));
    assert_eq!(best(3), Some((5, 3, 96)));   // 3 * 2^5 beats 5 * 2^4 = 80 and 6 * 2^3 = 48
    assert_eq!(best(4), Some((4, 5, 80)));
    assert_eq!(best(6), Some((3, 6, 48)));
    assert_eq!(best(7), None);
    assert_eq!(score(&[], 1), None);
    // one header far under every threshold counts once, like any other
    assert_eq!(score(&[200, 3, 3, 3, 3, 3], 6).map(|s| (s.threshold, s.count)), Some((3, 6)));
}

/// Real headers and proofs, on demand:
/// `TACHYS_HEADERS_JSON=a.json:b.json TACHYS_PROOF_JSON=p.json:... cargo test --test threshold -- --ignored --nocapture`
#[test]
#[ignore]
fn on_a_stretch_of_mainnet() {
    let mut headers: Vec<Header> = vec![];
    for path in std::env::var("TACHYS_HEADERS_JSON").expect("TACHYS_HEADERS_JSON").split(':') {
        headers.extend(serde_json::from_str::<Vec<Header>>(&std::fs::read_to_string(path).unwrap()).unwrap());
    }
    headers.sort_by_key(|h| h.height);
    assert!(headers.windows(2).all(|w| w[1].parent_id == w[0].id && w[1].height == w[0].height + 1), "the stretch is parent-linked");
    println!("stretch: {} headers, heights {}..{}", headers.len(), headers[0].height, headers[headers.len() - 1].height);

    // T0: every hit is under its own target
    let order = q();
    let hits: Vec<BigUint> = headers.iter().map(|h| hit(h).expect("hit")).collect();
    let diffs: Vec<BigUint> = headers.iter().map(|h| decode_compact_bits(h.n_bits).to_biguint().unwrap()).collect();
    let invalid = hits.iter().zip(&diffs).filter(|(hit, d)| **hit >= &order / *d).count();
    println!("T0 headers whose hit is not under their own target: {invalid}");
    assert_eq!(invalid, 0, "VOID: hits are not computed as the chain computes them");

    let levels: Vec<u32> = hits.iter().map(|h| level_of_hit(h).unwrap()).collect();
    let total: BigUint = diffs.iter().sum();
    let (dmin, dmax) = (diffs.iter().min().unwrap(), diffs.iter().max().unwrap());
    let lo = dmin.bits() as u32 - 1;                                   // 2^lo <= smallest difficulty
    let hi = if dmax.count_ones() == 1 { dmax.bits() as u32 - 1 } else { dmax.bits() as u32 };  // 2^hi >= largest
    println!("declared difficulty: sum {total}, smallest {dmin}, largest {dmax}; 2^{lo} <= smallest, 2^{hi} >= largest");
    let (mut t1_tested, mut t1_out, mut t2_bad) = (0, vec![], 0);
    for (nu, count) in counts(&levels) {
        if nu + 2 < lo || count == 0 { continue; }
        let value = BigUint::from(count) << nu;
        let ratio = value.to_f64().unwrap() / total.to_f64().unwrap();
        let band = 3.0 / (count as f64).sqrt();
        let mut note = String::new();
        if nu <= lo && value > total { t2_bad += 1; note.push_str("  T2 AGAINST"); }
        if nu >= hi && count >= 30 {
            t1_tested += 1;
            let sd = (ratio - 1.0) / (1.0 / (count as f64).sqrt());
            note.push_str(&format!("  T1 {:+.2} s.d.", sd));
            if (ratio - 1.0).abs() > band { t1_out.push((nu, sd)); note.push_str(" OUTSIDE"); }
        }
        println!("  threshold {nu:3}  under it {count:5}  2^nu*count/sum {ratio:.4}  band +-{band:.4}{note}");
    }
    println!("T1 thresholds tested {t1_tested}, outside three standard deviations {:?}", t1_out);
    println!("T2 thresholds at or below the smallest difficulty that overstate: {t2_bad}");
    assert_eq!(t2_bad, 0);

    // T3: the most recent k headers under each threshold are in the proof served for that sample
    for path in std::env::var("TACHYS_PROOF_JSON").expect("TACHYS_PROOF_JSON").split(':') {
        let p: NipopowProof = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let k = p.m as usize;
        let tip = p.suffix_tail.last().map_or(p.suffix_head.header.height, |h| h.height);
        let in_proof: HashSet<_> = p.prefix.iter().map(|h| h.header.id).chain(std::iter::once(p.suffix_head.header.id))
            .chain(p.suffix_tail.iter().map(|h| h.id)).collect();
        println!("T3 {}: sample {k}, tip {tip}, {} headers in the proof", path.rsplit('/').next().unwrap(), in_proof.len());
        let (mut tested, mut short) = (0, vec![]);
        for nu in hi.. {
            let under: Vec<&Header> = headers.iter().zip(&levels).filter(|(h, l)| h.height <= tip && **l >= nu).map(|(h, _)| h).collect();
            if under.len() < k { break; }
            tested += 1;
            let recent = &under[under.len() - k..];
            let missing: Vec<u32> = recent.iter().filter(|h| !in_proof.contains(&h.id)).map(|h| h.height).collect();
            println!("  threshold {nu:3}  in the stretch {:5}  the last {k} span heights {}..{}  missing from the proof {}",
                under.len(), recent[0].height, recent[k - 1].height, missing.len());
            if !missing.is_empty() { short.push((nu, missing.len())); }
        }
        println!("T3 thresholds tested {tested}, with a header missing {:?}", short);
    }
}

//! Ground truth for the size claim: real Ergo superblock levels from the deployed mainnet NiPoPoW proof
//! (vectors/mainnet-1915810.bin). Computes the node's own `max_level_of` (level = log2(T) − log2(powHit),
//! from the real Autolykos hit) for every prefix header, and checks the geometric law the succinctness
//! argument rests on — `count(level ≥ µ) ≈ |C| / 2^µ` — against real data.
//!
//! Caveat that shapes the test: the prefix is the *sampled* subchain, which over-represents high levels,
//! so the raw law does NOT hold at low µ (the sample subsamples the dense low levels). It DOES hold at the
//! top, where the proof carries every such block in the chain. So the law is asserted only on that
//! complete-sample region, and the max level is checked against ⌊log2(height)⌋.

use ergo_nipopow::{NipopowAlgos, NipopowProof};
use sigma_ser::ScorexSerializable;

fn real() -> NipopowProof {
    NipopowProof::scorex_parse_bytes(include_bytes!("../../../vectors/mainnet-1915810.bin"))
        .unwrap()
}

#[test]
fn real_superblock_levels_follow_the_geometric_law_at_the_top() {
    let p = real();
    let algos = NipopowAlgos::default();
    let tip = p.suffix_tail.last().unwrap().height; // ~1.9M
    let log2_tip = (31 - (tip.leading_zeros())) as i32; // ⌊log2(tip)⌋

    // The level function must run on every non-genesis real header.
    let mut levels: Vec<i32> = vec![];
    let non_genesis = p.prefix.iter().filter(|x| x.header.height != 1).count();
    for ph in p.prefix.iter().filter(|x| x.header.height != 1) {
        levels.push(
            algos
                .max_level_of(&ph.header)
                .expect("max_level_of runs on a real header"),
        );
    }
    assert_eq!(levels.len(), non_genesis, "every real header got a level");

    let max = *levels.iter().max().unwrap();
    let ge = |mu: i32| levels.iter().filter(|&&l| l >= mu).count();

    eprintln!(
        "tip height {tip}, ⌊log2⌋={log2_tip}; {} prefix levels",
        levels.len()
    );
    eprintln!(" µ   count(≥µ)   |C|/2^µ   obs/exp");
    for mu in 0..=max {
        let c = ge(mu);
        let exp = tip as f64 / 2f64.powi(mu);
        eprintln!("{mu:>3}   {c:>8}   {exp:>9.1}   {:.2}", c as f64 / exp);
    }

    // 1. The chain's highest superblock level equals ⌊log2(height)⌋, as geometric levels predict (a chain
    //    of N blocks has, in expectation, a single block near level log2(N)).
    assert!(
        (max - log2_tip).abs() <= 1,
        "max real level {max} should be within 1 of ⌊log2(tip)⌋={log2_tip}"
    );

    // 2. At the top, where the proof carries *every* block of that level (the sample is complete), the
    //    geometric law count(≥µ) ≈ |C|/2^µ holds within small-count (Poisson) noise. Check the top 5 levels.
    for mu in (max - 4)..=max {
        let c = ge(mu) as f64;
        let exp = tip as f64 / 2f64.powi(mu);
        assert!(
            c >= 0.35 * exp && c <= 2.5 * exp,
            "µ={mu}: observed {c} vs |C|/2^µ={exp:.1} — outside geometric tolerance"
        );
    }

    // 3. Sanity: the counts are monotone non-increasing in µ (a level-≥(µ+1) block is also level ≥µ).
    assert!((1..=max).all(|mu| ge(mu) <= ge(mu - 1)));
}

//! The Miller comparator witnesses the Gap-2b reversal the node's `bestArg` is blind to.
//!
//! The scenario is ergo#2961's: after a shared fork point, an honest branch of 40 heavy blocks (each
//! difficulty 64) against an adversarial branch of 120 trivial blocks (each difficulty 1). The honest
//! branch carries 40·64 = 2560 units of work; the adversary carries 120·1 = 120 — the honest chain is
//! >21× heavier. But the adversary has 3× the block count, so the count-weighted `bestArg` prefers it.
//!
//! - `best_arg` (the node today, NipopowAlgos.bestArg via isBetterThan): prefers the ADVERSARY. Wrong.
//! - `compare` (Miller, accumulated difficulty): prefers the HONEST chain. Right.

use num_bigint::BigInt;
use tachys_verify::miller::{compare, Blk, MillerProof, K_UNSTABLE};
use std::cmp::Ordering;

/// Superblock level of the i-th block (1-based), from the rarity of its PoW hit: half of blocks reach
/// level ≥1, a quarter level ≥2, and so on. Modelled deterministically as the trailing-zero count, which
/// gives exactly that geometric distribution and is independent of the block's difficulty — as in Miller.
fn level(i_1based: u32) -> u32 {
    i_1based.trailing_zeros()
}

/// A branch of `n` blocks each at difficulty `diff`, all children of the shared genesis (id 0), with
/// ids in a private range so the two branches share only the genesis.
fn branch(n: u32, diff: u64, id_base: u64) -> MillerProof {
    let genesis = Blk { id: 0, parent: 0, height: 0, level: 0, difficulty: BigInt::from(0) };
    let mut d = vec![genesis];
    for i in 1..=n {
        d.push(Blk {
            id: id_base + i as u64,
            parent: if i == 1 { 0 } else { id_base + (i - 1) as u64 },
            height: i,
            level: level(i),
            difficulty: BigInt::from(diff),
        });
    }
    MillerProof { d, x: vec![], omega: vec![] }
}

#[test]
fn count_and_work_disagree_and_miller_picks_work() {
    let honest = branch(40, 64, 1_000);
    let adversary = branch(120, 1, 2_000);

    // Sanity: the work gap is real and large.
    assert_eq!(honest.work_after(0), BigInt::from(40 * 64)); // 2560
    assert_eq!(adversary.work_after(0), BigInt::from(120)); //  120
    assert!(honest.work_after(0) > adversary.work_after(0));

    // The node's current metric prefers the adversary — the Gap-2b defect, reproduced.
    // Exact values (witnessed): honest 41, adversary 121 — count wins where work loses.
    assert_eq!(honest.best_arg(), BigInt::from(41));
    assert_eq!(adversary.best_arg(), BigInt::from(121));
    assert!(adversary.best_arg() > honest.best_arg());

    // Miller's comparator prefers the honest chain. (Greater = first arg is the heavier chain.)
    let (ord, _enough) = compare(&honest, &adversary, 0);
    assert_eq!(ord, Ordering::Greater, "Miller must rank the heavier-work honest chain first");

    // And it is antisymmetric: swapping the arguments flips the verdict.
    let (ord_rev, _) = compare(&adversary, &honest, 0);
    assert_eq!(ord_rev, Ordering::Less);
}

#[test]
fn lca_is_the_shared_fork_point() {
    let honest = branch(40, 64, 1_000);
    let adversary = branch(120, 1, 2_000);
    // They share only the genesis (id 0), so that is the latest common ancestor.
    assert_eq!(tachys_verify::miller::lca(&honest, &adversary), Some(0));
}

#[test]
fn k_depth_flag_reflects_post_lca_block_count() {
    let honest = branch(40, 64, 1_000);
    let adversary = branch(120, 1, 2_000);
    // Only min(40,120)=40 blocks follow the LCA, below the pinned k=512, so the "enough depth" flag is false.
    let (_ord, enough) = compare(&honest, &adversary, K_UNSTABLE);
    assert!(!enough, "40 post-LCA blocks is below k={K_UNSTABLE}");
    // With a modest depth requirement it is met.
    let (_ord, enough2) = compare(&honest, &adversary, 40);
    assert!(enough2);
}

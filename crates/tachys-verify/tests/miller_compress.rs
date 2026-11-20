//! `compress` (Algorithm 1) builds a (D,X,Ω) proof whose size grows with log|C|, not |C| — the
//! succinctness property. This is an **algorithm** unit test: the chain is synthetic because the claim is
//! asymptotic (|C| up to mainnet height), a scale no mined chain reaches. Superblock levels are drawn
//! i.i.d. geometric — exactly what real Autolykos PoW hits produce (level = trailing zeros of a uniform
//! hit) — via a seeded mixer, not the regular lattice, so the test reflects real variance. Real-level
//! ground truth comes separately from a real mainnet proof (`tests/miller_real_levels.rs`).

use num_bigint::BigInt;
use tachys_verify::miller::{compress, nipopow_level, Blk, CHI_UNCOMPRESSED, K_SAMPLING, K_UNSTABLE};

/// splitmix64 — a uniform 64-bit value from the height. Its trailing-zero count is geometric(1/2): half
/// of blocks reach level ≥1, a quarter ≥2, …, as PoW hits distribute. Deterministic, so runs reproduce.
fn mix(h: u32) -> u64 {
    let mut z = (h as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A chain of `n` blocks (plus genesis) with i.i.d. geometric superblock levels. Difficulty is a constant
/// unit; the size property is about block counts per level, not work.
fn chain(n: u32) -> Vec<Blk> {
    let mut c = Vec::with_capacity(n as usize + 1);
    c.push(Blk { id: 0, parent: 0, height: 0, level: 40, difficulty: BigInt::from(0) });
    for h in 1..=n {
        c.push(Blk {
            id: h as u64,
            parent: (h - 1) as u64,
            height: h,
            level: mix(h).trailing_zeros(),
            difficulty: BigInt::from(1u32),
        });
    }
    c
}

#[test]
fn proof_size_grows_with_log_not_length() {
    let lengths = [20_000u32, 40_000, 80_000, 160_000, 320_000];
    let mut rows = vec![];
    for &n in &lengths {
        let c = chain(n);
        let p = compress(&c, K_SAMPLING, CHI_UNCOMPRESSED, K_UNSTABLE);
        rows.push((n, p.d.len(), p.len()));
    }

    eprintln!("     |C|        |D|   |D|+χ+k");
    for (n, d, total) in &rows {
        eprintln!("{n:>9}  {d:>9}  {total:>9}");
    }

    let two_k = 2 * K_SAMPLING as usize;
    for (n, d, _total) in &rows {
        let compressible = (*n as usize).saturating_sub(CHI_UNCOMPRESSED as usize + K_UNSTABLE as usize);
        let ell = nipopow_level(compressible, K_SAMPLING) as usize;
        let levels = ell + 1; // levels 0..=ℓ
        // The sampled part stays in the paper's envelope: ≥ 2K, and ≤ ~3K per level plus top-level slack
        // (D[ℓ] keeps *all* ℓ-blocks, up to ~4K).
        assert!(*d >= two_k, "|D|={d} should be ≥ 2K={two_k}");
        assert!(
            *d <= 3 * K_SAMPLING as usize * levels + 4 * K_SAMPLING as usize,
            "|D|={d} exceeds ~3K·levels + slack; levels={levels}"
        );
    }

    // The core witness: each doubling of |C| adds at most one level's worth to |D| (~2K–3K blocks), never
    // a multiplicative growth. A linear-in-|C| proof would double each row; log grows by a constant step.
    for w in rows.windows(2) {
        let (n0, d0, _) = w[0];
        let (n1, d1, _) = w[1];
        assert_eq!(n1, n0 * 2, "lengths double");
        let increment = d1.saturating_sub(d0);
        assert!(
            increment <= 3 * K_SAMPLING as usize + two_k,
            "doubling |C| {n0}→{n1} grew |D| by {increment}, more than one level (~2K–3K): not O(log)"
        );
        assert!(d1 < d0 * 2, "|D| nearly doubled ({d0}→{d1}): that is O(|C|), not O(log|C|)");
    }

    // Succinctness is asymptotic: at the largest length the whole proof is two orders below the chain.
    let (n_big, _, total_big) = *rows.last().unwrap();
    assert!(total_big * 50 < n_big as usize, "at |C|={n_big} the proof {total_big} should be « |C|");
}

#[test]
fn compress_is_succinct_at_mainnet_height() {
    // Current mainnet height (ergo-explorer, 2026-11-16): 1,918,185.
    let c = chain(1_918_185);
    let p = compress(&c, K_SAMPLING, CHI_UNCOMPRESSED, K_UNSTABLE);
    let compressible = 1_918_185usize - (CHI_UNCOMPRESSED as usize + K_UNSTABLE as usize);
    let ell = nipopow_level(compressible, K_SAMPLING);
    eprintln!(
        "mainnet |C|=1,918,185: ℓ={ell}, |D|={}, |D|+χ+k={} (χ={}, k={})",
        p.d.len(),
        p.len(),
        p.x.len(),
        p.omega.len()
    );
    // ℓ ≈ ⌊log2(|C⋆|/416)⌋ ≈ 12 (sampled levels 0..=ℓ); proof ~10k–15k headers vs a 1.9M-block chain.
    assert!((10..=13).contains(&ell), "ℓ={ell} off expected ~12");
    assert!(p.len() < 20_000, "proof {} should be well under 20k headers", p.len());
    assert!(p.len() * 100 < 1_918_185, "proof must be two orders below the chain");
}

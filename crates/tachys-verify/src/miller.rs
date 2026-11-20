//! Prototype of the Miller et al. (CCS 2025) variable-difficulty NiPoPoW comparator and proof object.
//!
//! This is a research prototype, not used by the wallet: it realises the one piece that closes
//! Gap 2 of ergo#2961 — comparing two proofs by **accumulated difficulty**, not by superblock **count** —
//! and witnesses, as a runnable test, the count-vs-work reversal that the node's `bestArg` is blind to.
//!
//! It is written over a light `Blk` abstraction, not sigma-rust `Header`, on purpose: the comparator is
//! the intellectual content of Miller; header serialization is a separate concern (ergo#2461 / Gap 4).
//! A `Blk` carries exactly what the comparator needs — identity, ancestry, superblock level, and the
//! block's own mining difficulty (target T = decode_compact_bits(nBits), i.e. tachys `difficulty(h)`).
//!
//! Ergo constants (provisional):
//!   K (sampling) = 208   — 2K = 416 μ-blocks kept per level
//!   χ (uncompressed run) = 1152 blocks   (about nine epochs of 128; the exact run a tip needs is 1,026 to
//!                                          1,153 headers, `run::run_length_for`)
//!   k (unstable tip)     = 512 (provisional)
//!
//! Mapping to the paper: a NiPoPoW is (D, X, Ω, ℓ) — compressed samples, uncompressed recent run,
//! unstable tip, level (Algorithm 1). Compare (Algorithm 2): find the latest common ancestor at the
//! lowest shared level μ, then `highdiff` the μ-blocks after it — the side with more accumulated
//! difficulty wins. This module implements that, plus `best_arg` (the node's current count metric) so
//! the two can be set against each other.

use crate::difficulty::difficulty;
use ergo_chain_types::Header;
use ergo_nipopow::NipopowAlgos;
use num_bigint::BigInt;
use std::cmp::Ordering;
use std::collections::BTreeSet;

pub const K_SAMPLING: u32 = 208;
pub const CHI_UNCOMPRESSED: u32 = 1152;
pub const K_UNSTABLE: u32 = 512;

/// One block as the comparator sees it. `level` is its superblock level ℓ (h(b)/T ≤ 1/2^ℓ), a property
/// of how far below target the PoW hit landed — independent of the target itself. `difficulty` is the
/// block's own mining difficulty (target T), the quantity Miller accumulates.
#[derive(Debug, Clone)]
pub struct Blk {
    pub id: u64,
    pub parent: u64,
    pub height: u32,
    pub level: u32,
    pub difficulty: BigInt,
}

impl Blk {
    /// Bridge from a real Ergo header: level via the node's own `max_level_of` (the real Autolykos hit),
    /// difficulty via `decode_compact_bits(nBits)` (tachys `difficulty`). Genesis (height 1, level ∞) is
    /// capped at 40. Identity/parent are the first 8 bytes of the real id — unique within one proof.
    pub fn from_header(h: &Header, algos: &NipopowAlgos) -> Self {
        let id8 = |b: [u8; 32]| u64::from_be_bytes(b[..8].try_into().unwrap());
        let level = match algos.max_level_of(h) {
            Ok(l) if l == i32::MAX => 40, // genesis
            Ok(l) => l.max(0) as u32,
            Err(_) => 0,
        };
        Blk {
            id: id8(<[u8; 32]>::from(h.id.0)),
            parent: id8(<[u8; 32]>::from(h.parent_id.0)),
            height: h.height,
            level,
            difficulty: difficulty(h),
        }
    }
}

/// A Miller NiPoPoW: (D, X, Ω). `d` = compressed sampled subchain (≤ 2K per level), `x` = the last χ
/// uncompressed full blocks, `omega` = the last k unstable blocks. The three are a single chain,
/// genesis-ward first.
#[derive(Debug, Clone, Default)]
pub struct MillerProof {
    pub d: Vec<Blk>,
    pub x: Vec<Blk>,
    pub omega: Vec<Blk>,
}

impl MillerProof {
    /// The whole attested chain, D then X then Ω, in chain order.
    pub fn blocks(&self) -> impl Iterator<Item = &Blk> {
        self.d.iter().chain(self.x.iter()).chain(self.omega.iter())
    }

    pub fn len(&self) -> usize {
        self.d.len() + self.x.len() + self.omega.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The node's current ranking metric: `max_µ 2^µ · |C↑µ|` (NipopowAlgos.bestArg, NipopowProof.scala
    /// via isBetterThan). A pure count, weighted by level — it never looks at `difficulty`.
    pub fn best_arg(&self) -> BigInt {
        let max_level = self.blocks().map(|b| b.level).max().unwrap_or(0);
        (0..=max_level)
            .map(|mu| {
                let count = self.blocks().filter(|b| b.level >= mu).count() as u64;
                (BigInt::from(1u64) << mu) * BigInt::from(count)
            })
            .max()
            .unwrap_or_else(|| BigInt::from(0))
    }

    /// The node's `bestArg` with the superblock threshold `m`, mirroring sigma-rust
    /// `NipopowAlgos::best_arg` exactly: level 0 always counts (2^0·|C|), then climb while a level still
    /// has ≥ m blocks, taking `max_µ 2^µ·|C↑µ|`. Used to cross-check this prototype against the reference
    /// on a real proof.
    pub fn best_arg_m(&self, m: u32) -> BigInt {
        let max_level = self.blocks().map(|b| b.level).max().unwrap_or(0);
        let mut best = BigInt::from(self.blocks().count());
        let mut mu = 1u32;
        while mu <= max_level {
            let count = self.blocks().filter(|b| b.level >= mu).count();
            if (count as u32) < m {
                break;
            }
            let val = (BigInt::from(1u64) << mu) * BigInt::from(count);
            if val > best {
                best = val;
            }
            mu += 1;
        }
        best
    }

    /// The accumulated difficulty of the blocks strictly after `anchor_id` in this proof — Miller's
    /// quantity. (Sum of each block's own target T.)
    pub fn work_after(&self, anchor_id: u64) -> BigInt {
        let mut seen_anchor = false;
        let mut acc = BigInt::from(0);
        for b in self.blocks() {
            if seen_anchor {
                acc += &b.difficulty;
            }
            if b.id == anchor_id {
                seen_anchor = true;
            }
        }
        acc
    }
}

/// Miller `Compress` (Algorithm 1): sample a full chain (genesis-first, height-ordered) into a
/// (D, X, Ω) NiPoPoW. Ω = the last `k_unstable` blocks; X = the `chi` blocks before Ω; C⋆ = the rest,
/// which is compressed by keeping, at each level µ ≤ ℓ, the 2K most recent µ-blocks plus every µ-block
/// after the pivot b⋆ = C⋆↑^(µ+1)[−K] (so consecutive levels overlap in ≥ K blocks). ℓ = max level with
/// ≥ 2K blocks. This keeps O(K·log|C|) sampled blocks — the succinctness property.
pub fn compress(chain: &[Blk], k_sampling: u32, chi: u32, k_unstable: u32) -> MillerProof {
    let n = chain.len();
    let k = k_sampling as usize;
    let two_k = 2 * k;
    let (chi, ku) = (chi as usize, k_unstable as usize);

    let omega_start = n.saturating_sub(ku);
    let omega = chain[omega_start..].to_vec();
    let x_start = omega_start.saturating_sub(chi);
    let x = chain[x_start..omega_start].to_vec();
    let cstar = &chain[..x_start];

    // Too short to compress: D[0] = all of C⋆.
    if cstar.len() < two_k {
        return MillerProof { d: cstar.to_vec(), x, omega };
    }

    let up = |mu: u32| -> Vec<&Blk> { cstar.iter().filter(|b| b.level >= mu).collect() };
    let max_level = cstar.iter().map(|b| b.level).max().unwrap_or(0);
    // ℓ = max µ : |C⋆↑µ| ≥ 2K.
    let ell = (0..=max_level)
        .rev()
        .find(|&mu| up(mu).len() >= two_k)
        .unwrap_or(0);

    let mut keep: BTreeSet<u64> = BTreeSet::new();
    // D[ℓ] = all the ℓ-blocks.
    for b in up(ell) {
        keep.insert(b.id);
    }
    // Lower levels: 2K most recent µ-blocks ∪ µ-blocks after the pivot.
    for mu in (0..ell).rev() {
        let up_mu = up(mu);
        let start = up_mu.len().saturating_sub(two_k);
        for b in &up_mu[start..] {
            keep.insert(b.id);
        }
        let up1 = up(mu + 1);
        if up1.len() >= k {
            let pivot_h = up1[up1.len() - k].height;
            for b in up_mu.iter().filter(|b| b.height >= pivot_h) {
                keep.insert(b.id);
            }
        } else {
            for b in &up_mu {
                keep.insert(b.id);
            }
        }
    }
    let d: Vec<Blk> = cstar.iter().filter(|b| keep.contains(&b.id)).cloned().collect();
    MillerProof { d, x, omega }
}

/// The NiPoPoW level ℓ of a chain of `n` compressible blocks with 2K per level: ⌊log2(n / 2K)⌋.
pub fn nipopow_level(compressible_len: usize, k_sampling: u32) -> u32 {
    let two_k = 2 * k_sampling as usize;
    if compressible_len < two_k {
        return 0;
    }
    usize::BITS - 1 - (compressible_len / two_k).leading_zeros()
}

/// The latest common ancestor of two proofs: the highest-height block id they both contain.
/// (Paper: the LCA block b at the lowest shared level μ; for chains that share a prefix this is the
/// last shared block.)
pub fn lca(a: &MillerProof, b: &MillerProof) -> Option<u64> {
    let bs: BTreeSet<u64> = b.blocks().map(|x| x.id).collect();
    a.blocks()
        .filter(|x| bs.contains(&x.id))
        .max_by_key(|x| x.height)
        .map(|x| x.id)
}

/// Miller `Compare` (Algorithm 2), prototype core: order two proofs by the accumulated difficulty each
/// carries after their latest common ancestor. `Greater` means `a` is the better (heavier) chain.
/// `k` is the minimum number of post-LCA blocks the comparison needs to be meaningful; the prototype
/// records whether it was met rather than blocking.
pub fn compare(a: &MillerProof, b: &MillerProof, k: u32) -> (Ordering, bool) {
    let anchor = lca(a, b);
    let (wa, wb) = match anchor {
        Some(id) => (a.work_after(id), b.work_after(id)),
        // No common ancestor at all: compare whole-proof accumulated difficulty (the Sync-handshake case
        // degenerates to this when neither shares a block).
        None => (
            a.blocks().map(|x| &x.difficulty).sum(),
            b.blocks().map(|x| &x.difficulty).sum(),
        ),
    };
    let after = anchor
        .map(|id| {
            let ca = a.blocks().skip_while(|x| x.id != id).count().saturating_sub(1);
            let cb = b.blocks().skip_while(|x| x.id != id).count().saturating_sub(1);
            ca.min(cb)
        })
        .unwrap_or_else(|| a.len().min(b.len()));
    (wa.cmp(&wb), after as u32 >= k)
}

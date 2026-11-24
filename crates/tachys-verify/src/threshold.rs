//! Work counted by hits under one fixed threshold, whatever difficulty a header declares.
//!
//! A header is under threshold `nu` when `hit * 2^nu <= q`, with `q` the group order the proof of work is
//! defined over. Finding such a header takes `2^nu` hash computations on average, and a header that is
//! valid for a difficulty of at least `2^nu` is always under it. So for any set of headers, `2^nu` times the
//! number under `nu` estimates the work behind them, and no header counts for more than any other. Integers
//! only: the same answer on every machine.
use ergo_chain_types::autolykos_pow_scheme::order_bigint;
use ergo_chain_types::Header;
use num_bigint::BigUint;
use std::collections::BTreeMap;

/// The proof-of-work hit of a header: the lower, the more work it took to find.
pub fn hit(h: &Header) -> Option<BigUint> {
    crate::pow::pow_hit(h).ok()
}

fn order() -> BigUint {
    order_bigint()
        .to_biguint()
        .expect("the group order is positive")
}

/// The highest threshold a hit is under: the largest `nu` with `hit * 2^nu <= q`. `None` for a hit above `q`.
pub fn level_of_hit(hit: &BigUint) -> Option<u32> {
    let q = order();
    if *hit > q {
        return None;
    }
    if hit.bits() == 0 {
        return Some(q.bits() as u32);
    }
    let nu = (q.bits() - hit.bits()) as u32;
    Some(if (hit << nu) <= q { nu } else { nu - 1 })
}

/// The highest threshold a header is under.
pub fn level(h: &Header) -> Option<u32> {
    hit(h).as_ref().and_then(level_of_hit)
}

/// A set of headers scored at one threshold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Score {
    pub threshold: u32,
    /// How many of the headers are under the threshold.
    pub count: usize,
    /// `2^threshold * count`.
    pub value: BigUint,
}

/// How many of `levels` are under each threshold from the lowest level present to the highest.
pub fn counts(levels: &[u32]) -> BTreeMap<u32, usize> {
    let (Some(lo), Some(hi)) = (levels.iter().min().copied(), levels.iter().max().copied()) else {
        return BTreeMap::new();
    };
    (lo..=hi)
        .map(|nu| (nu, levels.iter().filter(|l| **l >= nu).count()))
        .collect()
}

/// The largest `2^nu * count` over the thresholds with at least `k` headers under them; `None` when no
/// threshold has `k`. `levels` holds, for each header, the highest threshold it is under.
pub fn score(levels: &[u32], k: usize) -> Option<Score> {
    counts(levels)
        .into_iter()
        .filter(|(_, count)| *count >= k.max(1))
        .map(|(threshold, count)| Score {
            threshold,
            count,
            value: BigUint::from(count) << threshold,
        })
        .max_by(|a, b| a.value.cmp(&b.value).then(b.threshold.cmp(&a.threshold)))
}

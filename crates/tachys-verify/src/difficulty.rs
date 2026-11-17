//! Port of the node's `DifficultyAdjustment` (EIP-37 rule), integer for integer.
use ergo_chain_types::autolykos_pow_scheme::{decode_compact_bits, encode_compact_bits};
use ergo_chain_types::Header;
use num_bigint::BigInt;
use num_traits::One;

pub const PRECISION: i64 = 1_000_000_000;

pub struct ChainParams {
    pub epoch_length: u32,
    pub use_last_epochs: u32,
    pub block_interval_ms: u64,
    pub v2_activation_height: u32,
    pub initial_difficulty: BigInt,
    /// `None` where EIP-37 gives a header's difficulty: mainnet, since height 844,673. The other networks keep the
    /// rule from before it (the node's `calculate`), which has no limit on one change, and give this difficulty to
    /// the two headers at which version 2 starts.
    pub earlier_rule: Option<BigInt>,
}

impl ChainParams {
    pub fn mainnet() -> Self {
        ChainParams {
            epoch_length: 128,
            use_last_epochs: 8,
            block_interval_ms: 120_000,
            v2_activation_height: 417_792,
            initial_difficulty: BigInt::from(1_199_990_374_400u64),
            earlier_rule: None,
        }
    }
}

pub fn difficulty(h: &Header) -> BigInt { decode_compact_bits(h.n_bits) }

fn normalize(d: &BigInt) -> BigInt { decode_compact_bits(encode_compact_bits(d) as u32) }

pub fn next_recalculation_height(height: u32, epoch: u32) -> u32 {
    if height.is_multiple_of(epoch) { height + 1 } else { (height / epoch + 1) * epoch + 1 }
}

/// Heights whose headers are needed to compute the difficulty of `height`.
pub fn previous_heights_required(height: u32, epoch: u32, use_last_epochs: u32) -> Vec<u32> {
    let h = height as i64 - 1;
    let e = epoch as i64;
    if h % e == 0 && (epoch > 1 || height > epoch * use_last_epochs) {
        let mut v: Vec<u32> = (0..=use_last_epochs as i64).map(|i| h - i * e).filter(|x| *x >= 0).map(|x| x as u32).collect();
        v.reverse();
        v
    } else {
        vec![height - 1]
    }
}

pub fn heights_for_next_recalculation(height: u32, p: &ChainParams) -> Vec<u32> {
    previous_heights_required(next_recalculation_height(height, p.epoch_length), p.epoch_length, p.use_last_epochs)
}

fn interval_diff(start: &Header, end: &Header, p: &ChainParams) -> Option<BigInt> {
    let dt = end.timestamp as i128 - start.timestamp as i128;
    if dt == 0 { return None }
    Some(difficulty(end) * BigInt::from(p.block_interval_ms) * BigInt::from(p.epoch_length) / BigInt::from(dt))
}

fn interpolate(data: &[(u32, BigInt)], epoch: u32) -> Option<BigInt> {
    if data.len() == 1 { return Some(data[0].1.clone()) }
    let n = BigInt::from(data.len());
    let prec = BigInt::from(PRECISION);
    let (mut xy, mut x, mut x2, mut y) = (BigInt::from(0), BigInt::from(0), BigInt::from(0), BigInt::from(0));
    for (h, d) in data {
        let hb = BigInt::from(*h);
        xy += &hb * d; x += &hb; x2 += &hb * &hb; y += d;
    }
    let den = &x2 * &n - &x * &x;
    if den == BigInt::from(0) { return None }
    let b = (&xy * &n - &x * &y) * &prec / den;
    let a = (&y * &prec - &b * &x) / &n / &prec;
    let point = BigInt::from(data.iter().map(|d| d.0).max()? + epoch);
    Some(a + b * point / prec)
}

/// The node's `calculate` (predictive rule). `hs` are the epoch-boundary headers, oldest first.
fn predictive(hs: &[Header], p: &ChainParams) -> Option<BigInt> {
    let d = if hs.len() == 1 || hs[0].timestamp >= hs[hs.len() - 1].timestamp {
        difficulty(&hs[0])
    } else {
        let mut data = vec![];
        for w in hs.windows(2) {
            if w[1].height - w[0].height != p.epoch_length { return None }
            data.push((w[1].height, interval_diff(&w[0], &w[1], p)?));
        }
        let d = interpolate(&data, p.epoch_length)?;
        if d >= BigInt::one() { d } else { p.initial_difficulty.clone() }
    };
    Some(normalize(&d))
}

/// The difficulty of the block after `hs.last()` by the rule this network uses. `hs` are the epoch-boundary
/// headers the rule reads, oldest first.
pub fn required_after(hs: &[Header], p: &ChainParams) -> Option<BigInt> {
    if p.earlier_rule.is_some() { predictive(hs, p) } else { eip37_calculate(hs, p) }
}

/// The node's `eip37Calculate`: difficulty for the block after `hs.last()`.
pub fn eip37_calculate(hs: &[Header], p: &ChainParams) -> Option<BigInt> {
    if hs.len() < 2 { return None }
    let last = difficulty(&hs[hs.len() - 1]);
    let hi: BigInt = &last * 3 / 2;
    let lo: BigInt = &last / 2;
    let pred = predictive(hs, p)?;
    let limited = if pred > last { pred.min(hi.clone()) } else { pred.max(lo.clone()) };
    let classic = interval_diff(&hs[hs.len() - 2], &hs[hs.len() - 1], p)?;
    let avg: BigInt = (classic + limited) / 2;
    let out = if avg > last { avg.min(hi) } else { avg.max(lo) };
    Some(normalize(&out))
}

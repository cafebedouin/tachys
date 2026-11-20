//! A recent stretch of headers, each the parent of the next, with its difficulty recomputed.
//!
//! A proof's sampled headers say what difficulty they were mined at; nothing in the proof says that was the
//! difficulty the chain's rule gave. In a run of consecutive headers it can be recomputed: inside an epoch a
//! header has its parent's difficulty, and the first header of an epoch has the one the rule derives from
//! the nine epoch-boundary headers before it. How far back the run must reach depends on the tip's place in
//! its epoch (`run_length_for`): on mainnet 1,026 headers when the tip opens an epoch, 1,153 when it closes one.
use crate::difficulty::*;
use ergo_chain_types::Header;
use num_bigint::BigInt;
use std::collections::BTreeMap;

/// How a header's declared difficulty stands against the rule, as far as the run can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Declared {
    /// First header of an epoch, recomputed from nine boundary headers that are all in the run: equal.
    Recomputed,
    /// Inside an epoch: equal to its parent's.
    AsParent,
    /// First header of an epoch whose nine inputs reach back before the run: not recomputed, and within the
    /// bounds the rule puts on any one change (at most half as much again, at least half).
    WithinBounds,
    /// First header of an epoch on a network that keeps the earlier rule, with inputs before the run: that rule
    /// puts no limit on one change, so there is nothing to check it against.
    Unchecked,
    /// The first header of the run: nothing to compare with.
    Start,
    /// Not what the rule gives.
    Wrong,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    pub first_height: u32,
    pub last_height: u32,
    /// Headers by standing: recomputed, as parent, within bounds, wrong.
    pub recomputed: usize,
    pub as_parent: usize,
    pub within_bounds: usize,
    pub unchecked: usize,
    pub wrong: Vec<u32>,
    /// From this height on, every epoch's difficulty was recomputed from headers of the run (`None`: no epoch
    /// start was recomputed).
    pub exact_from: Option<u32>,
    /// Sum of the declared difficulty of the whole run, and of the part from `exact_from`.
    pub work: BigInt,
    pub work_exact: BigInt,
}

/// How many headers, ending at a tip of this height, a run must hold for the tip's own epoch to be recomputed
/// from the run: back to the ninth epoch boundary below the epoch's first header. Height 0 (genesis, or a header
/// claiming it) is read as height 1, not an overflow.
pub fn run_length_for(tip_height: u32, p: &ChainParams) -> u32 {
    let epoch_start = (tip_height.saturating_sub(1) / p.epoch_length) * p.epoch_length + 1;
    tip_height.saturating_sub(epoch_start) + 2 + p.use_last_epochs * p.epoch_length
}

/// `None` when the headers are not consecutive by height and parent id.
pub fn check_run(headers: &[Header], p: &ChainParams) -> Option<RunReport> {
    let (first, last) = (headers.first()?, headers.last()?);
    if headers.windows(2).any(|w| w[1].parent_id != w[0].id || w[1].height != w[0].height + 1) { return None; }
    let by_height: BTreeMap<u32, &Header> = headers.iter().map(|h| (h.height, h)).collect();
    let mut r = RunReport { first_height: first.height, last_height: last.height, recomputed: 0, as_parent: 0, within_bounds: 0, unchecked: 0,
        wrong: vec![], exact_from: None, work: headers.iter().map(difficulty).sum(), work_exact: BigInt::from(0) };
    for w in headers.windows(2) {
        let (parent, h) = (&w[0], &w[1]);
        // there is no header at height 0: near the start of a chain the earlier rule reads the boundaries that exist
        let inputs: Vec<u32> = previous_heights_required(h.height, p.epoch_length, p.use_last_epochs).into_iter().filter(|x| *x >= 1).collect();
        let enough = if p.earlier_rule.is_some() { !inputs.is_empty() } else { inputs.len() == p.use_last_epochs as usize + 1 };
        let at_v2 = parent.height == p.v2_activation_height || parent.height + 1 == p.v2_activation_height;
        let standing = if let Some(d) = p.earlier_rule.as_ref().filter(|_| at_v2) {
            if difficulty(h) == *d { Declared::Recomputed } else { Declared::Wrong }
        } else if inputs.len() == 1 && inputs[0] + 1 == h.height && !(h.height - 1).is_multiple_of(p.epoch_length) {
            if h.n_bits == parent.n_bits { Declared::AsParent } else { Declared::Wrong }
        } else if enough && inputs.iter().all(|x| by_height.contains_key(x)) {
            let hs: Vec<Header> = inputs.iter().map(|x| (*by_height[x]).clone()).collect();
            if required_after(&hs, p) == Some(difficulty(h)) { Declared::Recomputed } else { Declared::Wrong }
        } else if p.earlier_rule.is_some() {
            Declared::Unchecked
        } else {
            let (d, dp) = (difficulty(h), difficulty(parent));
            let lo = ergo_chain_types::autolykos_pow_scheme::decode_compact_bits(
                ergo_chain_types::autolykos_pow_scheme::encode_compact_bits(&(&dp / 2)) as u32);
            if d > &dp * 3 / 2 || d < lo { Declared::Wrong } else { Declared::WithinBounds }
        };
        match standing {
            Declared::Recomputed => { r.recomputed += 1; if r.exact_from.is_none() { r.exact_from = Some(h.height); } }
            Declared::AsParent => r.as_parent += 1,
            Declared::WithinBounds => { r.within_bounds += 1; r.exact_from = None; }
            Declared::Unchecked => { r.unchecked += 1; r.exact_from = None; }
            Declared::Wrong => { r.wrong.push(h.height); r.exact_from = None; }
            Declared::Start => {}
        }
    }
    if let Some(from) = r.exact_from {
        r.work_exact = headers.iter().filter(|h| h.height >= from).map(difficulty).sum();
    }
    Some(r)
}

//! What one peer's answers amount to, and which of two peers showed more work.
//!
//! A peer gives a proof, the Extension sections asked of it for headers its proof left unbound, and a run of
//! consecutive headers that starts at a header of the proof and ends at its tip. `assemble` keeps what those
//! answers tie to the tip and refuses the rest. `compare` weighs two such results above the last header they
//! share: by summed difficulty where both hold every header in a run, otherwise by a count of headers under
//! one fixed threshold.
use crate::difficulty::{difficulty, ChainParams};
use crate::run::{check_run, RunReport};
use crate::{reach_with, threshold};
use ergo_chain_types::{BlockId, Header};
use ergo_nipopow::NipopowProof;
use num_bigint::BigInt;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

/// The headers one peer tied to its tip, after the checks that need nothing but that peer's answers.
#[derive(Debug, Clone)]
pub struct Evidence {
    /// Every header tied to the tip, by height: the proof's headers reached from its suffix head, its suffix,
    /// and the run.
    pub tied: BTreeMap<u32, Header>,
    pub tip: Header,
    /// The report on the run; `None` when no run was fetched.
    pub run: Option<RunReport>,
    /// Proof headers on the path whose interlinks stayed unbound: followed by parent id only.
    pub unbound_on_path: Vec<u32>,
    /// Proof headers that nothing reached points at. They are not tied and not counted.
    pub unreached: Vec<u32>,
    /// Tied headers from before version 2, whose work this build cannot check. They are never counted.
    pub work_unchecked: usize,
    /// The height from which a header's work was checked (where version 2 starts). A header below it is tied
    /// but is neither counted nor summed: its hit is only what the sender wrote.
    pub work_checked_from: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The proof's headers are not in order of height, or its suffix is not a chain; the height where it breaks.
    ProofShape(u32),
    /// A header's hit is not under the target it declares.
    InvalidWork(u32),
    /// The run's headers are not consecutive by height and parent id.
    NotOneRun,
    /// Headers of the run declare a difficulty the chain's rule does not give.
    WrongDifficulty(Vec<u32>),
    /// The run does not start at the child of a header the proof ties to its tip.
    RunNotTied,
    /// At this height the run and the proof name different headers.
    RunLeavesProof(u32),
}

/// `settled` names the proof headers whose interlinks an Extension section showed to be the committed ones.
/// `run` is empty when none was fetched.
pub fn assemble(proof: &NipopowProof, settled: &HashSet<BlockId>, run: &[Header], p: &ChainParams) -> Result<Evidence, Refused> {
    let suffix: Vec<&Header> = std::iter::once(&proof.suffix_head.header).chain(proof.suffix_tail.iter()).collect();
    let all: Vec<&Header> = proof.prefix.iter().map(|x| &x.header).chain(suffix.iter().copied()).collect();
    if let Some(w) = all.windows(2).find(|w| w[0].height >= w[1].height) { return Err(Refused::ProofShape(w[1].height)) }
    if let Some(w) = suffix.windows(2).find(|w| w[1].parent_id != w[0].id || w[1].height != w[0].height + 1) {
        return Err(Refused::ProofShape(w[1].height));
    }
    let before_v2 = |h: &Header| h.height < p.v2_activation_height;
    let work_ok = |h: &Header| before_v2(h) || (h.version != 1 && matches!(h.check_pow(), Ok(true)));
    if let Some(h) = all.iter().copied().chain(run.iter()).find(|h| !work_ok(h)) { return Err(Refused::InvalidWork(h.height)) }

    let reach = reach_with(proof, settled);
    let reached: HashSet<u32> = reach.reached.iter().copied().collect();
    let mut tied: BTreeMap<u32, Header> = proof.prefix.iter().map(|x| &x.header).filter(|h| reached.contains(&h.height))
        .chain(suffix.iter().copied()).map(|h| (h.height, h.clone())).collect();
    let mut report = None;
    if let Some(first) = run.first() {
        let r = check_run(run, p).ok_or(Refused::NotOneRun)?;
        if !r.wrong.is_empty() { return Err(Refused::WrongDifficulty(r.wrong)) }
        if !tied.get(&first.height.wrapping_sub(1)).is_some_and(|h| h.id == first.parent_id) { return Err(Refused::RunNotTied) }
        if let Some(h) = run.iter().find(|h| tied.get(&h.height).is_some_and(|t| t.id != h.id)) { return Err(Refused::RunLeavesProof(h.height)) }
        tied.extend(run.iter().map(|h| (h.height, h.clone())));
        report = Some(r);
    }
    let tip = tied.values().next_back().cloned().ok_or(Refused::ProofShape(0))?;
    let work_unchecked = tied.values().filter(|h| before_v2(h)).count();
    Ok(Evidence { tied, tip, run: report, unbound_on_path: reach.unbound, unreached: reach.unreached, work_unchecked,
        work_checked_from: p.v2_activation_height })
}

/// How two results were weighed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Basis {
    /// One tip is on the other's chain. The longer one is ahead when the headers it adds are in its run, where
    /// their difficulty was held to the chain's rule; two results with the same tip are equal.
    SameChain,
    /// Both sides hold every header above the common one in a run, and its declared difficulty is summed.
    /// `exact` when the difficulty of every one of those headers was recomputed on both sides.
    Run { exact: bool },
    /// A side holds only sampled headers above the common one: headers under one fixed threshold are counted.
    Count,
    /// Nothing is decided: a side has fewer than `k` headers under every threshold above the common header, so its
    /// work cannot be weighed; or one result goes on from the other's tip with headers that are not in a run.
    Undecided,
    /// The two share no header.
    NoCommonHeader,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    /// Height of the last header both sides tie to their tips.
    pub common: Option<u32>,
    pub basis: Basis,
    /// What each side showed above the common header: summed difficulty, or the count's score (0 for none).
    pub ours: BigInt,
    pub theirs: BigInt,
    /// `Greater` when ours is the better.
    pub better: Ordering,
}

/// Weighs `a` against `b`. `k` is the least number of headers under a threshold for a count to stand; it must
/// not be more than the sample the proofs were asked for.
pub fn compare(a: &Evidence, b: &Evidence, k: usize) -> Comparison {
    let zero = || BigInt::from(0);
    let common = a.tied.iter().rev().find(|(h, x)| b.tied.get(*h).is_some_and(|y| y.id == x.id)).map(|(h, _)| *h);
    let Some(c) = common else {
        return Comparison { common: None, basis: Basis::NoCommonHeader, ours: zero(), theirs: zero(), better: Ordering::Equal };
    };
    let above = |e: &Evidence| -> Vec<Header> { e.tied.range(c + 1..).map(|(_, h)| h.clone()).collect() };
    let (ours, theirs) = (above(a), above(b));
    // a side holds every header above the common one when its run starts at or below the next height and ends at its tip
    let in_run = |e: &Evidence| e.run.as_ref().is_some_and(|r| r.first_height <= c + 1 && r.last_height == e.tip.height);
    if ours.is_empty() || theirs.is_empty() {
        // One tip is the common header. The result that goes on from it is ahead only if what it adds is in its
        // run: headers a proof shows above another's tip state a difficulty nobody has checked.
        let (ahead, added_in_run) = if ours.is_empty() { (theirs.len().cmp(&ours.len()).reverse(), in_run(b)) } else { (Ordering::Greater, in_run(a)) };
        return if ours.is_empty() && theirs.is_empty() || added_in_run {
            Comparison { common, basis: Basis::SameChain, ours: zero(), theirs: zero(), better: ahead }
        } else {
            Comparison { common, basis: Basis::Undecided, ours: zero(), theirs: zero(), better: Ordering::Equal }
        };
    }
    // only headers whose work was checked are weighed
    let weighed = |e: &Evidence, hs: Vec<Header>| -> Vec<Header> { hs.into_iter().filter(|h| h.height >= e.work_checked_from).collect() };
    let all_weighed = |e: &Evidence| c + 1 >= e.work_checked_from;
    let (ours, theirs) = (weighed(a, ours), weighed(b, theirs));
    if in_run(a) && in_run(b) && all_weighed(a) && all_weighed(b) {
        let sum = |hs: &[Header]| -> BigInt { hs.iter().map(difficulty).sum() };
        let exact = |e: &Evidence| e.run.as_ref().and_then(|r| r.exact_from).is_some_and(|f| f <= c + 1);
        let (x, y) = (sum(&ours), sum(&theirs));
        return Comparison { common, basis: Basis::Run { exact: exact(a) && exact(b) }, better: x.cmp(&y), ours: x, theirs: y };
    }
    let score = |hs: &[Header]| -> Option<BigInt> {
        let levels: Vec<u32> = hs.iter().filter_map(threshold::level).collect();
        threshold::score(&levels, k).map(|s| BigInt::from(s.value))
    };
    // A count stands only with k headers under one threshold. A side with fewer has shown too little for its work
    // to be weighed: it neither wins nor loses, whatever the other side shows.
    match (score(&ours), score(&theirs)) {
        (Some(x), Some(y)) => Comparison { common, basis: Basis::Count, better: x.cmp(&y), ours: x, theirs: y },
        (x, y) => Comparison { common, basis: Basis::Undecided, ours: x.unwrap_or_else(zero), theirs: y.unwrap_or_else(zero), better: Ordering::Equal },
    }
}

/// The index of a result that no other beats, and how each of the others compares with it (`Greater`: the
/// picked one is better). `None` for an empty list, and when some result shares no header with the candidate:
/// results that cannot be compared are not ranked by the order they came in.
pub fn pick(all: &[Evidence], k: usize) -> Option<(usize, Vec<Comparison>)> {
    if all.is_empty() { return None }
    let mut best = 0;
    for i in 1..all.len() {
        if compare(&all[i], &all[best], k).better == Ordering::Greater { best = i }
    }
    let against: Vec<Comparison> = all.iter().map(|e| compare(&all[best], e, k)).collect();
    if against.iter().any(|c| c.basis == Basis::NoCommonHeader) { return None }
    Some((best, against))
}

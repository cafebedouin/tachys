//! Choosing a chain tip from what peers push, by work.
//!
//! A header's height is whatever its maker wrote, so height cannot rank two tips. What a header cannot
//! fake cheaply is the difficulty it was mined at. Three rules follow.
//!
//! 1. Tips are tried heaviest first, and a tip is never agreed on while a heavier, recent tip has been
//!    heard that its chain does not contain (`candidates`, `judge`).
//! 2. The agreed tip is the newest header that a quorum of network groups have reached, not the newest
//!    header anyone pushed (`judge`).
//! 3. The least difficulty a header may claim remembers what this wallet last agreed on, loosened only
//!    as fast as the chain's own rule lets difficulty fall (`remembered_floor`).
//!
//! Every header given to these functions has already passed its own checks (work, floor, clock).
use crate::difficulty::{difficulty, ChainParams};
use ergo_chain_types::Header;
use num_bigint::BigInt;
use std::collections::BTreeSet;

/// One peer's tip, and the network group it counts for (the /16 of an IPv4 address).
#[derive(Clone)]
pub struct TipView {
    pub group: String,
    pub tip: Header,
}

/// Why a chain was not agreed on.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Fewer linked headers behind the tip than asked for.
    Short { have: usize, want: usize },
    /// Difficulty changes inside the chain somewhere other than an epoch boundary, or by more than the chain's rule allows there.
    UnsteadyDifficulty,
    /// Two headers linked by hash do not claim consecutive heights.
    NotConsecutive,
    /// A header's parent id is not the id of the header below it in the chain given.
    NotLinked,
    /// The tip is older than the release allows.
    Stale { age_ms: u64 },
    /// Too few network groups have their tip on this chain.
    NoQuorum { groups: usize, want: usize },
    /// A recent tip claiming more difficulty was heard, and it is not on this chain.
    HeavierHeard { height: u32 },
}

/// The floor that remembers. `last` is the newest header this wallet agreed on in an earlier run.
///
/// The chain's rule (EIP-37) lets difficulty fall by at most half at each epoch boundary, and it only
/// falls that far when the epoch before it ran slow. The epoch under way when `last` was mined may
/// already have run slow, so one halving may come at once. Each further halving needs a whole epoch
/// mined at the lower difficulty by a hashrate lower still, which takes at least twice as long as the
/// one before. So within `t` of wall-clock time after `last`, the honest chain can have halved at most
/// 1 + floor(log2(t / epoch + 1)) times, where `epoch` is an epoch at the target block interval. This
/// allows more halvings than that argument needs (the second is allowed after one epoch, not two), so
/// that an honest chain is not refused over timestamp slack.
pub fn remembered_floor(last: &Header, now_ms: u64, p: &ChainParams) -> BigInt {
    let epoch_ms = (p.epoch_length as u64).saturating_mul(p.block_interval_ms).max(1);
    let epochs = now_ms.saturating_sub(last.timestamp) / epoch_ms;
    let halvings = 1 + (64 - (epochs + 1).leading_zeros() as u64 - 1);
    difficulty(last) >> halvings.min(200) as usize
}

/// The pushed tips worth trying, heaviest first, then highest; each distinct header once.
pub fn candidates(views: &[TipView]) -> Vec<Header> {
    let mut seen = BTreeSet::new();
    let mut tips: Vec<Header> = views.iter().map(|v| v.tip.clone()).filter(|t| seen.insert(<[u8; 32]>::from(t.id.0))).collect();
    tips.sort_by(|a, b| difficulty(b).cmp(&difficulty(a)).then(b.height.cmp(&a.height)));
    tips
}

/// What `judge` agreed on.
#[derive(Debug, PartialEq, Eq)]
pub struct Agreement {
    /// How many of the chain's newest headers are ahead of the agreed tip: fewer groups than the quorum have
    /// reached them, so they rest on whoever pushed them. `chain[ahead]` is the agreed tip.
    pub ahead: usize,
    /// The network groups whose tip is the agreed tip or a header above it on this chain.
    pub groups: BTreeSet<String>,
}

/// Is this chain (newest first, linked, each header already checked) one to agree on, and how far up?
///
/// The agreed tip is the newest header that at least `quorum` network groups have reached: their own tip is
/// that header or one above it on this chain. A peer whose tip is an ancestor says nothing about the headers
/// above it, so one peer pushing a block of its own cannot carry the others' agreement with it. Below the
/// agreed tip there must still be `depth` linked headers; if the chain given is too short for that, `Short`
/// says how many headers are wanted in all.
///
/// `older` are ancestors of the chain's last header, fetched and hash-linked by the caller. They only explain
/// a heavier tip that turns out to be this chain's own past (see `rivals`).
#[allow(clippy::too_many_arguments)]
pub fn judge(chain: &[Header], views: &[TipView], older: &[Header], depth: usize, quorum: usize, epoch_length: u32, now_ms: u64, max_tip_age_ms: u64) -> Result<Agreement, Refusal> {
    if chain.is_empty() { return Err(Refusal::Short { have: 0, want: depth }) }
    if chain.len() < depth { return Err(Refusal::Short { have: chain.len(), want: depth }) }
    // newest first: w[0] is the child. Each header must be the next height after the one below it (a height is its
    // maker's word, and headers linked by hash alone could claim any) and name it as its parent (callers link by hash
    // first; this holds them to it), and carry the same difficulty, or at the first block of an epoch a difficulty
    // within the chain's own limits. Without the limits one header at full difficulty could stand on nine cheap ones
    // and be ranked by its own claim.
    for w in chain.windows(2) {
        if w[0].height != w[1].height.wrapping_add(1) { return Err(Refusal::NotConsecutive) }
        if w[0].parent_id != w[1].id { return Err(Refusal::NotLinked) }
    }
    if !chain.windows(2).all(|w| steps(&w[1], &w[0], epoch_length)) { return Err(Refusal::UnsteadyDifficulty) }
    // the newest header that enough groups have reached
    let at = |h: &Header| chain.iter().position(|c| c.id == h.id);
    let mut groups: BTreeSet<String> = BTreeSet::new();
    let mut ahead = None;
    for i in 0..chain.len() {
        groups.extend(views.iter().filter(|v| at(&v.tip) == Some(i)).map(|v| v.group.clone()));
        if groups.len() >= quorum { ahead = Some(i); break }
    }
    let Some(ahead) = ahead else { return Err(Refusal::NoQuorum { groups: groups.len(), want: quorum }) };
    let top = &chain[ahead];
    if chain.len() - ahead < depth { return Err(Refusal::Short { have: chain.len() - ahead, want: depth + ahead }) }
    let age = now_ms.saturating_sub(top.timestamp);
    if age > max_tip_age_ms { return Err(Refusal::Stale { age_ms: age }) }
    // A heavier tip that is on this chain, or among its fetched ancestors, is explained: it is from before difficulty fell.
    // An old tip is not evidence of a heavier chain now: a forged chain must itself be recent to be accepted.
    if let Some(v) = rivals(chain, views, older, ahead, now_ms, max_tip_age_ms).first() {
        return Err(Refusal::HeavierHeard { height: v.height });
    }
    Ok(Agreement { ahead, groups })
}

/// The recent tips that claim more difficulty than `chain[ahead]` and are neither on the chain nor among `older`.
/// While one of these stands, the chain is not agreed on. A caller may be able to explain one: if it is this
/// chain's own ancestor from before difficulty fell, fetching the headers between (each the parent of the
/// last, so linked by hash) and passing them as `older` shows it.
pub fn rivals(chain: &[Header], views: &[TipView], older: &[Header], ahead: usize, now_ms: u64, max_tip_age_ms: u64) -> Vec<Header> {
    let Some(top) = chain.get(ahead) else { return vec![] };
    let known = |h: &Header| chain.iter().chain(older.iter()).any(|c| c.id == h.id);
    let mine = difficulty(top);
    let mut seen = BTreeSet::new();
    views.iter().map(|v| &v.tip).filter(|t| !known(t) && now_ms.saturating_sub(t.timestamp) <= max_tip_age_ms && difficulty(t) > mine)
        .filter(|t| seen.insert(<[u8; 32]>::from(t.id.0))).cloned().collect()
}

/// May `child` follow `parent` on the same chain? It must be the next height and carry the same
/// difficulty, except that the first block of an epoch may carry as little as half and as much as half again (the
/// rule's own limits: `difficulty::eip37_calculate` clamps to them).
/// A header appended to an agreed chain from a single source must pass this: otherwise that source
/// could add a block mined at the floor, far below what the chain it extends was mined at.
pub fn continues(parent: &Header, child: &Header, epoch_length: u32) -> bool {
    child.height == parent.height.wrapping_add(1) && child.parent_id == parent.id && steps(parent, child, epoch_length)
}

/// Is `child`'s difficulty one that may follow `parent`'s? The same; or, where `child` is the first block of an epoch,
/// between half and one and a half times it.
fn steps(parent: &Header, child: &Header, epoch_length: u32) -> bool {
    if child.n_bits == parent.n_bits { return true }
    let (c, p) = (difficulty(child), difficulty(parent));
    child.height.saturating_sub(1).is_multiple_of(epoch_length.max(1)) && half_or_more(&c, &p) && c.clone() * 2 <= p * 3
}

/// Is `child` at least half of `parent`, as a header can state it? A header carries its difficulty in a compact form
/// (a mantissa of 16 to 23 bits) that rounds down, so the chain's largest fall, exactly half, can read a hair under
/// half. The allowance is that rounding and no more: one part in 2^15.
fn half_or_more(child: &BigInt, parent: &BigInt) -> bool { child * 2 + (parent >> 15usize) >= *parent }

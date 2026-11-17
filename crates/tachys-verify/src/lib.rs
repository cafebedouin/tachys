//! Strict single-proof checks for a light wallet, plus the rule for choosing among several peers' proofs.
//! Runs every check and reports every failure; it never stops at the first.
pub mod difficulty;
pub mod peers;
pub mod threshold;
pub mod tip;

use difficulty::*;
use ergo_chain_types::{BlockId, Header};
use ergo_nipopow::NipopowProof;
use num_bigint::BigInt;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Check { C1Params, C2Genesis, C3Heights, C4SuffixLinks, C5Work, C6TipHeight, C7Floor, C8Recency, C9DifficultyHeaders, C10DifficultyContinuity, C11InterlinkProofs }

/// What a wallet release carries.
pub struct Anchor {
    pub genesis_id: BlockId,
    pub checkpoint_height: u32,
    pub checkpoint_difficulty: BigInt,
    pub checkpoint_timestamp_ms: u64,
    /// A header above the checkpoint must have difficulty >= checkpoint_difficulty / divisor, where the
    /// divisor starts at `floor_divisor` and doubles every `floor_halving_ms` after the checkpoint
    /// (0 = never). A stale release tolerates a hashrate fall; a fresh one keeps forgery expensive.
    pub floor_divisor: u32,
    pub floor_halving_ms: u64,
    pub min_k: u32,
    pub max_tip_age_ms: u64,
    pub max_future_drift_ms: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DifficultyAssurance { Exact, Bounded, None }

#[derive(Debug)]
pub struct Report {
    pub failures: Vec<(Check, String)>,
    pub work_checked: usize,
    pub v1_skipped: usize,
    pub difficulty: DifficultyAssurance,
    pub tip: Option<Header>,
}

impl Anchor {
    pub fn floor_at(&self, now_ms: u64) -> BigInt {
        let steps = now_ms.saturating_sub(self.checkpoint_timestamp_ms).checked_div(self.floor_halving_ms).unwrap_or(0).min(60);
        &self.checkpoint_difficulty / (BigInt::from(self.floor_divisor) << steps as usize)
    }
}

impl Report {
    pub fn accepted(&self) -> bool { self.failures.is_empty() }
    pub fn failed(&self) -> Vec<Check> {
        let mut v: Vec<Check> = self.failures.iter().map(|f| f.0).collect();
        v.sort(); v.dedup(); v
    }
}

/// The node's `packInterlinks` (NipopowAlgos.scala:171): each distinct id is keyed by the index of
/// its first occurrence and carries its count over the whole vector.
pub fn pack_interlinks(links: &[BlockId]) -> Vec<([u8; 2], Vec<u8>)> {
    let mut out = vec![];
    let mut i = 0;
    while i < links.len() {
        let qty = links.iter().filter(|l| **l == links[i]).count();
        let id: Vec<u8> = links[i].0.into();
        out.push(([ergo_nipopow::INTERLINK_VECTOR_PREFIX, i as u8], std::iter::once(qty as u8).chain(id).collect()));
        i += qty;
    }
    out
}

/// Merkle root of an extension that holds these interlinks and nothing else.
pub fn interlinks_root(links: &[BlockId]) -> ergo_chain_types::Digest32 {
    let leaves: Vec<ergo_merkle_tree::MerkleNode> = pack_interlinks(links).into_iter()
        .map(|(k, v)| std::iter::once(2u8).chain(k).chain(v).collect::<Vec<u8>>())
        .map(ergo_merkle_tree::MerkleNode::from_bytes).collect();
    ergo_merkle_tree::MerkleTree::new(leaves).root_hash()
}

/// The node's `PoPowHeader.checkInterlinksProof`. In today's wire format the root is rebuilt from
/// the asserted interlinks, so this does not bind them to the header.
pub fn interlinks_proof_ok(h: &ergo_nipopow::PoPowHeader) -> bool {
    if h.interlinks.is_empty() && h.interlinks_proof.get_indices().is_empty() && h.interlinks_proof.get_proofs().is_empty() {
        return true;
    }
    h.interlinks_proof.valid(interlinks_root(&h.interlinks).as_ref())
}

/// Whether the interlinks a proof shows for a header are the ones the header commits to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    /// The genesis header, which has no interlinks.
    Genesis,
    /// The root of the interlinks shown is the header's `extension_root`: the block's extension holds exactly
    /// these interlinks and nothing else. No proof is needed to see it.
    Bound,
    /// The root differs: the extension holds other fields as well, or other interlinks. The bytes of the
    /// proof do not say which; the block's Extension section does.
    Unbound,
}

pub fn interlinks_binding(h: &ergo_nipopow::PoPowHeader) -> Binding {
    if h.header.height == 1 && h.interlinks.is_empty() {
        Binding::Genesis
    } else if !h.interlinks.is_empty() && interlinks_root(&h.interlinks) == h.header.extension_root {
        Binding::Bound
    } else {
        Binding::Unbound
    }
}

/// What a proof ties to its suffix head by links that headers commit to.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Reach {
    /// Heights of the prefix headers reached from the suffix head.
    pub reached: Vec<u32>,
    /// Reached headers whose interlinks are not bound: their parent id is followed, their interlinks are not.
    pub unbound: Vec<u32>,
    /// Prefix headers that no reached header points at. Nothing shows the tip descends from them.
    pub unreached: Vec<u32>,
}

/// Walks down from the suffix head. A prefix header is reached when a header already reached commits to it:
/// as its parent, one height below; or as an interlink, at a lower height, when that header's interlinks are
/// bound. The walk needs the prefix in order of height, which check C3 enforces.
pub fn reach(proof: &NipopowProof) -> Reach {
    use std::collections::HashMap;
    type Links = HashMap<BlockId, u32>;
    // records what a reached header commits to, and says whether its interlinks were bound
    fn follow(h: &ergo_nipopow::PoPowHeader, as_parent: &mut Links, as_interlink: &mut Links) -> Binding {
        as_parent.insert(h.header.parent_id, h.header.height);
        let binding = interlinks_binding(h);
        if binding == Binding::Bound {
            for l in &h.interlinks {
                let from = as_interlink.entry(*l).or_insert(0);
                *from = (*from).max(h.header.height);
            }
        }
        binding
    }
    let (mut as_parent, mut as_interlink) = (Links::new(), Links::new());
    let mut out = Reach::default();
    follow(&proof.suffix_head, &mut as_parent, &mut as_interlink);
    let mut prefix: Vec<&ergo_nipopow::PoPowHeader> = proof.prefix.iter().collect();
    prefix.sort_by_key(|h| std::cmp::Reverse(h.header.height));
    for h in prefix {
        let by_parent = as_parent.get(&h.header.id) == Some(&(h.header.height + 1));
        let by_interlink = as_interlink.get(&h.header.id).is_some_and(|from| *from > h.header.height);
        if by_parent || by_interlink {
            out.reached.push(h.header.height);
            if follow(h, &mut as_parent, &mut as_interlink) == Binding::Unbound {
                out.unbound.push(h.header.height);
            }
        } else {
            out.unreached.push(h.header.height);
        }
    }
    out.reached.sort(); out.unbound.sort(); out.unreached.sort();
    out
}

fn epoch_of(height: u32, p: &ChainParams) -> u32 { (height.max(1) - 1) / p.epoch_length }

/// `extra` are headers the wallet fetched to complete a difficulty recomputation; they get the same
/// work and floor checks as the proof's own headers.
pub fn verify(proof: &NipopowProof, extra: &[Header], anchor: &Anchor, p: &ChainParams, now_ms: u64) -> Report {
    let mut f: Vec<(Check, String)> = vec![];
    let suffix: Vec<&Header> = std::iter::once(&proof.suffix_head.header).chain(proof.suffix_tail.iter()).collect();
    let chain: Vec<&Header> = proof.prefix.iter().map(|x| &x.header).chain(suffix.iter().copied()).collect();
    let tip = *suffix.last().unwrap_or(&&proof.suffix_head.header);

    // C1
    if proof.m < 1 || proof.k < anchor.min_k || proof.suffix_tail.len() as u64 + 1 != proof.k as u64 {
        f.push((Check::C1Params, format!("m={} k={} suffix={}", proof.m, proof.k, suffix.len())));
    }
    // C2
    if chain[0].id != anchor.genesis_id { f.push((Check::C2Genesis, format!("first header {:?}", chain[0].id))); }
    // C3
    if let Some(w) = chain.windows(2).find(|w| w[0].height >= w[1].height) {
        f.push((Check::C3Heights, format!("{} then {}", w[0].height, w[1].height)));
    }
    // C4
    if let Some(w) = suffix.windows(2).find(|w| w[1].parent_id != w[0].id || w[1].height != w[0].height + 1) {
        f.push((Check::C4SuffixLinks, format!("at height {}", w[1].height)));
    }
    // C5, C7 over the proof's headers and the extras
    let floor = anchor.floor_at(now_ms);
    let (mut work_checked, mut v1_skipped) = (0, 0);
    for h in chain.iter().copied().chain(extra.iter()) {
        if h.height >= p.v2_activation_height && h.version != 1 {
            work_checked += 1;
            if !matches!(h.check_pow(), Ok(true)) { f.push((Check::C5Work, format!("height {}", h.height))); }
        } else if h.height >= p.v2_activation_height {
            f.push((Check::C5Work, format!("version 1 header at height {}", h.height)));
        } else {
            v1_skipped += 1;
        }
        if h.height > anchor.checkpoint_height && difficulty(h) < floor {
            f.push((Check::C7Floor, format!("height {} difficulty {}", h.height, difficulty(h))));
        }
    }
    // C6
    if tip.height < anchor.checkpoint_height {
        f.push((Check::C6TipHeight, format!("tip {} below checkpoint {}", tip.height, anchor.checkpoint_height)));
    }
    // C8
    if now_ms.saturating_sub(tip.timestamp) > anchor.max_tip_age_ms {
        f.push((Check::C8Recency, format!("tip is {} s old", (now_ms - tip.timestamp) / 1000)));
    }
    if tip.timestamp > now_ms + anchor.max_future_drift_ms {
        f.push((Check::C8Recency, format!("tip is {} s in the future", (tip.timestamp - now_ms) / 1000)));
    }
    // C9
    let by_height: BTreeMap<u32, &Header> = chain.iter().copied().chain(extra.iter()).map(|h| (h.height, h)).collect();
    let sh = proof.suffix_head.header.height;
    let needed: Vec<u32> = heights_for_next_recalculation(sh, p).into_iter().filter(|h| *h > 0 && *h < sh).collect();
    let missing: Vec<u32> = needed.iter().copied().filter(|h| !chain.iter().any(|x| x.height == *h)).collect();
    if !missing.is_empty() { f.push((Check::C9DifficultyHeaders, format!("missing heights {:?}", missing))); }
    // C10: walk boundary headers and suffix in height order
    let mut rel: Vec<&Header> = needed.iter().filter_map(|h| by_height.get(h).copied()).chain(suffix.iter().copied()).collect();
    rel.sort_by_key(|h| h.height); rel.dedup_by_key(|h| h.height);
    let mut assurance = if rel.len() >= 2 { DifficultyAssurance::Bounded } else { DifficultyAssurance::None };
    for w in rel.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (da, db) = (difficulty(a), difficulty(b));
        match epoch_of(b.height, p) - epoch_of(a.height, p).min(epoch_of(b.height, p)) {
            0 => if a.n_bits != b.n_bits {
                f.push((Check::C10DifficultyContinuity, format!("nBits change inside an epoch at {}", b.height)));
            },
            1 => {
                let lo = ergo_chain_types::autolykos_pow_scheme::decode_compact_bits(
                    ergo_chain_types::autolykos_pow_scheme::encode_compact_bits(&(&da / 2)) as u32);
                if db > &da * 3 / 2 || db < lo {
                    f.push((Check::C10DifficultyContinuity, format!("{} -> {} between {} and {}", da, db, a.height, b.height)));
                }
            }
            _ => f.push((Check::C10DifficultyContinuity, format!("gap between {} and {}", a.height, b.height))),
        }
    }
    // exact recomputation for the suffix head's epoch, when its nine inputs are all present
    let first = epoch_of(sh, p) * p.epoch_length + 1;
    let inputs: Vec<u32> = previous_heights_required(first, p.epoch_length, p.use_last_epochs);
    if inputs.len() == p.use_last_epochs as usize + 1 && inputs.iter().all(|h| by_height.contains_key(h)) {
        let hs: Vec<Header> = inputs.iter().map(|h| (*by_height[h]).clone()).collect();
        match eip37_calculate(&hs, p) {
            Some(d) if d == difficulty(&proof.suffix_head.header) => assurance = DifficultyAssurance::Exact,
            other => f.push((Check::C10DifficultyContinuity, format!("recomputed {:?}, header states {}", other, difficulty(&proof.suffix_head.header)))),
        }
    }
    // C11
    if !std::iter::once(&proof.suffix_head).chain(proof.prefix.iter()).all(interlinks_proof_ok) {
        f.push((Check::C11InterlinkProofs, "an interlink proof does not verify".into()));
    }
    Report { failures: f, work_checked, v1_skipped, difficulty: assurance, tip: Some(tip.clone()) }
}

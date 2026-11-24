//! Choosing among several peers' proofs.
//!
//! Each peer's proof has already passed the single-proof checks. Peers fetched seconds apart sit a
//! few blocks apart, so their ten-header suffixes overlap. Two views are *consistent* when they
//! share a height and agree on every shared height; they *conflict* when any shared height
//! differs. Views with no shared height are *unrelated* (not evidence either way).
use ergo_chain_types::{BlockId, Header};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub struct PeerView {
    pub peer: String,
    /// Network group used for diversity counting (the /16 of an IPv4 address).
    pub group: String,
    pub suffix: Vec<Header>,
}

pub struct Cluster {
    pub chain: BTreeMap<u32, Header>,
    pub peers: Vec<String>,
    pub groups: BTreeSet<String>,
}

impl Cluster {
    pub fn tip(&self) -> &Header {
        self.chain.values().next_back().expect("non-empty cluster")
    }
    fn relation(&self, v: &PeerView) -> Relation {
        let mut shared = 0;
        for h in &v.suffix {
            if let Some(mine) = self.chain.get(&h.height) {
                if mine.id != h.id {
                    return Relation::Conflict;
                }
                shared += 1;
            }
        }
        if shared > 0 {
            Relation::Consistent
        } else {
            Relation::Unrelated
        }
    }
}

enum Relation {
    Consistent,
    Conflict,
    Unrelated,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// One cluster reaches the quorum and nothing conflicts with it.
    Agreed,
    /// The best cluster reaches the quorum but another cluster disagrees at a shared height.
    AgreedWithDissent,
    /// No cluster reaches the quorum of distinct network groups.
    NoQuorum,
}

pub struct Decision {
    pub verdict: Verdict,
    /// Clusters, best first: by distinct network groups, then peers, then tip height.
    pub clusters: Vec<Cluster>,
    /// Heights at which two clusters name different headers.
    pub conflicts: Vec<(u32, BlockId, BlockId)>,
}

impl Decision {
    pub fn tip(&self) -> Option<&Header> {
        match self.verdict {
            Verdict::NoQuorum => None,
            _ => self.clusters.first().map(|c| c.tip()),
        }
    }
}

pub fn decide(views: &[PeerView], quorum_groups: usize) -> Decision {
    let mut clusters: Vec<Cluster> = vec![];
    // Highest tips first, so a view that bridges two lower ones is placed before them.
    let mut order: Vec<&PeerView> = views.iter().filter(|v| !v.suffix.is_empty()).collect();
    order.sort_by_key(|v| std::cmp::Reverse(v.suffix.iter().map(|h| h.height).max()));
    for v in order {
        let home = clusters
            .iter()
            .position(|c| matches!(c.relation(v), Relation::Consistent));
        let c = match home {
            Some(i) => &mut clusters[i],
            None => {
                clusters.push(Cluster {
                    chain: BTreeMap::new(),
                    peers: vec![],
                    groups: BTreeSet::new(),
                });
                clusters.last_mut().expect("just pushed")
            }
        };
        for h in &v.suffix {
            c.chain.entry(h.height).or_insert_with(|| h.clone());
        }
        c.peers.push(v.peer.clone());
        c.groups.insert(v.group.clone());
    }
    clusters.sort_by_key(|c| std::cmp::Reverse((c.groups.len(), c.peers.len(), c.tip().height)));
    let mut conflicts = vec![];
    for i in 0..clusters.len() {
        for j in i + 1..clusters.len() {
            for (h, a) in &clusters[i].chain {
                if let Some(b) = clusters[j].chain.get(h) {
                    if a.id != b.id {
                        conflicts.push((*h, a.id, b.id));
                        break;
                    }
                }
            }
        }
    }
    let verdict = match clusters.first() {
        Some(c) if c.groups.len() >= quorum_groups => {
            let best_has_conflict = clusters.iter().skip(1).any(|o| {
                o.chain
                    .iter()
                    .any(|(h, x)| clusters[0].chain.get(h).is_some_and(|m| m.id != x.id))
            });
            if best_has_conflict {
                Verdict::AgreedWithDissent
            } else {
                Verdict::Agreed
            }
        }
        _ => Verdict::NoQuorum,
    };
    Decision {
        verdict,
        clusters,
        conflicts,
    }
}

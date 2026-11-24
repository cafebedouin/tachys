//! Choosing a tip by work: real mainnet headers, and altered copies standing in for a forged chain.
//! (The altered copies carry no valid work; these functions are given headers that already passed that check.)
use ergo_chain_types::autolykos_pow_scheme::encode_compact_bits;
use ergo_chain_types::{BlockId, Digest32, Header};
use ergo_nipopow::NipopowProof;
use sigma_ser::ScorexSerializable;
use tachys_verify::difficulty::{difficulty, ChainParams};
use tachys_verify::tip::*;

const HOUR: u64 = 3_600_000;
const MAX_AGE: u64 = 3 * HOUR;

/// Ten linked mainnet headers, newest first.
fn real() -> Vec<Header> {
    let p =
        NipopowProof::scorex_parse_bytes(include_bytes!("../../../vectors/mainnet-1915810.bin"))
            .unwrap();
    let mut s: Vec<Header> = std::iter::once(p.suffix_head.header.clone())
        .chain(p.suffix_tail.iter().cloned())
        .collect();
    s.reverse();
    s
}
/// A chain of the same shape that claims `up` more height and a `1/div` of the difficulty.
fn forged(real: &[Header], up: u32, div: u32, tag: u8) -> Vec<Header> {
    let mut out: Vec<Header> = vec![];
    for (i, h) in real.iter().enumerate() {
        let mut f = h.clone();
        f.height += up;
        f.n_bits = encode_compact_bits(&(difficulty(h) / div)) as u32;
        let mut id = [tag; 32];
        id[0] = i as u8;
        f.id = BlockId(Digest32::from(id));
        out.push(f);
    }
    for i in 0..out.len() - 1 {
        out[i].parent_id = out[i + 1].id;
    }
    out
}
fn view(group: &str, tip: &Header) -> TipView {
    TipView {
        group: group.into(),
        tip: tip.clone(),
    }
}
fn now(chain: &[Header]) -> u64 {
    chain[0].timestamp + 60_000
}

#[test]
fn honest_peers_agree() {
    let c = real();
    let views = [view("1.1", &c[0]), view("2.2", &c[0]), view("3.3", &c[1])];
    assert_eq!(candidates(&views)[0].id, c[0].id);
    // two groups are at the newest header and a third is one behind: the agreed tip is the header all three have reached
    let a = judge(&c, &views, &[], 9, 3, 128, now(&c), MAX_AGE).unwrap();
    assert_eq!((a.ahead, a.groups.len()), (1, 3));
    let all = [view("1.1", &c[0]), view("2.2", &c[0]), view("3.3", &c[0])];
    assert_eq!(
        judge(&c, &all, &[], 10, 3, 128, now(&c), MAX_AGE)
            .unwrap()
            .ahead,
        0,
        "all three at the newest header: that is the tip"
    );
}

#[test]
fn one_peer_cannot_carry_the_others_to_its_own_block() {
    // one peer pushes a block of its own on the real tip, mined at the chain's difficulty; the others are at the real tip
    let c = real();
    let mut child = forged(&c, 1, 1, 5)[0].clone();
    child.parent_id = c[0].id;
    let views = [
        view("9.9", &child),
        view("1.1", &c[0]),
        view("2.2", &c[0]),
        view("3.3", &c[0]),
    ];
    assert_eq!(
        candidates(&views)[0].id,
        child.id,
        "it is tried first: same difficulty, one higher"
    );
    let chain: Vec<Header> = std::iter::once(child.clone())
        .chain(c.iter().cloned())
        .collect();
    let a = judge(&chain, &views, &[], 10, 3, 128, now(&c), MAX_AGE).unwrap();
    assert_eq!(
        a.ahead, 1,
        "its block is set aside: only its maker has reached it"
    );
    assert_eq!(chain[a.ahead].id, c[0].id);
    assert_eq!(
        a.groups.len(),
        4,
        "the groups counted are those at the agreed tip or above it"
    );
    // with only ten headers in hand the chain is one short below the agreed tip, and the refusal says how many are wanted
    assert_eq!(
        judge(&chain[..10], &views, &[], 10, 3, 128, now(&c), MAX_AGE),
        Err(Refusal::Short { have: 9, want: 11 })
    );
    // three groups pushing the same block do carry it
    let three = [
        view("9.1", &child),
        view("9.2", &child),
        view("9.3", &child),
        view("1.1", &c[0]),
    ];
    assert_eq!(
        judge(&chain, &three, &[], 10, 3, 128, now(&c), MAX_AGE)
            .unwrap()
            .ahead,
        0
    );
}

#[test]
fn a_higher_but_lighter_chain_is_not_tried_first() {
    // three network groups push a forged tip 100 blocks "above" the real one at a quarter of the difficulty
    let c = real();
    let f = forged(&c, 100, 4, 7);
    let views = [
        view("9.1", &f[0]),
        view("9.2", &f[0]),
        view("9.3", &f[0]),
        view("1.1", &c[0]),
    ];
    let order = candidates(&views);
    assert_eq!(
        order[0].id, c[0].id,
        "the heavier tip comes first, whatever heights say"
    );
    assert_eq!(order[1].id, f[0].id);
}

#[test]
fn a_lighter_chain_is_refused_while_a_heavier_tip_is_heard() {
    // the forged chain has the quorum; one honest peer is enough to stop it
    let c = real();
    let f = forged(&c, 100, 4, 7);
    let views = [
        view("9.1", &f[0]),
        view("9.2", &f[0]),
        view("9.3", &f[0]),
        view("1.1", &c[0]),
    ];
    assert_eq!(
        judge(&f, &views, &[], 10, 3, 128, now(&c), MAX_AGE),
        Err(Refusal::HeavierHeard {
            height: c[0].height
        })
    );
    // and the honest chain, heard from one group only, is not agreed on either: no tip is trusted
    assert_eq!(
        judge(&c, &views, &[], 10, 3, 128, now(&c), MAX_AGE),
        Err(Refusal::NoQuorum { groups: 1, want: 3 })
    );
}

#[test]
fn with_an_honest_quorum_the_real_tip_wins_over_a_forged_higher_one() {
    let c = real();
    let f = forged(&c, 100, 4, 7);
    let views = [
        view("9.1", &f[0]),
        view("9.2", &f[0]),
        view("9.3", &f[0]),
        view("9.4", &f[0]),
        view("1.1", &c[0]),
        view("2.2", &c[0]),
        view("3.3", &c[2]),
    ];
    let first = &candidates(&views)[0];
    assert_eq!(first.id, c[0].id);
    let a = judge(&c, &views, &[], 8, 3, 128, now(&c), MAX_AGE).unwrap();
    assert_eq!(
        (a.ahead, a.groups.len()),
        (2, 3),
        "the third honest group is two behind, so the agreed tip is there"
    );
}

#[test]
fn a_heavier_ancestor_on_the_chain_is_not_a_rival() {
    // difficulty fell at an epoch boundary: a peer one block behind holds a heavier tip, which is on the chain
    let mut c = real();
    let boundary = 1 + 128 * (c[0].height / 128);
    c[0].height = boundary; // the newest header opens an epoch
    for (i, h) in c.iter_mut().enumerate().skip(1) {
        h.height = boundary - i as u32;
    }
    c[0].n_bits = encode_compact_bits(&(difficulty(&c[1]) * 2 / 3)) as u32;
    let views = [view("1.1", &c[0]), view("2.2", &c[0]), view("3.3", &c[1])];
    assert_eq!(
        candidates(&views)[0].id,
        c[1].id,
        "the lagging, heavier tip is tried first"
    );
    assert_eq!(
        judge(&c[1..], &views, &[], 9, 3, 128, now(&c), MAX_AGE),
        Err(Refusal::NoQuorum { groups: 1, want: 3 })
    );
    let a = judge(&c, &views, &[], 9, 3, 128, now(&c), MAX_AGE).unwrap();
    assert_eq!(
        (a.ahead, a.groups.len()),
        (1, 3),
        "then the chain is agreed on, up to the header all three have reached"
    );
}

#[test]
fn an_old_heavier_tip_is_not_evidence() {
    // a peer stuck hours back, in an epoch when difficulty was higher, does not block agreement
    let c = real();
    let mut old = forged(&c, 0, 1, 3)[0].clone();
    old.height -= 300;
    old.timestamp -= 10 * HOUR;
    old.n_bits = encode_compact_bits(&(difficulty(&c[0]) * 2)) as u32;
    let views = [
        view("1.1", &c[0]),
        view("2.2", &c[0]),
        view("3.3", &c[0]),
        view("4.4", &old),
    ];
    assert!(judge(&c, &views, &[], 10, 3, 128, now(&c), MAX_AGE).is_ok());
}

#[test]
fn a_recent_heavier_tip_blocks_until_it_is_shown_to_be_an_ancestor() {
    // difficulty fell half an hour ago; one peer still pushes a header from before the fall, older than the ten held
    let c = real();
    let mut before = forged(&c, 0, 1, 3)[0].clone();
    before.height -= 15;
    before.timestamp -= HOUR / 2;
    before.n_bits = encode_compact_bits(&(difficulty(&c[0]) * 3 / 2)) as u32;
    let views = [
        view("1.1", &c[0]),
        view("2.2", &c[0]),
        view("3.3", &c[0]),
        view("4.4", &before),
    ];
    assert_eq!(
        judge(&c, &views, &[], 10, 3, 128, now(&c), MAX_AGE),
        Err(Refusal::HeavierHeard {
            height: before.height
        })
    );
    assert_eq!(rivals(&c, &views, &[], 0, now(&c), MAX_AGE).len(), 1);
    // the caller fetched the chain's ancestors back to that height and the header is among them: it is this chain's past
    assert!(judge(
        &c,
        &views,
        std::slice::from_ref(&before),
        10,
        3,
        128,
        now(&c),
        MAX_AGE
    )
    .is_ok());
}

#[test]
fn difficulty_may_change_only_at_an_epoch_boundary() {
    let mut c = real();
    c[0].n_bits = encode_compact_bits(&(difficulty(&c[1]) / 2)) as u32;
    let views = [view("1.1", &c[0]), view("2.2", &c[0]), view("3.3", &c[0])];
    assert_ne!(
        (c[0].height - 1) % 128,
        0,
        "the vector's tip does not open an epoch"
    );
    assert_eq!(
        judge(&c, &views, &[], 10, 3, 128, now(&c), MAX_AGE),
        Err(Refusal::UnsteadyDifficulty)
    );
}

#[test]
fn a_stale_or_short_chain_is_refused() {
    let c = real();
    let views = [view("1.1", &c[0]), view("2.2", &c[0]), view("3.3", &c[0])];
    assert!(matches!(
        judge(
            &c,
            &views,
            &[],
            10,
            3,
            128,
            c[0].timestamp + 4 * HOUR,
            MAX_AGE
        ),
        Err(Refusal::Stale { .. })
    ));
    assert_eq!(
        judge(&c[..6], &views, &[], 10, 3, 128, now(&c), MAX_AGE),
        Err(Refusal::Short { have: 6, want: 10 })
    );
}

#[test]
fn the_floor_remembers_and_loosens_slowly() {
    let c = real();
    let (p, d, t) = (ChainParams::mainnet(), difficulty(&c[0]), c[0].timestamp);
    let epoch = 128 * 120_000u64; // 4 h 16 min
    assert_eq!(
        remembered_floor(&c[0], t, &p),
        &d / 2,
        "at once: one halving, for an epoch already under way"
    );
    assert_eq!(remembered_floor(&c[0], t + epoch - 1, &p), &d / 2);
    assert_eq!(
        remembered_floor(&c[0], t + epoch, &p),
        &d / 4,
        "after one epoch"
    );
    assert_eq!(
        remembered_floor(&c[0], t + 3 * epoch, &p),
        &d / 8,
        "after three"
    );
    assert_eq!(
        remembered_floor(&c[0], t + 24 * HOUR, &p),
        &d / 8,
        "a day later: an eighth"
    );
    assert_eq!(
        remembered_floor(&c[0], t + 7 * 24 * HOUR, &p),
        &d / 64,
        "a week later"
    );
    assert_eq!(
        remembered_floor(&c[0], t - HOUR, &p),
        &d / 2,
        "a clock set back does not loosen it"
    );
    // a chain at a quarter of the difficulty, forged an hour after the wallet last synced, is under the floor
    let f = forged(&c, 100, 4, 7);
    assert!(difficulty(&f[0]) < remembered_floor(&c[0], t + HOUR, &p));
}

#[test]
fn a_header_added_to_an_agreed_chain_must_carry_its_difficulty() {
    let c = real();
    assert!(
        continues(&c[1], &c[0], 128),
        "the real child follows its parent"
    );
    assert!(
        !continues(&c[2], &c[0], 128),
        "a header two blocks on does not"
    );
    // a block mined on the real tip at a quarter of its difficulty: valid work for what it claims, but not this chain's
    let mut cheap = c[0].clone();
    cheap.n_bits = encode_compact_bits(&(difficulty(&c[1]) / 4)) as u32;
    assert!(!continues(&c[1], &cheap, 128));
    // at the first block of an epoch difficulty may fall, by half at most
    let (mut parent, mut child) = (c[1].clone(), c[0].clone());
    parent.height = 128 * (parent.height / 128);
    child.height = parent.height + 1;
    child.n_bits = encode_compact_bits(&(difficulty(&parent) * 2 / 3)) as u32;
    assert!(continues(&parent, &child, 128));
    child.n_bits = encode_compact_bits(&(difficulty(&parent) / 4)) as u32;
    assert!(!continues(&parent, &child, 128));
    // the chain's own largest fall: exactly half, written in the header's compact form, which rounds down. With an
    // odd mantissa the header claims a hair under half; that is the honest chain and must follow
    parent.n_bits |= 1;
    child.n_bits = encode_compact_bits(&(difficulty(&parent) / 2)) as u32;
    assert!(
        difficulty(&child) * 2 < difficulty(&parent),
        "the compact form lost the last bit"
    );
    assert!(continues(&parent, &child, 128));
    // and the allowance is no wider than that rounding
    child.n_bits = encode_compact_bits(&(difficulty(&parent) * 499 / 1000)) as u32;
    assert!(!continues(&parent, &child, 128));
}

#[test]
fn one_heavy_header_on_cheap_ones_is_not_a_heavy_chain() {
    // the forgery: nine headers at a thousandth of the difficulty, and on top one at the real chain's difficulty that
    // claims to open an epoch (so its difficulty "may change"). Ranked by its tip it looks as heavy as the real chain.
    let c = real();
    let mut f = forged(&c, 0, 1000, 7);
    let base = 128 * (f[9].height / 128 + 1);
    for (i, h) in f.iter_mut().enumerate() {
        h.height = base + 1 - i as u32;
    }
    f[0].n_bits = c[0].n_bits;
    for i in 0..f.len() - 1 {
        f[i].parent_id = f[i + 1].id;
    }
    assert_eq!(difficulty(&f[0]), difficulty(&c[0]));
    let views = [view("a", &f[0]), view("b", &f[0]), view("c", &f[0])];
    assert_eq!(
        judge(&f, &views, &[], 10, 3, 128, now(&f), MAX_AGE),
        Err(Refusal::UnsteadyDifficulty),
        "a thousandfold rise is not one the chain's rule allows"
    );
    assert!(!continues(&f[1], &f[0], 128));
    // the most the rule allows at an epoch's first block is half again; a hair more is refused, that much is taken
    f[0].n_bits = encode_compact_bits(&(difficulty(&f[1]) * 3 / 2)) as u32;
    assert!(judge(&f, &views, &[], 10, 3, 128, now(&f), MAX_AGE).is_ok());
    f[0].n_bits = encode_compact_bits(&(difficulty(&f[1]) * 16 / 10)) as u32;
    assert_eq!(
        judge(&f, &views, &[], 10, 3, 128, now(&f), MAX_AGE),
        Err(Refusal::UnsteadyDifficulty)
    );
    // headers that link by hash but skip heights (each claiming to open an epoch) are not a chain
    let mut g = forged(&c, 0, 1, 8);
    for (i, h) in g.iter_mut().enumerate() {
        h.height = base + 1 + 128 * (9 - i as u32);
    }
    let views = [view("a", &g[0]), view("b", &g[0]), view("c", &g[0])];
    assert_eq!(
        judge(&g, &views, &[], 10, 3, 128, now(&g), MAX_AGE),
        Err(Refusal::NotConsecutive)
    );
    // and the real chain is still agreed on
    let views = [view("a", &c[0]), view("b", &c[0]), view("c", &c[0])];
    assert!(judge(&c, &views, &[], 10, 3, 128, now(&c), MAX_AGE).is_ok());
}

#[test]
fn consecutive_heights_with_a_broken_parent_link_are_not_a_chain() {
    // `judge` checked heights and difficulty steps, not that each header names the one below it as its parent; its
    // callers link by hash first, and now it holds them to it
    let c = real();
    let views = [view("a", &c[0]), view("b", &c[0]), view("c", &c[0])];
    assert!(
        judge(&c, &views, &[], 10, 3, 128, now(&c), MAX_AGE).is_ok(),
        "the real chain"
    );
    for at in [0, 4, 8] {
        let mut b = c.clone();
        b[at].parent_id = BlockId(Digest32::from([0xab; 32]));
        assert_eq!(
            judge(&b, &views, &[], 10, 3, 128, now(&b), MAX_AGE),
            Err(Refusal::NotLinked),
            "link broken above {}",
            b[at + 1].height
        );
    }
}

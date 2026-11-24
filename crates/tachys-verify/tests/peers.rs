use ergo_nipopow::NipopowProof;
use sigma_ser::ScorexSerializable;
use tachys_verify::peers::*;

fn suffix() -> Vec<ergo_chain_types::Header> {
    let p =
        NipopowProof::scorex_parse_bytes(include_bytes!("../../../vectors/mainnet-1915810.bin"))
            .unwrap();
    std::iter::once(p.suffix_head.header.clone())
        .chain(p.suffix_tail.iter().cloned())
        .collect()
}
fn view(name: &str, group: &str, s: &[ergo_chain_types::Header]) -> PeerView {
    PeerView {
        peer: name.into(),
        group: group.into(),
        suffix: s.to_vec(),
    }
}

#[test]
fn overlapping_views_agree() {
    let s = suffix();
    let d = decide(
        &[
            view("a", "1.1", &s[0..8]),
            view("b", "2.2", &s[2..10]),
            view("c", "3.3", &s[1..9]),
        ],
        3,
    );
    assert_eq!(d.verdict, Verdict::Agreed);
    assert_eq!(d.clusters.len(), 1);
    assert_eq!(d.tip().unwrap().id, s[9].id);
}

#[test]
fn same_network_group_is_one_vote() {
    let s = suffix();
    let d = decide(
        &[
            view("a", "1.1", &s),
            view("b", "1.1", &s),
            view("c", "1.1", &s),
        ],
        3,
    );
    assert_eq!(d.verdict, Verdict::NoQuorum);
    assert!(d.tip().is_none());
}

#[test]
fn a_conflicting_view_is_reported_and_outvoted() {
    let s = suffix();
    // a different header claimed at an existing height: reuse a real header under the wrong height
    let mut forged = s.clone();
    forged[5] = s[4].clone();
    forged[5].height = s[5].height;
    let d = decide(
        &[
            view("a", "1.1", &s),
            view("b", "2.2", &s),
            view("c", "3.3", &s),
            view("x", "9.9", &forged[5..6]),
        ],
        3,
    );
    assert_eq!(d.verdict, Verdict::AgreedWithDissent);
    assert_eq!(d.conflicts.len(), 1);
    assert_eq!(d.conflicts[0].0, s[5].height);
    assert_eq!(d.tip().unwrap().id, s[9].id);
}

#[test]
fn disjoint_views_do_not_count_as_agreement() {
    let s = suffix();
    let d = decide(
        &[
            view("a", "1.1", &s[0..3]),
            view("b", "2.2", &s[4..7]),
            view("c", "3.3", &s[8..10]),
        ],
        2,
    );
    assert_eq!(d.verdict, Verdict::NoQuorum);
    assert_eq!(d.clusters.len(), 3);
}

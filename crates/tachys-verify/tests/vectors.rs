use ergo_chain_types::{BlockId, Digest32, Header};
use ergo_nipopow::NipopowProof;
use tachys_verify::difficulty::*;
use tachys_verify::Check::*;
use tachys_verify::*;
use sigma_ser::ScorexSerializable;

const GENESIS: &str = "b0244dfc267baca974a4caee06120321562784303a8a688976ae56170e4d175b";
fn real() -> NipopowProof { NipopowProof::scorex_parse_bytes(include_bytes!("../../../vectors/mainnet-1915810.bin")).unwrap() }
fn reparse(p: &NipopowProof) -> NipopowProof { NipopowProof::scorex_parse_bytes(&p.scorex_serialize_bytes().unwrap()).unwrap() }
fn hdr(p: &NipopowProof, h: u32) -> Header { p.prefix.iter().map(|x| &x.header).find(|x| x.height == h).unwrap().clone() }
fn anchor(p: &NipopowProof) -> Anchor {
    Anchor { genesis_id: BlockId(Digest32::try_from(GENESIS.to_string()).unwrap()), checkpoint_height: 1_914_880,
        checkpoint_difficulty: difficulty(&hdr(p, 1_914_880)), checkpoint_timestamp_ms: hdr(p, 1_914_880).timestamp, floor_divisor: 4, floor_halving_ms: 0, min_k: 10,
        max_tip_age_ms: 3 * 3600 * 1000, max_future_drift_ms: 20 * 60 * 1000 }
}
fn tip_ts(p: &NipopowProof) -> u64 { p.suffix_tail.last().unwrap().timestamp }
fn show(name: &str, r: &Report) {
    println!("{:34} accepted={} failed={:?} work_checked={} v1_skipped={} difficulty={:?}", name, r.accepted(), r.failed(), r.work_checked, r.v1_skipped, r.difficulty);
    for (c, m) in r.failures.iter().take(3) { println!("    {:?}: {}", c, m); }
}

#[test]
fn vectors() {
    let cp = ChainParams::mainnet();
    let p = real();
    let a = anchor(&p);
    let now = tip_ts(&p) + 600_000;

    let r = verify(&p, &[], &a, &cp, now); show("mainnet proof", &r);
    assert!(r.accepted()); assert_eq!(r.difficulty, DifficultyAssurance::Bounded); assert_eq!(r.v1_skipped, 3); assert_eq!(r.work_checked, 127);

    let hs: Vec<Header> = serde_json::from_str(include_str!("../../../vectors/epoch-headers.json")).unwrap();
    let r = verify(&p, &hs[..1], &a, &cp, now); show("mainnet proof + header 1914752", &r);
    assert!(r.accepted()); assert_eq!(r.difficulty, DifficultyAssurance::Exact);

    let forged = NipopowProof::scorex_parse_bytes(include_bytes!("../../../vectors/fabricated-low-difficulty.bin")).unwrap();
    let r = verify(&forged, &[], &a, &cp, now); show("fabricated low-difficulty proof", &r);
    assert!(r.failed().contains(&C6TipHeight));

    let mut t = real(); t.prefix[60].header.autolykos_solution.nonce[0] ^= 1;
    let r = verify(&reparse(&t), &[], &a, &cp, now); show("nonce flip, prefix[60]", &r);
    assert_eq!(r.failed(), vec![C5Work]);

    let mut t = real(); let l = t.suffix_tail.len() - 1; t.suffix_tail[l].autolykos_solution.nonce[0] ^= 1;
    let r = verify(&reparse(&t), &[], &a, &cp, tip_ts(&t) + 600_000); show("nonce flip, last suffix header", &r);
    assert_eq!(r.failed(), vec![C5Work]);

    let mut t = real(); t.suffix_tail.remove(4);
    let r = verify(&reparse(&t), &[], &a, &cp, now); show("suffix header removed", &r);
    assert_eq!(r.failed(), vec![C1Params, C4SuffixLinks]);

    let r = verify(&p, &[], &a, &cp, tip_ts(&p) + 6 * 3600 * 1000); show("now = tip + 6 h", &r);
    assert_eq!(r.failed(), vec![C8Recency]);

    let mut a2 = anchor(&p); a2.checkpoint_height = 2_000_000;
    let r = verify(&p, &[], &a2, &cp, now); show("checkpoint above the tip", &r);
    assert_eq!(r.failed(), vec![C6TipHeight]);

    let mut a3 = anchor(&p); a3.checkpoint_difficulty *= 8;
    let r = verify(&p, &[], &a3, &cp, now); show("anchor difficulty x8", &r);
    assert_eq!(r.failed(), vec![C7Floor]);

    let mut t = real(); let i = t.prefix.iter().position(|x| x.header.height == 1_915_776).unwrap(); t.prefix.remove(i);
    let r = verify(&reparse(&t), &[], &a, &cp, now); show("epoch header 1915776 removed", &r);
    assert!(r.failed().contains(&C9DifficultyHeaders));

    // decaying floor: with the anchor x8 the chain is under the floor now, and back above it once
    // the divisor has doubled (4 -> 8) one halving period after the checkpoint
    let mut a5 = anchor(&p); a5.checkpoint_difficulty *= 8; a5.floor_halving_ms = 3600 * 1000;
    let r = verify(&p, &[], &a5, &cp, a5.checkpoint_timestamp_ms + 3 * 3600 * 1000 + 1); show("anchor x8, floor decayed 3 steps", &r);
    assert!(!r.failed().contains(&C7Floor));

    let mut a4 = anchor(&p); a4.genesis_id = p.prefix[1].header.id;
    let r = verify(&p, &[], &a4, &cp, now); show("wrong genesis id", &r);
    assert_eq!(r.failed(), vec![C2Genesis]);
}

/// A canary for node PR #2461 (open on 2026-11-14). That change proves a header's interlinks against its whole
/// extension root; this verifier expects a tree built from the interlinks alone, which is what mainnet nodes serve
/// today. The vector is the PR's own fixture (`nipopow-full-root-mixed-nipopow-proof.json` at head 97aa0226): a
/// synthetic proof whose suffix head carries one extension field that is not an interlink.
/// Today: the bytes parse (the trailing mode byte is ignored) and the interlink check fails.
/// When this build learns the new form, this test must be changed with it, on purpose.
#[test]
fn pr2461_new_form_is_not_read_yet() {
    let bytes = include_bytes!("../../../vectors/pr2461-full-root-proof.bin");
    let p = NipopowProof::scorex_parse_bytes(bytes).expect("today's parser reads the new form's bytes");
    assert_eq!((p.m, p.k, p.prefix.len(), p.suffix_tail.len()), (1, 2, 1, 1));
    assert_eq!(p.scorex_serialize_bytes().unwrap().len(), bytes.len() - 1, "written back, it is the old form: one byte shorter");
    assert!(!interlinks_proof_ok(&p.suffix_head), "the new form's interlink proof does not verify here");
}

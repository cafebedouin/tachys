use crate::seal::phrase_problem;
use crate::util::{clip, one_line, unhex};
use crate::wallet::names;
use ergo_lib::wallet::derivation_path::DerivationPath;
use ergo_lib::wallet::ext_secret_key::ExtSecretKey;
use ergo_lib::wallet::mnemonic::Mnemonic;
use ergotree_ir::sigma_protocol::sigma_boolean::cor::Cor;
use ergotree_ir::sigma_protocol::sigma_boolean::{ProveDlog, SigmaBoolean};
use std::str::FromStr;

// the BIP-39 test vector for sixteen zero bytes of entropy
const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

fn key(i: u32) -> ProveDlog {
    let m = ExtSecretKey::derive_master(Mnemonic::to_seed(PHRASE, "")).unwrap();
    m.derive(DerivationPath::from_str(&format!("m/44'/429'/0'/0/{i}")).unwrap()).unwrap().public_image()
}

#[test]
fn a_condition_that_names_our_key_makes_the_box_ours() {
    let (mine, theirs) = (key(0), key(1));
    let keys = [mine.clone()];
    assert!(names(&SigmaBoolean::from(mine.clone()), &keys), "a box at our address");
    assert!(!names(&SigmaBoolean::from(theirs.clone()), &keys), "someone else's box");
    assert!(!names(&SigmaBoolean::TrivialProp(true), &keys), "a box anyone can spend");
    // an offer box reduces to its maker's key when the maker takes it back; inside an OR it still counts
    let either = Cor::normalized(vec![SigmaBoolean::from(theirs.clone()), SigmaBoolean::from(mine.clone())].try_into().unwrap());
    assert!(names(&either, &keys));
    let neither = Cor::normalized(vec![SigmaBoolean::from(theirs.clone()), SigmaBoolean::from(key(2))].try_into().unwrap());
    assert!(!names(&neither, &keys));
}

#[test]
fn a_mistyped_phrase_is_caught() {
    assert_eq!(phrase_problem(PHRASE), None);
    assert_eq!(phrase_problem(&format!("  {}  ", PHRASE.replace(' ', "   "))), None, "spacing does not matter");
    assert!(phrase_problem(&PHRASE.replace("about", "abandon")).unwrap().contains("checksum"));
    assert!(phrase_problem(&PHRASE.replace("about", "abuot")).unwrap().contains("not a seed-phrase word"));
    assert!(phrase_problem("abandon abandon about").unwrap().contains("this one has 3"));
    // a phrase this wallet makes passes
    let made = ergo_lib::wallet::mnemonic_generator::MnemonicGenerator::new(ergo_lib::wallet::mnemonic_generator::Language::English, 128).unwrap().generate();
    assert_eq!(phrase_problem(&made), None);
}

#[test]
fn text_from_a_node_is_never_sliced_inside_a_character() {
    assert_eq!(unhex("\u{20ac}a"), None, "four bytes, an even number, and not hex");
    assert_eq!(unhex("0aFF"), Some(vec![10, 255]));
    assert_eq!(unhex("0a\n"), None);
    assert_eq!(clip("\u{20ac}\u{20ac}\u{20ac}", 2), "\u{20ac}\u{20ac}");
    assert_eq!(one_line("refused\nbroadcast via x: accepted, y"), "refused broadcast via x: accepted, y");
}

#[test]
fn a_value_that_cannot_be_read_is_not_the_default() {
    use crate::util::{try_arg, unknown_option};
    let args = |a: &[&str]| -> Vec<String> { std::iter::once("tachys").chain(a.iter().copied()).map(String::from).collect() };
    assert_eq!(try_arg(&args(&["tips"]), "--peers", 6usize), Ok(6), "not given: the default");
    assert_eq!(try_arg(&args(&["tips", "--peers", "9"]), "--peers", 6usize), Ok(9));
    assert!(try_arg(&args(&["tips", "--peers", "lots"]), "--peers", 6usize).is_err());
    assert!(try_arg(&args(&["tips", "--wait", "1e3"]), "--wait", 6u64).is_err());
    assert!(try_arg(&args(&["tips", "--peers"]), "--peers", 6usize).is_err(), "given with nothing after it");
    // a slip in an option's name is caught, not read as "option not given"
    assert_eq!(unknown_option(&args(&["wallet", "send", "--nanoergs", "5"])), Some("--nanoergs"));
    assert_eq!(unknown_option(&args(&["wallet", "send", "--nanoerg", "5", "--to", "x", "--broadcast"])), None);
    assert_eq!(unknown_option(&args(&["pay", "--purpose", "--for lunch"])), None, "free text may start with dashes");
    assert_eq!(unknown_option(&args(&["wallet", "restore", "--mnemonic-file", "-"])), None);
}

#[test]
fn one_provider_is_one_network_group() {
    use crate::util::group_of;
    assert_eq!(group_of("198.51.100.68:9030"), "198.51");
    assert_eq!(group_of("198.51.1.2:9030"), group_of("198.51.200.9:9031"));
    assert_ne!(group_of("198.51.1.2:9030"), group_of("198.52.1.2:9030"));
    // every address of an IPv6 /32 is one group: a single machine holds a /64 and could otherwise be many
    assert_eq!(group_of("[2001:db8::1]:9030"), group_of("[2001:db8:ffff::2]:9030"));
    assert_ne!(group_of("[2001:db8::1]:9030"), group_of("[2001:db9::1]:9030"));
    assert_eq!(group_of("2001:db8::1"), group_of("[2001:db8::7]:9030"));
    // an IPv4 address written as IPv6 is the IPv4 address
    assert_eq!(group_of("[::ffff:198.51.1.2]:9030"), "198.51");
    // `tachys check` names files <ip>_<port>
    assert_eq!(group_of("198.51.100.68_9030"), "198.51");
    assert_eq!(group_of("node.example.org:9030"), "node.example");
}

#[test]
fn a_seed_list_skips_blank_lines_and_comments() {
    assert_eq!(crate::network::seed_lines("# mainnet\n\n 1.2.3.4:9030 \n#5.6.7.8:9030\n9.9.9.9:9030\n"), vec!["1.2.3.4:9030", "9.9.9.9:9030"]);
}

#[test]
fn a_proof_is_not_read_against_an_old_chain() {
    use crate::chain::fresh_enough;
    let h = crate::network::checkpoint_header();
    let limit = crate::network::release_anchor().max_tip_age_ms;
    assert!(fresh_enough(std::slice::from_ref(&h), h.timestamp + limit, limit).is_ok());
    assert!(fresh_enough(std::slice::from_ref(&h), h.timestamp + limit + 1, limit).unwrap_err().contains("run `tachys tips` again"));
    assert!(fresh_enough(std::slice::from_ref(&h), h.timestamp.saturating_sub(5_000), limit).is_ok(), "a clock a little behind the header's is not age");
    assert!(fresh_enough(&[], h.timestamp, limit).is_err(), "no chain at all");
}

/// A state tree of our own with three boxes in it, as a node's is built: each value filed under its hash.
/// Returns the root after `generate_proof`, the proof of looking up `asked`, and the three (key, value) pairs.
type Held = Vec<([u8; 32], Vec<u8>)>;
fn proven_lookup(asked: &[[u8; 32]]) -> (Vec<u8>, Vec<u8>, Held) { proven_lookup_in(3, asked) }
/// The same, over `n` boxes of varied sizes.
fn proven_lookup_in(n: usize, asked: &[[u8; 32]]) -> (Vec<u8>, Vec<u8>, Held) {
    use bytes::Bytes;
    use ergo_avltree_rust::authenticated_tree_ops::AuthenticatedTreeOps;
    use ergo_avltree_rust::batch_avl_prover::BatchAVLProver;
    use ergo_avltree_rust::batch_node::{AVLTree, Node, NodeHeader};
    use ergo_avltree_rust::operation::{KeyValue, Operation};
    let mut prover = BatchAVLProver::new(AVLTree::new(|d| Node::LabelOnly(NodeHeader::new(Some(*d), None)), 32, None), true);
    let held: Held = (1..=n).map(|i| { let v = if n == 3 { vec![i as u8; 40 + i] } else { (i as u64).to_be_bytes().repeat(5 + i % 7) }; (*sigma_util::hash::blake2b256_hash(&v), v) }).collect();
    for (k, v) in &held { prover.perform_one_operation(&Operation::Insert(KeyValue { key: Bytes::copy_from_slice(k), value: Bytes::from(v.clone()) })).unwrap(); }
    prover.generate_proof();
    let root = prover.digest().unwrap().to_vec();
    for k in asked { prover.perform_one_operation(&Operation::Lookup(Bytes::copy_from_slice(k))).unwrap(); }
    (root, prover.generate_proof().to_vec(), held)
}

#[test]
fn a_batch_proof_is_read_against_its_state_root_and_no_other() {
    use crate::proofs::lookup_all;
    let (_, _, held) = proven_lookup(&[]);
    let gone = [9u8; 32];
    let asked = [held[0].0, gone, held[2].0];
    let (root, proof, _) = proven_lookup(&asked);
    assert_eq!(lookup_all(&root, &proof, &asked), Some(vec![Some(held[0].1.clone()), None, Some(held[2].1.clone())]), "two boxes there, one proven absent");
    // another root (another block's state): no proof
    let mut other = root.clone(); other[0] ^= 1;
    assert_eq!(lookup_all(&other, &proof, &asked), None);
    // a proof for other ids does not answer for these
    assert_eq!(lookup_all(&root, &proof, &[held[1].0, gone, held[2].0]), None);
    // any byte of the proof changed: refused (or, never, a different answer)
    for i in 0..proof.len() {
        let mut bad = proof.clone(); bad[i] ^= 0x40;
        let got = lookup_all(&root, &bad, &asked);
        assert!(got.is_none() || got == lookup_all(&root, &proof, &asked), "byte {i}");
    }
    // cut short
    for n in [0, 1, proof.len() / 2, proof.len() - 1] { assert_eq!(lookup_all(&root, &proof[..n], &asked), None, "{n} bytes"); }
}

#[test]
fn a_proof_that_is_not_one_is_refused_not_fatal() {
    let key = [7u8; 32];
    for junk in [&[][..], &[0u8; 1], &[0xffu8; 64], &[3u8; 300]] {
        assert_eq!(crate::proofs::lookup_all(&[1u8; 33], junk, &[key]), None);
    }
}

#[test]
fn what_became_of_a_payment() {
    use crate::wallet::payment_outcome as outcome;
    // (settled, a box it creates exists, a box it spends is gone, enough blocks have passed)
    assert_eq!(outcome(true, true, true, false), Some("confirmed"), "its inputs are gone and its output is there");
    assert_eq!(outcome(true, true, false, false), Some("confirmed"));
    // its coins are gone and so is everything it made: the payee moved on, another wallet of this phrase spent the
    // change, or something else spent the coins. Never "failed": this wallet does not assume it is alone
    assert_eq!(outcome(true, false, true, false), Some("coins spent"));
    assert_eq!(outcome(true, false, true, true), Some("coins spent"));
    assert_eq!(outcome(true, false, false, false), None, "still waiting");
    assert_eq!(outcome(true, false, false, true), Some("not included"));
    // a proof above the tip the peers agree on settles nothing, whatever it shows
    for (c, g, u) in [(true, true, true), (false, true, false), (false, false, true), (true, false, false)] { assert_eq!(outcome(false, c, g, u), None); }
}

#[test]
fn every_option_a_command_reads_is_known_without_a_list_kept_by_hand() {
    use crate::util::{unknown_option, OPTIONS};
    // gathered from the source by build.rs: old and new alike
    for o in ["--state", "--node", "--nanoerg", "--json", "--dry-run", "--refresh", "--tamper-for-control", "--mnemonic-file", "--also-token", "--declare", "--net-file", "--network"] { assert!(OPTIONS.contains(&o), "{o}"); }
    // and nothing that only a test or a sentence mentions
    for o in ["--nanoergs", "--pears", "--for"] { assert!(!OPTIONS.contains(&o), "{o}"); }
    assert!(OPTIONS.len() > 50 && OPTIONS.windows(2).all(|w| w[0] < w[1]), "sorted, each once");
    let args: Vec<String> = ["tachys", "nodes", "--refresh", "--state", "x"].map(String::from).to_vec();
    assert_eq!(unknown_option(&args), None);
    assert_eq!(crate::util::count(0, "token"), "no tokens");
    assert_eq!(crate::util::count(1, "token"), "1 token");
    assert_eq!(crate::util::count(2, "token"), "2 tokens");
}

/// A node's REST port on this machine: answers each path with what `answer` gives for it (None: 404).
fn rest_node(answer: impl Fn(&str) -> Option<String> + Send + 'static) -> String {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { continue };
            let mut buf = [0u8; 8192];
            let n = c.read(&mut buf).unwrap_or(0);
            let path = String::from_utf8_lossy(&buf[..n]).split_whitespace().nth(1).unwrap_or("").to_string();
            let (status, body) = match answer(&path) { Some(b) => (200, b), None => (404, "{}".to_string()) };
            let _ = c.write_all(format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
        }
    });
    url
}

#[test]
fn a_merkle_proof_leads_from_our_leaf_to_the_root_or_it_is_not_one() {
    use crate::proofs::merkle_ok;
    use ergo_merkle_tree::{MerkleNode, MerkleTree, NodeSide};
    // a block of five transactions, as a node builds the tree
    let ids: Vec<[u8; 32]> = (1u8..=5).map(|i| [i; 32]).collect();
    let tree = MerkleTree::new(ids.iter().map(|i| MerkleNode::from_bytes(i.to_vec())).collect::<Vec<_>>());
    let root = tree.root_hash();
    let levels = |i: usize| -> Vec<(String, u64)> {
        let p: serde_json::Value = serde_json::to_value(tree.proof_by_index(i).unwrap()).unwrap();
        p["levels"].as_array().unwrap().iter().map(|l| (l[0].as_str().unwrap_or("").to_string(), l[1].as_u64().unwrap())).collect()
    };
    for (i, id) in ids.iter().enumerate() { assert!(merkle_ok(id, &levels(i), root.as_ref()), "transaction {i} is in the block"); }
    // the proof of one transaction does not show another, and one not in the block has no proof
    assert!(!merkle_ok(&ids[1], &levels(0), root.as_ref()));
    assert!(!merkle_ok(&[9u8; 32], &levels(0), root.as_ref()));
    // another block's root; a level changed, dropped, or turned to the other side
    assert!(!merkle_ok(&ids[0], &levels(0), &[7u8; 32]));
    let mut l = levels(2); l[0].0 = crate::util::hex(&[8u8; 32]); assert!(!merkle_ok(&ids[2], &l, root.as_ref()));
    let mut l = levels(2); l.pop(); assert!(!merkle_ok(&ids[2], &l, root.as_ref()));
    let mut l = levels(2); l[0].1 ^= 1; assert!(!merkle_ok(&ids[2], &l, root.as_ref()));
    // not hex, a side that is neither, more levels than any tree has: refused, not a panic
    let mut l = levels(2); l[0].0 = "zz".into(); assert!(!merkle_ok(&ids[2], &l, root.as_ref()));
    let mut l = levels(2); l[0].1 = 2; assert!(!merkle_ok(&ids[2], &l, root.as_ref()));
    assert!(!merkle_ok(&ids[2], &vec![(String::new(), NodeSide::Left as u64); 65], root.as_ref()));
    // an inner node passed off as a transaction: the leaf and inner hashes are made differently, so it does not check
    let inner = <[u8; 32]>::try_from(crate::util::unhex(&levels(0)[1].0).unwrap()).unwrap();
    assert!(!merkle_ok(&inner, &levels(0)[2..], root.as_ref()));
}

#[test]
fn a_block_a_node_names_must_link_back_to_the_agreed_chain_by_hash() {
    use crate::chain::id32;
    use crate::proofs::{included_in, RestCost};
    use crate::util::hex;
    use ergo_chain_types::Header;
    use sigma_ser::ScorexSerializable;
    // ten linked mainnet headers, newest first; the wallet holds the newest three, a payment is said to be in the seventh
    let p = ergo_nipopow::NipopowProof::scorex_parse_bytes(include_bytes!("../../../vectors/mainnet-1915810.bin")).unwrap();
    let mut all: Vec<Header> = std::iter::once(p.suffix_head.header.clone()).chain(p.suffix_tail.iter().cloned()).collect();
    all.sort_by_key(|h| std::cmp::Reverse(h.height));
    let (stored, block) = (all[..3].to_vec(), all[6].clone());
    let tx = "aa".repeat(32);
    // as a node writes a version-2 header's solution (this library writes nulls there, which it cannot read back)
    let json = |hs: &[Header]| serde_json::to_string(hs).unwrap().replace("\"w\":null", "\"w\":\"0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798\"").replace("\"d\":null", "\"d\":0");
    // what each node serves: the block it names, the headers it gives for a slice of heights, the proof
    let node = |named: Header, headers: Vec<Header>, proof: &'static str| {
        rest_node(move |path| {
            if path.starts_with("/blockchain/transaction/byId/") { return Some(serde_json::json!({ "id": "aa".repeat(32), "blockId": hex(&id32(&named)), "inclusionHeight": named.height }).to_string()) }
            if let Some(q) = path.strip_prefix("/blocks/chainSlice?") {
                let n = |k: &str| q.split('&').find_map(|kv| kv.strip_prefix(k)).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
                return Some(json(&headers.iter().filter(|h| h.height >= n("fromHeight=") && h.height <= n("toHeight=")).cloned().collect::<Vec<_>>()))
            }
            path.contains("/proofFor/").then(|| proof.to_string())
        })
    };
    let try_it = |n: String| included_in(&n, &stored, &tx, &mut RestCost::default()).unwrap_err();
    // an honest chain of headers, and a proof that does not lead to the block's transactions: not shown
    let e = try_it(node(block.clone(), all.clone(), r#"{"leafData":"","levels":[["0000000000000000000000000000000000000000000000000000000000000000",0]]}"#));
    assert!(e.contains("does not lead to the transactions root"), "{e}");
    // the node names a block that is not the chain's block at that height (a header from elsewhere, given that height)
    let mut other = all[7].clone(); other.height = block.height;
    let e = try_it(node(other, all.clone(), "{}"));
    assert!(e.contains("is not the agreed chain's block"), "{e}");
    // headers that do not link: one left out, or one altered (its id is computed from its bytes, so it no longer matches)
    let gap: Vec<Header> = all.iter().filter(|h| h.height != all[4].height).cloned().collect();
    assert!(try_it(node(block.clone(), gap, "{}")).contains("did not give headers that link back"));
    let mut altered = all.clone(); altered[4].timestamp += 1;
    assert!(try_it(node(block.clone(), altered, "{}")).contains("did not give headers that link back"));
    assert!(try_it(node(block.clone(), vec![], "{}")).contains("did not give headers that link back"));
    // a block above the stored chain, a node that knows nothing, an id that is not one
    let mut above = all[0].clone(); above.height += 5;
    assert!(try_it(node(above, all.clone(), "{}")).contains("above the stored chain"));
    assert!(try_it(rest_node(|_| None)).contains("knows no such transaction"));
    assert!(included_in("http://127.0.0.1:1", &stored, "nonsense", &mut RestCost::default()).unwrap_err().contains("32 bytes of hex"));
    // a block on the stored chain itself needs no linking: the id must be that header's, and then only the proof is asked for
    let e = try_it(node(all[1].clone(), vec![], r#"{"levels":[]}"#));
    assert!(e.contains("does not lead to the transactions root"), "{e}");
}

#[test]
fn proofs_are_judged_by_work_not_by_height_or_head_count() {
    use crate::sync::by_work;
    use ergo_chain_types::autolykos_pow_scheme::encode_compact_bits;
    use ergo_chain_types::{BlockId, Digest32, Header};
    use sigma_ser::ScorexSerializable;
    use tachys_verify::difficulty::difficulty;
    use tachys_verify::peers::{decide, PeerView, Verdict};
    const HOUR: u64 = 3_600_000;
    let p = ergo_nipopow::NipopowProof::scorex_parse_bytes(include_bytes!("../../../vectors/mainnet-1915810.bin")).unwrap();
    let real: Vec<Header> = std::iter::once(p.suffix_head.header.clone()).chain(p.suffix_tail.iter().cloned()).collect();
    let now = real.iter().map(|h| h.timestamp).max().unwrap() + 60_000;
    // a chain of the same shape that claims five more blocks and a quarter of the difficulty (see tachys-verify/tests/tip.rs)
    let mut forged: Vec<Header> = real.iter().enumerate().map(|(i, h)| { let mut f = h.clone(); f.height += 5; f.n_bits = encode_compact_bits(&(difficulty(h) / 4)) as u32; let mut id = [9u8; 32]; id[0] = i as u8; f.id = BlockId(Digest32::from(id)); f }).collect();
    forged.sort_by_key(|h| std::cmp::Reverse(h.height));
    for i in 0..forged.len() - 1 { forged[i].parent_id = forged[i + 1].id; }
    let view = |peer: &str, group: &str, suffix: &[Header]| PeerView { peer: peer.into(), group: group.into(), suffix: suffix.to_vec() };
    let honest = [view("a", "1.1", &real), view("b", "2.2", &real), view("c", "3.3", &real)];
    let tip = real.iter().max_by_key(|h| h.height).unwrap();
    assert_eq!(by_work(&honest, 3, now, 3 * HOUR, 128).map(|t| t.id), Ok(tip.id), "three groups on the real chain agree on its tip");
    assert!(by_work(&honest[..2], 3, now, 3 * HOUR, 128).unwrap_err()[0].contains("NoQuorum"));
    assert!(by_work(&honest, 3, now + 4 * HOUR, 3 * HOUR, 128).unwrap_err()[0].contains("Stale"));
    assert!(by_work(&[], 3, now, 3 * HOUR, 128).is_err());
    // four groups on the higher, lighter chain against three on the real one
    let mut mixed = honest.to_vec();
    mixed.extend([view("w", "6.6", &forged), view("x", "7.7", &forged), view("y", "8.8", &forged), view("z", "9.9", &forged)]);
    // the old ranking (groups, then peers, then height) handed the forged chain the win
    let old = decide(&mixed, 3);
    assert!(old.verdict != Verdict::NoQuorum && old.tip().unwrap().height == tip.height + 5, "by head count the forged tip won");
    // by work the real tip is tried first and stands
    assert_eq!(by_work(&mixed, 3, now, 3 * HOUR, 128).map(|t| t.id), Ok(tip.id));
    // and with too few honest groups nothing is agreed on: the lighter chain is refused while the heavier tip is heard
    let mut outnumbered = honest[..2].to_vec();
    outnumbered.extend(mixed[3..].iter().cloned());
    let why = by_work(&outnumbered, 3, now, 3 * HOUR, 128).unwrap_err();
    assert!(why.iter().any(|w| w.contains("NoQuorum")) && why.iter().any(|w| w.contains("HeavierHeard")), "{why:?}");
}


/// A small generator for the tests below, the same numbers every run.
fn xorshift(seed: u64) -> impl FnMut() -> u64 { let mut x = seed; move || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x } }

#[test]
fn a_malformed_proof_is_refused_without_a_panic() {
    // The verifier indexes a proof without checking its length and unwraps an empty stack: before the proof was walked
    // first, about 290,000 of 350,000 such proofs made it panic, at nine places, and the phone build aborts on a panic.
    // `lookup_unguarded` has no catch_unwind, so any panic left fails this test.
    let (_, _, held) = proven_lookup(&[]);
    let asked = [held[0].0, [9u8; 32], held[2].0];
    let (root, proof, _) = proven_lookup(&asked);
    let mut rnd = xorshift(0x9e3779b97f4a7c15);
    for _ in 0..100_000 {
        let mut p = proof.clone();
        for _ in 0..(1 + rnd() % 4) {
            let i = (rnd() as usize) % p.len().max(1);
            match rnd() % 5 { 0 => p[i] ^= 1 << (rnd() % 8), 1 => p[i] = rnd() as u8, 2 => p.insert(i, rnd() as u8), 3 => { if p.len() > 1 { p.remove(i); } } _ => p.truncate(i) }
            if p.is_empty() { break }
        }
        let got = crate::proofs::lookup_unguarded(&root, &p, &asked);
        assert!(got.is_none() || got == crate::proofs::lookup_unguarded(&root, &proof, &asked));
    }
    for _ in 0..20_000 { let p: Vec<u8> = (0..rnd() % 400).map(|_| rnd() as u8).collect(); assert_eq!(crate::proofs::lookup_unguarded(&root, &p, &asked), None); }
}

#[test]
fn an_honest_proof_from_a_large_tree_is_read_and_its_direction_bits_cannot_be_bent() {
    // the depth allowed is the root's height and two; an honest proof's tree is never deeper than the height and one
    let mut rnd = xorshift(0x1234567);
    for n in [1usize, 2, 5, 64, 1000, 20_000] {
        let (_, _, held) = proven_lookup_in(n, &[]);
        let mut asked: Vec<[u8; 32]> = (0..20).map(|j| held[(j * 7919) % n].0).collect();
        asked.dedup(); asked.push([0xab; 32]);
        let (root, proof, _) = proven_lookup_in(n, &asked);
        let (end, depth) = crate::proofs::packed_tree(&proof).expect("an honest proof packs one tree");
        assert!(depth <= root[32] as usize + 1, "{n} boxes: depth {depth}, height {}", root[32]);
        let honest = crate::proofs::lookup_unguarded(&root, &proof, &asked).expect("an honest proof is read");
        assert_eq!(honest.iter().filter(|v| v.is_some()).count(), asked.len() - 1, "{n} boxes");
        // the direction bits after the tree were read without a bound (one place the verifier panicked): flip them, and the tree's bytes
        for _ in 0..1_500 {
            let mut p = proof.clone();
            for _ in 0..(1 + rnd() % 3) { let tail = p.len() - end; let i = if rnd().is_multiple_of(2) && tail > 0 { end + (rnd() as usize) % tail } else { (rnd() as usize) % p.len() }; p[i] ^= 1 << (rnd() % 8); }
            let got = crate::proofs::lookup_unguarded(&root, &p, &asked);
            assert!(got.is_none() || got.as_ref() == Some(&honest), "{n} boxes: a changed proof gave another answer");
        }
    }
}

#[test]
fn a_proof_nested_deeper_than_its_root_says_is_refused_before_it_is_labelled() {
    // A tree 100,000 levels deep, 3.4 MB: labelling it recursed until the stack overflowed, which aborts the process in
    // every build, catch_unwind or not. Now its depth is read first and set against the height the root names.
    let mut p = vec![3u8]; p.extend([7u8; 32]);
    for _ in 0..100_000 { p.push(3); p.extend([8u8; 32]); p.push(0); }
    p.push(4); p.extend([0u8; 64]);
    assert_eq!(crate::proofs::packed_tree(&p).map(|t| t.1), Some(100_001));
    let mut root = vec![1u8; 32]; root.push(40);
    assert_eq!(crate::proofs::lookup_all(&root, &p, &[[5u8; 32]]), None);
}


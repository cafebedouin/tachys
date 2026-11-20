//! Tests for review findings W2 to W6, written to fail on the code before their fixes (f7e5f0a) and pass after them.
//! Each asserts the correct behaviour (W5 pins a documented allowance). W1 is in `src/witness.rs`.
//!
//! These run the built program, one process per command, as `tests/cli.rs` does. W4 to W6 need a network of our own
//! (`--net-file`) so that headers of a tiny difficulty carry real Autolykos work; the network is process-wide state in
//! the library, so setting it inside the crate's own unit tests would change it under the tests running beside them.
//! Nodes are on this machine: a REST port (`rest_node`) and, for W4 and W6c, tachys's own peer server.
use std::io::{Read, Write};
use std::process::Command;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ergo_avltree_rust::authenticated_tree_ops::AuthenticatedTreeOps;
use ergo_chain_types::autolykos_pow_scheme::{decode_compact_bits, encode_compact_bits};
use ergo_chain_types::{ADDigest, BlockId, Digest32, Header};
use ergo_lib::chain::ergo_box::box_builder::ErgoBoxCandidateBuilder;
use ergo_lib::wallet::derivation_path::DerivationPath;
use ergo_lib::wallet::ext_secret_key::ExtSecretKey;
use ergo_lib::wallet::mnemonic::Mnemonic;
use ergotree_ir::chain::address::{Address, AddressEncoder, NetworkPrefix};
use ergotree_ir::chain::ergo_box::box_value::BoxValue;
use ergotree_ir::chain::ergo_box::ErgoBox;
use ergotree_ir::chain::tx_id::TxId;
use ergotree_ir::serialization::SigmaSerializable;
use num_bigint::BigInt;
use serde_json::{json, Value};
use sigma_ser::ScorexSerializable;

const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const ERG: u64 = 1_000_000_000;
const MAGIC: [u8; 4] = [9, 9, 9, 9];

// ---- the program, a state directory, keys and boxes ----

struct Ran { code: i32, out: String, err: String }
impl Ran {
    fn json(&self) -> Value { serde_json::from_str(self.out.trim()).unwrap_or_else(|e| panic!("stdout is not one JSON object ({e}): {:?}\nstderr: {}", self.out, self.err)) }
}
fn tachys(args: &[&str]) -> Ran {
    let o = Command::new(env!("CARGO_BIN_EXE_tachys")).args(args).env_remove("TACHYS_PASS").output().unwrap();
    Ran { code: o.status.code().unwrap_or(-1), out: String::from_utf8_lossy(&o.stdout).to_string(), err: String::from_utf8_lossy(&o.stderr).to_string() }
}
struct Tmp(String);
impl std::ops::Deref for Tmp { type Target = str; fn deref(&self) -> &str { &self.0 } }
impl Drop for Tmp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
fn dir(name: &str) -> Tmp {
    let d = std::env::temp_dir().join(format!("tachys-witness-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Tmp(d.to_string_lossy().to_string())
}
fn now_ms() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 }
fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }
fn unhex(s: &str) -> Vec<u8> { (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect() }
fn address(i: u32) -> Address {
    let m = ExtSecretKey::derive_master(Mnemonic::to_seed(PHRASE, "")).unwrap();
    Address::P2Pk(m.derive(DerivationPath::from_str(&format!("m/44'/429'/0'/0/{i}")).unwrap()).unwrap().public_image())
}
fn text(a: &Address, testnet: bool) -> String { AddressEncoder::new(if testnet { NetworkPrefix::Testnet } else { NetworkPrefix::Mainnet }).address_to_str(a) }
/// A box at `to`, as a transaction of id zero creates it at output `index`: its id, and its bytes.
fn ergo_box(value: u64, to: &Address, creation: u32, index: u16) -> (String, Vec<u8>) {
    let b = ErgoBox::from_box_candidate(&ErgoBoxCandidateBuilder::new(BoxValue::try_from(value).unwrap(), to.script().unwrap(), creation).build().unwrap(), TxId::zero(), index).unwrap();
    (String::from(b.box_id()), b.sigma_serialize_bytes().unwrap())
}
/// A wallet file of the test phrase (its first address) tracking `boxes`, with `done` and `pending` payments.
fn wallet(st: &str, testnet: bool, boxes: Value, done: Value, pending: Value) {
    let me = text(&address(0), testnet);
    let w = json!({ "name": "witness", "address": me, "address_list": [me], "birthday": 0, "mnemonic": PHRASE, "boxes": boxes, "done": done, "pending": pending });
    std::fs::write(format!("{st}/wallet.json"), w.to_string()).unwrap();
}
fn read_wallet(st: &str) -> Value { serde_json::from_str(&std::fs::read_to_string(format!("{st}/wallet.json")).unwrap()).unwrap() }

// ---- a state tree, as a node's is built: each value filed under its hash ----

type Prover = ergo_avltree_rust::batch_avl_prover::BatchAVLProver;
fn key(v: &[u8]) -> [u8; 32] { *sigma_util::hash::blake2b256_hash(v) }
fn prover(state: &[Vec<u8>]) -> Prover {
    use ergo_avltree_rust::batch_node::{AVLTree, Node, NodeHeader};
    use ergo_avltree_rust::operation::{KeyValue, Operation};
    let mut p = Prover::new(AVLTree::new(|d| Node::LabelOnly(NodeHeader::new(Some(*d), None)), 32, None), true);
    for v in state { p.perform_one_operation(&Operation::Insert(KeyValue { key: bytes::Bytes::copy_from_slice(&key(v)), value: bytes::Bytes::from(v.clone()) })).unwrap(); }
    p.generate_proof();
    p
}
/// The state root (33 bytes) of a tree holding `state`.
fn root_of(state: &[Vec<u8>]) -> Vec<u8> { prover(state).digest().unwrap().to_vec() }
/// A batch proof of looking up `asked` (box ids, in hex) in a tree holding `state`, in hex.
fn proof_of(state: &[Vec<u8>], asked: &[String]) -> String {
    use ergo_avltree_rust::operation::Operation;
    let mut p = prover(state);
    for id in asked { p.perform_one_operation(&Operation::Lookup(bytes::Bytes::from(unhex(id)))).unwrap(); }
    hex(&p.generate_proof())
}
/// A filler value that makes one height's tree unlike another's.
fn filler(tag: u32) -> Vec<u8> { tag.to_be_bytes().iter().copied().cycle().take(44).collect() }

// ---- headers ----

fn template() -> Header { serde_json::from_str(include_str!("../../../anchors/mainnet.json")).unwrap() }
/// The compact form of a difficulty, checked to read back as that difficulty.
fn bits(d: u64) -> u32 { let n = encode_compact_bits(&BigInt::from(d)) as u32; assert_eq!(decode_compact_bits(n), BigInt::from(d), "compact form of {d}"); n }
/// The mainnet checkpoint header with these fields changed, read back from its bytes (so its id is its own).
fn make(height: u32, parent: BlockId, ts: u64, n_bits: u32, root: &[u8], nonce: u64) -> Header {
    let mut h = template();
    h.height = height; h.parent_id = parent; h.timestamp = ts; h.n_bits = n_bits;
    h.state_root = ADDigest::from(<[u8; 33]>::try_from(root).expect("a state root is 33 bytes"));
    h.autolykos_solution.nonce = nonce.to_be_bytes().to_vec();
    Header::scorex_parse_bytes(&h.scorex_serialize_bytes().unwrap()).unwrap()
}
/// `make`, with the first nonce whose Autolykos v2 hit meets the header's own difficulty: real work, if little of it.
fn mined(height: u32, parent: BlockId, ts: u64, n_bits: u32, root: &[u8]) -> Header {
    (0u64..).map(|n| make(height, parent, ts, n_bits, root, n)).find(|h| h.check_pow() == Ok(true)).unwrap()
}
/// A linked chain of headers `top - n + 1 ..= top`, newest first, one block a minute up to `top_ts`.
fn chain(n: u32, top: u32, top_ts: u64, n_bits: u32, work: bool, root: impl Fn(u32) -> Vec<u8>) -> Vec<Header> {
    let mut parent = BlockId(Digest32::from([7u8; 32]));
    let mut out = vec![];
    for height in (top + 1 - n)..=top {
        let ts = top_ts - (top - height) as u64 * 60_000;
        let h = if work { mined(height, parent, ts, n_bits, &root(height)) } else { make(height, parent, ts, n_bits, &root(height), 0) };
        parent = h.id;
        out.push(h);
    }
    out.reverse();
    out
}
/// Headers above `below` (newest first), linked onto it.
/// `first_ts`: the timestamp of the first of them; one a minute after it.
fn above(below: &Header, n: u32, first_ts: u64, n_bits: u32, root: impl Fn(u32) -> Vec<u8>) -> Vec<Header> {
    let mut out = vec![];
    let mut parent = below.clone();
    for i in 1..=n { let h = mined(below.height + i, parent.id, first_ts + (i - 1) as u64 * 60_000, n_bits, &root(below.height + i)); parent = h.clone(); out.push(h); }
    out.reverse();
    out
}
fn id32(h: &Header) -> [u8; 32] { h.id.0.into() }
/// The stored chain (`agreed-chain.bin`, newest first) and the agreed tip (`agreed-tip.bin`).
fn store_chain(st: &str, chain: &[Header], agreed: &Header) {
    let mut b = vec![];
    for h in chain { let x = h.scorex_serialize_bytes().unwrap(); b.extend_from_slice(&(x.len() as u16).to_be_bytes()); b.extend_from_slice(&x); }
    std::fs::write(format!("{st}/agreed-chain.bin"), b).unwrap();
    std::fs::write(format!("{st}/agreed-tip.bin"), agreed.scorex_serialize_bytes().unwrap()).unwrap();
}
fn stored_heights(st: &str) -> Vec<u32> {
    let raw = std::fs::read(format!("{st}/agreed-chain.bin")).unwrap();
    let (mut out, mut i) = (vec![], 0);
    while i + 2 <= raw.len() { let n = u16::from_be_bytes([raw[i], raw[i + 1]]) as usize; out.push(Header::scorex_parse_bytes(&raw[i + 2..i + 2 + n]).unwrap().height); i += 2 + n; }
    out
}
/// Headers as a node's REST port writes them (this library writes nulls for a version-2 solution's `w` and `d`, which
/// it cannot read back; see `src/tests.rs`).
fn headers_json(hs: &[Header]) -> String {
    serde_json::to_string(hs).unwrap().replace("\"w\":null", "\"w\":\"0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798\"").replace("\"d\":null", "\"d\":0")
}
/// A network of our own (`--net-file`): its checkpoint, a release floor of `checkpoint difficulty / 1`, and a block
/// interval (which sets how fast the remembered floor loosens).
fn net_file(st: &str, checkpoint: &Header, block_interval_ms: u64) -> String { net_file_with(st, checkpoint, block_interval_ms, 1) }
/// `net_file`, with a release floor of `checkpoint difficulty / floor_divisor`.
fn net_file_with(st: &str, checkpoint: &Header, block_interval_ms: u64, floor_divisor: u32) -> String {
    let cp: Value = serde_json::from_str::<Value>(&headers_json(std::slice::from_ref(checkpoint))).unwrap()[0].clone();
    let path = format!("{st}/net.json");
    std::fs::write(&path, json!({ "name": "witness", "magic": MAGIC, "genesis": "11".repeat(32), "checkpoint": cp, "floor_divisor": floor_divisor, "v2_activation_height": 0, "block_interval_ms": block_interval_ms, "seeds": "", "fee_tree": "" }).to_string()).unwrap();
    path
}

// ---- nodes on this machine ----

/// A node's REST port: `answer(method, path, body)` gives the body to send (None: 404).
fn rest_node(answer: impl Fn(&str, &str, &str) -> Option<String> + Send + 'static) -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { continue };
            let (mut buf, mut chunk) = (vec![], [0u8; 8192]);
            let req = loop {
                match c.read(&mut chunk) { Ok(0) | Err(_) => break None, Ok(n) => buf.extend_from_slice(&chunk[..n]) }
                let t = String::from_utf8_lossy(&buf).to_string();
                let Some((head, body)) = t.split_once("\r\n\r\n") else { continue };
                let want: usize = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
                if body.len() >= want { break Some((head.to_string(), body.to_string())) }
            };
            let Some((head, body)) = req else { continue };
            let mut first = head.lines().next().unwrap_or("").split_whitespace();
            let (method, path) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
            let (status, out) = match answer(&method, &path, &body) { Some(b) => (200, b), None => (404, "{}".to_string()) };
            let _ = c.write_all(format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len()).as_bytes());
        }
    });
    url
}
/// A REST node whose state is `state` and whose last headers are `tip_down` (newest first): it proves what it is asked
/// against its own state, gives its headers to a wallet behind it, and lists the boxes its state holds at an address.
fn node_at(state: Vec<Vec<u8>>, tip_down: Vec<Header>) -> String {
    rest_node(move |_, path, body| {
        if path == "/utxo/getBoxesBinaryProof" { let ids: Vec<String> = serde_json::from_str(body).ok()?; return Some(format!("\"{}\"", proof_of(&state, &ids))) }
        if path.starts_with("/blockchain/box/unspent/byAddress") {
            let tree = AddressEncoder::unchecked_parse_address_from_str(body.trim()).ok()?.script().ok()?;
            let at: Vec<Value> = state.iter().filter_map(|v| ErgoBox::sigma_parse_bytes(v).ok()).filter(|b| b.ergo_tree == tree).map(|b| json!({ "boxId": String::from(b.box_id()) })).collect();
            return Some(Value::from(at).to_string())
        }
        if let Some(n) = path.strip_prefix("/blocks/lastHeaders/") { let n: usize = n.parse().ok()?; return Some(headers_json(&tip_down[..n.min(tip_down.len())])) }
        None
    })
}
/// tachys's own peer server (`tachys_net::serve`) on this machine for `secs`: it pushes `headers` (newest first) to each
/// peer, answers header requests for them, and answers a box-proof request with `relay`.
fn peer(headers: Vec<Header>, relay: Option<tachys_net::ProofRelay>, secs: u64) -> String {
    tachys_net::set_network_magic(MAGIC);
    let addr = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().to_string();
    let a = addr.clone();
    let hs: Vec<([u8; 32], Vec<u8>)> = headers.iter().map(|h| (id32(h), h.scorex_serialize_bytes().unwrap())).collect();
    std::thread::spawn(move || { tokio::runtime::Runtime::new().unwrap().block_on(async move { let _ = tachys_net::serve(&a, hs, secs, 8, None, relay, None).await; }) });
    std::thread::sleep(Duration::from_millis(300));
    addr
}

// ---- W2 ----

/// W2 (`wallet.rs:1063` `done.truncate(20)` against `:748`): a payment given up as "not included" is still a valid
/// signed transaction, so the next payment spends one of its coins and only one of the two can confirm. That record
/// lives in `done`, which a sync cuts to twenty. Correct: after more than twenty later settled payments, the next
/// payment still spends one of the given-up payment's inputs.
#[test]
fn w2_a_given_up_payment_still_pins_its_coins_after_more_than_twenty_later_payments_settle() {
    let cp = template();
    let (me, payee) = (address(0), address(1));
    // the two boxes of `wallet.rs`'s own test of this rule: box selection alone takes the big one and leaves the small
    let (small_id, small) = ergo_box(10 * ERG, &me, cp.height - 100, 0);
    let (big_id, big) = ergo_box(50 * ERG, &me, 1, 1);
    // two payments waiting, each of whose first output (paid to someone else) is on chain: both settle as confirmed
    let paid: Vec<(String, Vec<u8>)> = (0..2).map(|i| ergo_box(ERG, &payee, 1, 10 + i)).collect();
    let state: Vec<Vec<u8>> = vec![small.clone(), big.clone(), paid[0].1.clone(), paid[1].1.clone()];
    let root = root_of(&state);
    // the same chain after the small box is spent elsewhere: a node a block behind the tip still has this state
    let gone_state: Vec<Vec<u8>> = vec![big.clone(), paid[0].1.clone(), paid[1].1.clone()];
    let top = cp.height + 50;
    let hs = chain(10, top, now_ms() - 60_000, cp.n_bits, false, |h| if h == top - 1 { root_of(&gone_state) } else { root.clone() });
    let node = node_at(state, vec![]);
    let tracked = json!({ &small_id: { "value": 10 * ERG, "confirmed_at": top - 60, "proven_at": top - 60, "bytes": hex(&small) },
                          &big_id: { "value": 50 * ERG, "confirmed_at": top - 60, "proven_at": top - 60, "bytes": hex(&big) } });
    let pending = json!((0..2).map(|i| json!({ "tx": format!("{:064x}", 0xe0 + i), "inputs": [format!("{:064x}", 0x400 + i)], "watch": paid[i].0, "json": "" })).collect::<Vec<_>>());
    // `earlier`: settled payments already in `done` above the given-up one (None: no given-up payment at all).
    // Returns the inputs of the payment `wallet send` signs after one `wallet status`, and how many `done` entries the
    // sync left, and whether the given-up one is among them.
    let run = |name: &str, earlier: Option<usize>, node: &str| -> (Vec<String>, usize, bool) {
        let st = dir(name);
        store_chain(&st, &hs, &hs[0]);
        let mut done: Vec<Value> = (0..earlier.unwrap_or(0)).map(|i| json!({ "tx": format!("{:064x}", 0x100 + i), "inputs": [format!("{:064x}", 0x200 + i)], "watch": format!("{:064x}", 0x300 + i), "result": "confirmed", "seen_at": top - 30 })).collect();
        if earlier.is_some() { done.push(json!({ "tx": "ab".repeat(32), "inputs": [small_id], "watch": "cd".repeat(32), "result": "not included", "seen_at": top - 40 })); }
        wallet(&st, false, tracked.clone(), json!(done), pending.clone());
        let s = tachys(&["wallet", "status", "--state", &st, "--node", node, "--json"]);
        assert_eq!(s.code, 0, "{name}: the sync runs ({}): {}", s.out, s.err);
        assert_eq!(s.json()["settled"], json!(true), "{name}: the proof is at the agreed tip");
        let w = read_wallet(&st);
        let left = w["done"].as_array().map_or(0, |d| d.len());
        let kept = w["done"].as_array().is_some_and(|d| d.iter().any(|x| x["result"] == "not included"));
        let r = tachys(&["wallet", "send", "--to", &text(&payee, false), "--nanoerg", "1000000000", "--state", &st, "--json"]);
        assert_eq!(r.code, 0, "{name}: the payment is signed ({}): {}", r.out, r.err);
        let tx: Value = serde_json::from_str(&std::fs::read_to_string(format!("{}/last-tx.json", &*st)).unwrap()).unwrap();
        (tx["inputs"].as_array().unwrap().iter().map(|i| i["boxId"].as_str().unwrap().to_string()).collect(), left, kept)
    };
    // control: with no payment given up, selection alone spends the big box and not the small
    let (ins, _, _) = run("w2-control", None, &node);
    assert_eq!(ins, vec![big_id.clone()], "control: selection alone takes the big box");
    // positive arm: a payment given up, and only the two settled by this sync after it: the next payment spends its coin
    let (ins, left, kept) = run("w2-few", Some(0), &node);
    assert!(kept && left == 3, "positive arm: the given-up record is kept ({left} entries)");
    assert!(ins.contains(&small_id), "positive arm: the given-up payment's coin is spent: {ins:?}");
    // the lead: nineteen settled payments already above it, and two more settled by this sync (twenty-one in all)
    let (ins, left, kept) = run("w2-many", Some(19), &node);
    assert!(ins.contains(&small_id),
        "W2: after 21 later payments settled, the next payment does not spend the given-up payment's coin {small_id}: it spends {ins:?}; the sync left {left} done entries, given-up record kept: {kept}");
    // and once its coin is gone from the chain it is settled ("coins spent") and ages out like the rest: `done` stays bounded
    let gone = node_at(gone_state.clone(), vec![]);
    let (_, left, kept) = run("w2-gone", Some(19), &gone);
    assert!(!kept && left == 20, "a given-up record whose coin is gone ages out ({left} entries, kept: {kept})");
}

// ---- W3 ----

/// W3 (`proofs.rs:120` + `wallet.rs:1077`): a proof is read against the newest four stored headers, and a box absent
/// from it is dropped as spent with no check of the height it was confirmed at. Correct: a box with `confirmed_at = H`
/// is not removed when the only proof available matches a stored header below H (it was created after that block).
#[test]
fn w3_a_box_confirmed_at_h_is_not_dropped_by_a_proof_against_a_stored_header_below_h() {
    let cp = template();
    let top = cp.height + 50;
    // the box was created in block top-1 and proven unspent at top
    let (b_id, b) = ergo_box(5 * ERG, &address(0), top - 1, 0);
    let state_at = |h: u32| -> Vec<Vec<u8>> { if h >= top - 1 { vec![filler(h), b.clone()] } else { vec![filler(h)] } };
    let hs = chain(10, top, now_ms() - 60_000, cp.n_bits, false, |h| root_of(&state_at(h)));
    let run = |name: &str, node_height: u32| -> (Ran, bool) {
        let st = dir(name);
        store_chain(&st, &hs, &hs[0]);
        wallet(&st, false, json!({ &b_id: { "value": 5 * ERG, "confirmed_at": top, "proven_at": top, "first_proven": top, "bytes": hex(&b) } }), json!([]), json!([]));
        let r = tachys(&["wallet", "status", "--state", &st, "--node", &node_at(state_at(node_height), vec![]), "--json"]);
        let kept = read_wallet(&st)["boxes"].get(&b_id).is_some();
        (r, kept)
    };
    // positive arm: a node at the agreed tip proves the box unspent there; it stays
    let (r, kept) = run("w3-at-tip", top);
    assert_eq!((r.code, &r.json()["proven_at"]), (0, &json!(top)), "positive arm: proven at the tip: {}", r.err);
    assert!(kept, "positive arm: the box stays");
    // the lead: a node two blocks behind; its proof matches the stored header at top-2, where the box did not exist yet
    let (r, kept) = run("w3-behind", top - 2);
    let j = r.json();
    assert_eq!((r.code, &j["proven_at"], &j["settled"]), (0, &json!(top - 2), &json!(true)), "the proof is read against the stored header two below the tip: {}", r.err);
    let gone: Vec<&str> = r.err.lines().filter(|l| l.contains(&b_id)).collect();
    assert!(kept, "W3: a box confirmed at {top} was dropped as spent on a proof at {} (a block before it existed); said: {gone:?}", top - 2);
}

// ---- W4 ----

/// W4 (`tips.rs:81`): a peer's tip was the highest of the headers it pushed that pass their checks, so one cheap header
/// higher than the rest became its tip. Correct (the rule now in `tips`): a peer's tip is the highest of its headers
/// that follows another it pushed (its parent, at a difficulty the chain's rule allows). Not the heaviest: after an
/// honest fall in difficulty at an epoch boundary the newest header is lighter than its ancestors. A peer whose headers
/// follow none of each other (the reference node pushes tip, tip-16, tip-128, tip-512) has its highest taken.
#[test]
fn w4_a_peers_tip_is_its_highest_pushed_header_that_follows_another_it_pushed() {
    let st = dir("w4");
    let now = now_ms();
    let root = |h: u32| -> Vec<u8> { filler(h).into_iter().chain([0u8; 33]).take(33).collect() };
    // A first run links the agreed chain back to the release checkpoint, so each arm's network has as its checkpoint the
    // parent of the chain's first header, at that header's difficulty, and a floor a quarter or an eighth of it (1).
    let net_below = |name: &str, first: u32, n_bits: u32, divisor: u32, ts: u64| -> (Header, String) {
        let d = format!("{}/{name}", &*st);
        std::fs::create_dir_all(&d).unwrap();
        let cp = make(first - 1, BlockId(Digest32::from([5u8; 32])), ts, n_bits, &[0u8; 33], 0);
        let nf = net_file_with(&d, &cp, 120_000, divisor);
        (cp, nf)
    };
    // ten linked headers at difficulty 4, the newest a minute old; and one header at difficulty 1, five blocks higher,
    // that links to nothing the peer has. Every one carries real work for its own difficulty and passes the floor (1).
    let (cp, nf) = net_below("main", 1991, bits(4), 4, now - 11 * 60_000);
    let real = above(&cp, 10, now - 10 * 60_000, bits(4), root);
    assert_eq!((real[0].height, real[9].height), (2000, 1991));
    let cheap = mined(2005, BlockId(Digest32::from([8u8; 32])), now - 30_000, bits(1), &[1u8; 33]);
    let tips_on = |nf: &str, name: &str, pushed: Vec<Header>| -> Ran {
        let seeds = format!("{}/{name}.seeds", &*st);
        std::fs::write(&seeds, peer(pushed, None, 40)).unwrap();
        tachys(&["tips", "--net-file", nf, "--seeds", &seeds, "--peers", "1", "--tries", "1", "--quorum", "1", "--depth", "10", "--wait", "2", "--json"])
    };
    let tips = |name: &str, pushed: Vec<Header>| -> Ran { tips_on(&nf, name, pushed) };
    // positive arm: the peer pushes the real chain alone; its tip is agreed on
    let r = tips("real", real.clone());
    let j = r.json();
    assert_eq!((r.code, &j["verdict"], &j["tip"]["height"]), (0, &json!("agreed"), &json!(2000)), "positive arm: {}", r.err);
    // the lead: the same peer also pushes the cheap header; it follows nothing the peer pushed, so it is not the tip
    let mut pushed = vec![cheap.clone()];
    pushed.extend(real.clone());
    let r = tips("with-cheap", pushed);
    let j = r.json();
    let said: Vec<&str> = r.err.lines().filter(|l| l.contains("tip") || l.contains("verdict")).collect();
    assert_eq!((r.code, &j["verdict"], &j["tip"]["height"]), (0, &json!("agreed"), &json!(2000)),
        "W4: a header of difficulty 1 at height 2005 pushed beside a chain of difficulty 4 up to 2000 was taken as the peer's tip; refusals {}; said: {said:#?}", j["refusals"]);
    // an honest fall in difficulty: 1917..1920 at difficulty 8, then 1921 (the first block of an epoch: 1920 = 15 * 128)
    // and on at difficulty 4. The peer's tip is the newest, 1926, not the heavier 1920 below it
    let (cp_fall, nf_fall) = net_below("fall", 1917, bits(8), 8, now - 11 * 60_000);
    let mut fall: Vec<Header> = vec![];
    let mut parent = cp_fall.id;
    for h in 1917..=1926u32 {
        let x = mined(h, parent, now - (1926 - h) as u64 * 60_000 - 60_000, bits(if h <= 1920 { 8 } else { 4 }), &root(h));
        parent = x.id; fall.insert(0, x);
    }
    let r = tips_on(&nf_fall, "fall", fall);
    let j = r.json();
    assert_eq!((r.code, &j["verdict"], &j["tip"]["height"]), (0, &json!("agreed"), &json!(1926)), "after an honest fall in difficulty the peer's tip is its newest header: {}", r.err);
    // a peer whose headers follow none of each other, as the reference node pushes them: its highest is its tip
    let sparse: Vec<Header> = real.iter().filter(|h| [2000, 1995, 1991].contains(&h.height)).cloned().collect();
    let r = tips("sparse", sparse);
    assert!(r.err.lines().any(|l| l.contains("3 headers (3 pass), tip 2000")), "sparse headers: the highest is the tip: {}", r.err);
}

// ---- W5 ----

/// W5's headers: an agreed tip at 2560 (the last block of an epoch: 2560 = 20 * 128) of difficulty 65538, and a child
/// of it, the first block of the next epoch, at difficulty `d`. Fixed (no clock), so the nonce can be mined once.
const W5_T0: u64 = 1_794_000_000_000;
fn w5_chain() -> Vec<Header> { chain(10, 2560, W5_T0, bits(65_538), false, |h| filler(h).into_iter().chain([0u8; 33]).take(33).collect()) }
fn w5_child(d: u64, nonce: u64) -> Header { make(2561, w5_chain()[0].id, W5_T0 + 120_000, bits(d), &[9u8; 33], nonce) }
/// The nonces below were found by `w5_mine_nonces` (ignored: about as many Autolykos hits as the difficulty, each).
const W5_NONCE_32768: u64 = 15_239;
const W5_NONCE_32769: u64 = 60_347;
const W5_NONCE_16384: u64 = 12_592;

#[test]
#[ignore = "mines W5's three nonces; run once with --ignored --nocapture and paste them above"]
fn w5_mine_nonces() {
    for d in [32_768u64, 32_769, 16_384] {
        let found = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(u64::MAX));
        let threads: Vec<_> = (0..12u64).map(|t| { let found = found.clone(); std::thread::spawn(move || {
            let mut n = t;
            while found.load(std::sync::atomic::Ordering::Relaxed) == u64::MAX {
                // the library's `gen_indexes` panics on an index of exactly 0 (about one hit in two million): skipped, and said
                match std::panic::catch_unwind(|| w5_child(d, n).check_pow()) {
                    Ok(Ok(true)) => found.store(n, std::sync::atomic::Ordering::Relaxed),
                    Err(_) => println!("difficulty {d}: nonce {n} makes check_pow panic"),
                    _ => {}
                }
                n += 12;
            }
        }) }).collect();
        for t in threads { t.join().unwrap(); }
        println!("difficulty {d}: nonce {}", found.load(std::sync::atomic::Ordering::Relaxed));
    }
}

/// W5: headers a single node adds in `prove_p2p` / `extend_chain` are not held to the floor `tachys tips` remembers from
/// the agreed tip; they are held to `continues`, at an epoch's first block at least half the difficulty they follow,
/// less the compact form's rounding (one part in 2^15). The remembered floor is half in the first epoch-time after a
/// tip, so the only headers taken below it are in that rounding window, where an honest fall of the largest size lands
/// too. This test pins that documented window (see `extend_chain`): a header in it is taken; one clearly below it is not.
///
/// Reached through `wallet receive`: the stored chain is old, so the first proof is refused as stale and the node is
/// asked for newer headers (`extend_chain`); whether it took the header is the sentence it says. The network's block
/// interval is set so long that the remembered floor has not loosened since the agreed tip (one halving: what it is
/// in the first epoch-time after a tip), which is what lets the headers be fixed.
#[test]
fn w5_a_node_supplied_header_is_taken_only_down_to_the_rounding_window_below_the_remembered_floor() {
    use tachys_verify::difficulty::{difficulty, ChainParams};
    let st = dir("w5");
    let cp = make(1000, BlockId(Digest32::from([5u8; 32])), W5_T0 - 86_400_000, bits(1), &[0u8; 33], 0);
    let interval = 1_000_000_000_000u64;
    let nf = net_file(&st, &cp, interval);
    let hs = w5_chain();
    let agreed = hs[0].clone();
    let params = ChainParams { block_interval_ms: interval, v2_activation_height: 0, ..ChainParams::mainnet() };
    let floor = tachys_verify::tip::remembered_floor(&agreed, now_ms(), &params);
    assert_eq!(floor, BigInt::from(32_769), "the remembered floor: 65538, halved once");
    let try_child = |name: &str, child: &Header| -> Ran {
        assert_eq!(child.check_pow(), Ok(true), "{name}: the child carries work for its difficulty (re-mine with w5_mine_nonces if the headers changed)");
        let s = dir(&format!("w5-{name}"));
        store_chain(&s, &hs, &agreed);
        wallet(&s, true, json!({}), json!([]), json!([]));
        tachys(&["wallet", "receive", "--ids", &"ab".repeat(32), "--state", &s, "--net-file", &nf, "--node", &node_at(vec![], vec![child.clone()]), "--json"])
    };
    let taken = |r: &Ran| r.err.contains("1 block(s) past the agreed tip; extended");
    // at the remembered floor: taken
    let at = w5_child(32_769, W5_NONCE_32769);
    assert!(difficulty(&at) >= floor && tachys_verify::tip::continues(&agreed, &at, 128));
    let r = try_child("at-floor", &at);
    assert!(taken(&r), "a header at the remembered floor is taken: {}", r.err);
    // one unit below it, inside the rounding window (65538 / 2 less one part in 2^15): taken, on purpose
    let window = w5_child(32_768, W5_NONCE_32768);
    assert!(difficulty(&window) < floor && tachys_verify::tip::continues(&agreed, &window, 128), "in the window: below the remembered floor, and may follow the tip");
    let r = try_child("in-window", &window);
    assert!(taken(&r), "a header in the rounding window below the remembered floor is taken (documented): {}", r.err);
    // a quarter of the agreed tip's difficulty, with real work for it: clearly below the window, refused
    let quarter = w5_child(16_384, W5_NONCE_16384);
    assert!(!tachys_verify::tip::continues(&agreed, &quarter, 128), "a quarter may not follow the tip");
    let r = try_child("quarter", &quarter);
    assert!(!taken(&r), "a header at a quarter of the agreed tip's difficulty is refused: {}", r.err);
}

// ---- W6 ----

/// W6's world, on a network of our own: an agreed chain of ten up to height 2000 (the agreed tip), five more blocks
/// above it that some nodes have, and the wallet's one box B, created long before. Each height's state holds B and a
/// filler of its own, so a proof matches one header only.
struct W6 { st: Tmp, nf: String, b_id: String, agreed: Header, all: Vec<Header>, state: Box<dyn Fn(u32) -> Vec<Vec<u8>>> }
fn w6(name: &str, above_n: u32) -> W6 { w6_aged(name, above_n, 20 * 60_000) }
/// `w6`, with the agreed tip `age_ms` old; the headers above it come up to a minute ago.
fn w6_aged(name: &str, above_n: u32, age_ms: u64) -> W6 {
    let st = dir(name);
    let now = now_ms();
    let cp = make(1000, BlockId(Digest32::from([5u8; 32])), now - 3_600_000, bits(1), &[0u8; 33], 0);
    let nf = net_file(&st, &cp, 120_000);
    let (b_id, b) = ergo_box(3 * ERG, &address(0), 1500, 0);
    let state = move |h: u32| -> Vec<Vec<u8>> { vec![filler(h), b.clone()] };
    let agreed_chain = chain(10, 2000, now - age_ms, bits(1), true, |h| root_of(&state(h)));
    let agreed = agreed_chain[0].clone();
    let first = (agreed.timestamp + 60_000).max(now.saturating_sub(above_n as u64 * 60_000));
    let mut all = above(&agreed, above_n, first, bits(1), |h| root_of(&state(h)));
    all.extend(agreed_chain.clone());
    store_chain(&st, &agreed_chain, &agreed);
    wallet(&st, true, json!({}), json!([]), json!([]));
    W6 { st, nf, b_id, agreed, all, state: Box::new(state) }
}
impl W6 {
    /// A REST node at height `h`: its state there, and its last headers from `h` down.
    fn node(&self, h: u32) -> String { node_at((self.state)(h), self.all.iter().filter(|x| x.height <= h).cloned().collect()) }
    fn sync(&self, sub: &str, node: &str) -> Ran {
        let mut args = vec!["wallet", sub, "--state", &self.st, "--net-file", &self.nf, "--node", node, "--json"];
        if sub == "receive" { args.extend(["--ids", &self.b_id]); }
        tachys(&args)
    }
    /// Does a sync against an honest node at the agreed tip settle the box there?
    fn honest_settles(&self) -> (bool, Ran) {
        let r = self.sync("status", &self.node(self.agreed.height));
        let ok = r.code == 0 && r.json()["settled"] == json!(true) && r.json()["proven_at"] == json!(self.agreed.height)
            && read_wallet(&self.st)["boxes"][&self.b_id]["confirmed_at"] == json!(self.agreed.height);
        (ok, r)
    }
}
fn said(r: &Ran) -> Vec<String> { r.err.lines().filter(|l| l.contains("proof") || l.contains("past the agreed tip") || l.contains("PROVEN") || l.contains("note:")).map(String::from).collect() }

/// W6 (`wallet.rs:962-966`): after a proof, the chain with the headers a node added is saved over the agreed one
/// (`take(16)` drops the oldest), while proofs are read against its newest four. Correct (as first stated): after one
/// sync where a node extended the chain, a later sync against an honest node at the agreed tip still settles a box at
/// or below the agreed tip. The node here is a REST node, which adds at most three headers in a sync.
#[test]
fn w6_after_one_sync_extended_by_a_rest_node_an_honest_node_at_the_agreed_tip_still_settles() {
    // positive arm: no node has extended the chain; a node at the agreed tip settles the box
    let w = w6("w6-plain", 3);
    let r = w.sync("receive", &w.node(2000));
    assert_eq!((r.code, &r.json()["settled"]), (0, &json!(true)), "positive arm, first sync: {}", r.err);
    let (ok, r) = w.honest_settles();
    assert!(ok, "positive arm: {:#?}", said(&r));
    // the lead: the first sync is against a node three blocks ahead, which extends the stored chain
    let w = w6("w6-rest", 3);
    let r = w.sync("receive", &w.node(2003));
    assert_eq!((r.code, &r.json()["proven_at"], &r.json()["settled"]), (0, &json!(2003), &json!(false)), "first sync: extended by three and proven above the agreed tip: {}", r.err);
    assert_eq!(stored_heights(&w.st)[..4], [2003, 2002, 2001, 2000], "the node's headers were saved on top of the agreed chain");
    let (ok, r) = w.honest_settles();
    assert!(ok, "W6: after one sync extended by a REST node, an honest node at the agreed tip does not settle the box: exit {}; stored chain {:?}; said: {:#?}", r.code, stored_heights(&w.st), said(&r));
}

/// W6, beyond the first case: two syncs, each extended by a REST node three blocks further. Correct, by the
/// same rule: a later sync against an honest node at the agreed tip still settles the box.
#[test]
fn w6b_after_two_syncs_extended_by_rest_nodes_an_honest_node_at_the_agreed_tip_still_settles() {
    let w = w6("w6b", 6);
    let r = w.sync("receive", &w.node(2003));
    assert_eq!((r.code, &r.json()["proven_at"]), (0, &json!(2003)), "first sync: {}", r.err);
    let r = w.sync("status", &w.node(2006));
    assert_eq!((r.code, &r.json()["proven_at"], &r.json()["settled"]), (0, &json!(2006), &json!(false)), "second sync: extended three more: {}", r.err);
    assert_eq!(stored_heights(&w.st)[..4], [2006, 2005, 2004, 2003]);
    let (ok, r) = w.honest_settles();
    assert!(ok, "W6b: after two syncs extended by REST nodes, an honest node at the agreed tip does not settle the box: exit {}; stored chain {:?}; said: {:#?}", r.code, stored_heights(&w.st), said(&r));
}

/// W6, through the peer protocol: one sync against a peer five blocks ahead (`prove_p2p` links up to twelve headers in a
/// sync). Correct, by the same rule: a later sync against an honest node at the agreed tip still settles the box.
#[test]
fn w6c_after_one_sync_extended_over_the_peer_protocol_an_honest_node_at_the_agreed_tip_still_settles() {
    let w = w6("w6c", 5);
    let top: Vec<Header> = w.all.iter().filter(|h| h.height > 2000).cloned().collect();
    let (named, state) = (id32(&top[0]), (w.state)(2005));
    let relay: tachys_net::ProofRelay = std::sync::Arc::new(move |ids: Vec<[u8; 32]>| {
        let proof = unhex(&proof_of(&state, &ids.iter().map(|i| hex(i)).collect::<Vec<_>>()));
        Box::pin(async move { Some(tachys_net::RelayAnswer { header_id: named, proof, new_headers: vec![] }) })
    });
    let p = format!("p2p://{}", peer(top, Some(relay), 40));
    let r = w.sync("receive", &p);
    assert_eq!((r.code, &r.json()["proven_at"], &r.json()["settled"]), (0, &json!(2005), &json!(false)), "first sync over the peer protocol: extended by five: {}", r.err);
    assert_eq!(stored_heights(&w.st)[..6], [2005, 2004, 2003, 2002, 2001, 2000]);
    let (ok, r) = w.honest_settles();
    assert!(ok, "W6c: after one sync extended over the peer protocol, an honest node at the agreed tip does not settle the box: exit {}; stored chain {:?}; said: {:#?}", r.code, stored_heights(&w.st), said(&r));
}

/// W6, far past the agreed tip: a sync over the peer protocol twelve blocks ahead, then two REST syncs three more each,
/// eighteen headers above the agreed tip in all, more than the sixteen the stored chain used to keep. Correct: the
/// agreed tip stays stored, and an honest node at it still settles the box.
#[test]
fn w6d_after_eighteen_headers_added_above_the_agreed_tip_an_honest_node_at_it_still_settles() {
    let w = w6("w6d", 18);
    let top: Vec<Header> = w.all.iter().filter(|h| h.height > 2000 && h.height <= 2012).cloned().collect();
    let (named, state) = (id32(&top[0]), (w.state)(2012));
    let relay: tachys_net::ProofRelay = std::sync::Arc::new(move |ids: Vec<[u8; 32]>| {
        let proof = unhex(&proof_of(&state, &ids.iter().map(|i| hex(i)).collect::<Vec<_>>()));
        Box::pin(async move { Some(tachys_net::RelayAnswer { header_id: named, proof, new_headers: vec![] }) })
    });
    let r = w.sync("receive", &format!("p2p://{}", peer(top, Some(relay), 40)));
    assert_eq!((r.code, &r.json()["proven_at"]), (0, &json!(2012)), "over the peer protocol: {}", r.err);
    for h in [2015, 2018] {
        let r = w.sync("status", &w.node(h));
        assert_eq!((r.code, &r.json()["proven_at"], &r.json()["settled"]), (0, &json!(h), &json!(false)), "REST sync to {h}: {}", r.err);
    }
    let stored = stored_heights(&w.st);
    assert!(stored.contains(&2000) && stored[0] == 2018, "the agreed tip is still stored: {stored:?}");
    let (ok, r) = w.honest_settles();
    assert!(ok, "W6d: exit {}; stored chain {:?}; said: {:#?}", r.code, stored_heights(&w.st), said(&r));
}

/// W6's fix reads the proof against the agreed tip's header even when added headers fill the newest few; it must not
/// read it against an agreed tip older than a proof may rest on. Here the agreed tip is four hours old, and two syncs
/// added six recent headers above it: an honest node at the old agreed tip does not settle the box (run `tachys tips` again).
#[test]
fn w6e_a_proof_is_not_settled_against_an_agreed_tip_older_than_a_proof_may_rest_on() {
    let w = w6_aged("w6e", 6, 4 * 3_600_000);
    for h in [2003, 2006] {
        let r = w.sync(if h == 2003 { "receive" } else { "status" }, &w.node(h));
        assert_eq!((r.code, &r.json()["proven_at"], &r.json()["settled"]), (0, &json!(h), &json!(false)), "sync extended to {h}: {}", r.err);
    }
    assert_eq!(stored_heights(&w.st)[..4], [2006, 2005, 2004, 2003], "the agreed tip is out of the newest four");
    let (ok, r) = w.honest_settles();
    assert!(!ok && r.code == 1, "a proof against a four-hour-old agreed tip was read: exit {}; said: {:#?}", r.code, said(&r));
}

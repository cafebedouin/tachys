//! A forged chain against a live client: headers mined here at a chosen difficulty, served by `tachys serve` from a
//! state directory of their own, and judged by `tachys tips` over the peer protocol, both the built binary. Nothing
//! leaves this machine. The checkpoint is made here too, so the floor (the checkpoint's difficulty over the
//! network's divisor) is small enough to mine at in a test: what is shown is how the client judges, not what an attack
//! on mainnet costs.
use std::process::{Child, Command, Stdio};

use ergo_chain_types::autolykos_pow_scheme::encode_compact_bits;
use ergo_chain_types::{ADDigest, Header};
use num_bigint::BigInt;
use sigma_ser::ScorexSerializable;

const BIN: &str = env!("CARGO_BIN_EXE_tachys");
/// The checkpoint's difficulty and the network's divisor: the floor is 16.
const CHECKPOINT_DIFFICULTY: u32 = 64;
const DIVISOR: u32 = 4;
const FLOOR: u32 = CHECKPOINT_DIFFICULTY / DIVISOR;

struct Tmp(String);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
impl std::fmt::Display for Tmp {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
fn dir(name: &str) -> Tmp {
    let d = std::env::temp_dir().join(format!("tachys-forged-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Tmp(d.to_string_lossy().to_string())
}
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
fn bits(d: u32) -> u32 {
    encode_compact_bits(&BigInt::from(d)) as u32
}
/// A header as it is read off the wire: serialised and parsed again, so its id is its own.
fn sealed(h: &Header) -> Header {
    Header::scorex_parse_bytes(&h.scorex_serialize_bytes().unwrap()).unwrap()
}
/// The next header after `parent`, mined at `difficulty` with a real Autolykos solution.
fn mine(parent: &Header, difficulty: u32, root: &ADDigest, timestamp: u64) -> Header {
    let mut h = parent.clone();
    h.parent_id = parent.id;
    h.height = parent.height + 1;
    h.n_bits = bits(difficulty);
    h.state_root = *root;
    h.timestamp = timestamp;
    for n in 0u64.. {
        h.autolykos_solution.nonce = n.to_be_bytes().to_vec();
        if tachys_verify::pow::pow_ok(&h) == Ok(true) {
            return sealed(&h);
        }
    }
    unreachable!()
}
/// The release's mainnet checkpoint, made over: recent, and at `CHECKPOINT_DIFFICULTY`. In the node's JSON, which is
/// what a network file holds, with the id it now has.
fn checkpoint() -> (Header, serde_json::Value) {
    let mut v: serde_json::Value =
        serde_json::from_str(include_str!("../../../anchors/mainnet.json")).unwrap();
    v["nBits"] = bits(CHECKPOINT_DIFFICULTY).into();
    // an epoch's last block, whatever height the release checkpoint is at: the tests below let the difficulty fall
    // at the block right after it, which the rule allows only at an epoch's first
    let height = v["height"].as_u64().unwrap();
    v["height"] = (height - height % 128).into();
    v["timestamp"] = (now_ms() - 3_600_000).into();
    let h = sealed(&serde_json::from_value::<Header>(v.clone()).unwrap());
    let id: Vec<u8> = h.id.0.into();
    v["id"] = id
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
        .into();
    (h, v)
}
/// `n` headers on `from`, newest first, each at `difficulty` (the newest at `tip` if given), recent and a minute apart.
fn chain(
    from: &Header,
    n: usize,
    difficulty: u32,
    tip: Option<u32>,
    root: &ADDigest,
) -> Vec<Header> {
    let start = now_ms() - 60_000 * (n as u64 + 1);
    let mut out = vec![];
    let mut last = from.clone();
    for i in 0..n {
        let d = if i + 1 == n {
            tip.unwrap_or(difficulty)
        } else {
            difficulty
        };
        last = mine(&last, d, root, start + 60_000 * (i as u64 + 1));
        out.push(last.clone());
    }
    out.reverse();
    out
}
/// The network file both sides read: our checkpoint, our divisor, a magic of our own.
fn net_file(at: &Tmp, cp: &serde_json::Value) -> String {
    let f = format!("{at}/net.json");
    let v = serde_json::json!({ "name": "forged", "magic": [9, 8, 7, 6], "genesis": "00".repeat(32), "checkpoint": cp, "floor_divisor": DIVISOR, "v2_activation_height": 0, "block_interval_ms": 120_000, "seeds": "" });
    std::fs::write(&f, v.to_string()).unwrap();
    f
}
/// A state directory whose stored chain is `headers` (newest first, from just above the checkpoint), as `tachys tips
/// --keep-chain` would have left it, so that `serve` also answers a first run linking back to the checkpoint.
fn store(at: &str, headers: &[Header]) {
    let bytes = |hs: &[Header]| {
        let mut raw = vec![];
        for h in hs {
            let b = h.scorex_serialize_bytes().unwrap();
            raw.extend_from_slice(&(b.len() as u16).to_be_bytes());
            raw.extend_from_slice(&b);
        }
        raw
    };
    // as `tips --keep-chain` leaves them: the newest ten agreed on, and the whole chain from just above the checkpoint
    std::fs::write(
        format!("{at}/agreed-chain.bin"),
        bytes(&headers[..headers.len().min(10)]),
    )
    .unwrap();
    std::fs::write(format!("{at}/linked-chain.bin"), bytes(headers)).unwrap();
}
/// `tachys serve` passing on `headers` on a port of this machine; stopped when dropped.
struct Forger(Child, String);
impl Drop for Forger {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn forger(at: &Tmp, name: &str, net: &str, headers: &[Header]) -> Forger {
    let st = format!("{at}/{name}");
    std::fs::create_dir_all(&st).unwrap();
    store(&st, headers);
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let addr = format!("127.0.0.1:{port}");
    let child = Command::new(BIN)
        .args([
            "serve",
            "--state",
            &st,
            "--listen",
            &addr,
            "--secs",
            "60",
            "--net-file",
            net,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if std::net::TcpStream::connect(&addr).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Forger(child, addr)
}
/// What `tachys tips` agrees on when its only peers are `peers`.
fn tips(at: &Tmp, state: &str, net: &str, peers: &[&Forger]) -> (i32, serde_json::Value, String) {
    tips_with(at, state, net, peers, &[])
}
fn tips_with(
    at: &Tmp,
    state: &str,
    net: &str,
    peers: &[&Forger],
    extra: &[&str],
) -> (i32, serde_json::Value, String) {
    let seeds = format!("{at}/seeds-{state}.txt");
    std::fs::write(
        &seeds,
        peers
            .iter()
            .map(|p| p.1.clone())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let n = peers.len().to_string();
    let o = Command::new(BIN)
        .args([
            "tips",
            "--seeds",
            &seeds,
            "--peers",
            &n,
            "--tries",
            &n,
            "--quorum",
            "1",
            "--state",
            &format!("{at}/{state}"),
            "--net-file",
            net,
            "--json",
        ])
        .args(extra)
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null);
    (
        o.status.code().unwrap_or(-1),
        v,
        String::from_utf8_lossy(&o.stderr).to_string(),
    )
}
fn root(b: u8) -> ADDigest {
    ADDigest::from([b; 33])
}

// The checkpoint made here is an hour old and the network's pace is a block in two minutes, so a first run wants a tip
// at least fifteen headers above it (half the pace). A first run also links back to the checkpoint, so a chain must
// start at the checkpoint's difficulty and may fall by at most half at an epoch's first block (the one right after it).

#[test]
fn a_forged_chain_below_the_floor_or_at_it_is_refused_on_a_first_run() {
    let at = dir("below");
    let (cp, cp_json) = checkpoint();
    let net = net_file(&at, &cp_json);
    let below = forger(
        &at,
        "forger",
        &net,
        &chain(&cp, 18, FLOOR / 2, None, &root(1)),
    );
    let (code, v, err) = tips(&at, "victim", &net, &[&below]);
    assert_eq!(
        (code, v["verdict"].as_str()),
        (1, Some("not_trusted")),
        "{err}"
    );
    assert!(
        err.contains("least difficulty a header may claim: 16") && err.contains("no tip heard"),
        "its tip is put aside unheard: {err}"
    );
    // at the floor itself: it is judged, and then it cannot be linked to the checkpoint (it dropped to a quarter at once).
    // Before the first run linked (finding 53) this chain was agreed on, and that was all a false tip cost
    drop(below);
    let at_floor = forger(&at, "forger2", &net, &chain(&cp, 18, FLOOR, None, &root(1)));
    let (code, v, err) = tips(&at, "victim", &net, &[&at_floor]);
    assert_eq!(
        (code, v["verdict"].as_str()),
        (1, Some("not_trusted")),
        "{err}"
    );
    assert!(
        err.contains("must link the chain to the release checkpoint")
            && err.contains("does not follow the one before"),
        "{err}"
    );
}

#[test]
fn a_forger_on_a_first_run_must_mine_from_the_checkpoint_at_the_chains_pace() {
    let at = dir("pace");
    let (cp, cp_json) = checkpoint();
    let net = net_file(&at, &cp_json);
    // half the checkpoint's difficulty from its first header on (the rule allows that much there), but too few headers
    // for the hour since the checkpoint
    let short = forger(
        &at,
        "short",
        &net,
        &chain(&cp, 12, CHECKPOINT_DIFFICULTY / 2, None, &root(1)),
    );
    let (code, v, err) = tips(&at, "victim", &net, &[&short]);
    assert_eq!(
        (code, v["verdict"].as_str()),
        (1, Some("not_trusted")),
        "{err}"
    );
    assert!(err.contains("half the height"), "{err}");
    drop(short);
    // enough headers, each a step the rule allows: agreed. That is the price now: every header from the checkpoint to a
    // tip as high as the time since it implies, at no less than half its difficulty for an epoch and the floor after
    let paid = forger(
        &at,
        "paid",
        &net,
        &chain(&cp, 18, CHECKPOINT_DIFFICULTY / 2, None, &root(1)),
    );
    let (code, v, err) = tips(&at, "victim", &net, &[&paid]);
    assert_eq!((code, v["verdict"].as_str()), (0, Some("agreed")), "{err}");
    assert!(
        err.contains(&format!("linked: 8 header(s) from height {}", cp.height)),
        "the eight below the ten judged: {err}"
    );
}

#[test]
fn a_forged_tip_claiming_more_work_than_the_headers_under_it_is_refused_by_a_live_client() {
    // light headers and one heavy one at the tip: ranked by its own claim it would outweigh an honest chain. A peer's tip
    // is the highest header it pushed that follows another it pushed at a step the difficulty rule allows; the heavy one
    // (eight times its parent, not at an epoch's first block) follows none, so it is never ranked by its claim. What is
    // judged is the light chain under it, which has paid for every header from the checkpoint, as in the test above
    let hex = |h: &Header| -> String {
        let id: Vec<u8> = h.id.0.into();
        id.iter().map(|b| format!("{b:02x}")).collect()
    };
    let at = dir("heavy-tip");
    let (cp, cp_json) = checkpoint();
    let net = net_file(&at, &cp_json);
    let heavy_chain = chain(
        &cp,
        18,
        CHECKPOINT_DIFFICULTY / 2,
        Some(CHECKPOINT_DIFFICULTY * 4),
        &root(1),
    );
    let f = forger(&at, "forger", &net, &heavy_chain);
    let (code, v, err) = tips(&at, "victim", &net, &[&f]);
    assert!(
        !err.contains(&format!("tip at height {}", heavy_chain[0].height)),
        "the heavy header is not tried as a tip: {err}"
    );
    assert_eq!(
        (code, v["verdict"].as_str(), v["tip"]["id"].as_str()),
        (0, Some("agreed"), Some(hex(&heavy_chain[1]).as_str())),
        "agreed at most on what was paid for: {err}"
    );
    drop(f);
    // beside an honest peer, the heavy claim neither wins nor blocks the honest chain as a heavier tip heard
    let honest_chain = chain(&cp, 18, CHECKPOINT_DIFFICULTY, None, &root(2));
    let honest = forger(&at, "honest", &net, &honest_chain);
    let f = forger(&at, "forger2", &net, &heavy_chain);
    let (code, v, err) = tips(&at, "victim2", &net, &[&f, &honest]);
    assert_eq!(
        (code, v["verdict"].as_str(), v["tip"]["id"].as_str()),
        (0, Some("agreed"), Some(hex(&honest_chain[0]).as_str())),
        "the honest tip: {err}"
    );
}

#[test]
fn a_live_client_given_an_honest_peer_and_a_forger_agrees_on_the_heavier_chain() {
    let at = dir("two-peers");
    let (cp, cp_json) = checkpoint();
    let net = net_file(&at, &cp_json);
    let honest_chain = chain(&cp, 18, CHECKPOINT_DIFFICULTY, None, &root(2));
    // the forger's chain is longer and claims greater heights, lighter
    let forged_chain = chain(&cp, 26, CHECKPOINT_DIFFICULTY / 2, None, &root(1));
    let honest = forger(&at, "honest", &net, &honest_chain);
    let forged = forger(&at, "forger", &net, &forged_chain);
    let (code, v, err) = tips(&at, "victim", &net, &[&forged, &honest]);
    assert_eq!((code, v["verdict"].as_str()), (0, Some("agreed")), "{err}");
    let id: Vec<u8> = honest_chain[0].id.0.into();
    assert_eq!(
        v["tip"]["id"].as_str(),
        Some(
            id.iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
                .as_str()
        ),
        "the honest tip, though lower: {err}"
    );
}

#[test]
fn a_wallet_that_agreed_on_a_heavier_chain_before_refuses_a_forged_one_at_the_release_floor() {
    // a wallet that has agreed on a tip before holds later chains to what it saw, loosened only as fast as difficulty
    // can honestly fall; one on its first run links to the checkpoint
    let at = dir("remembered");
    let (cp, cp_json) = checkpoint();
    let net = net_file(&at, &cp_json);
    let honest = forger(
        &at,
        "honest",
        &net,
        &chain(&cp, 18, CHECKPOINT_DIFFICULTY, None, &root(2)),
    );
    let (code, _, err) = tips(&at, "victim", &net, &[&honest]);
    assert_eq!(code, 0, "{err}");
    drop(honest);
    let forged = forger(&at, "forger", &net, &chain(&cp, 26, FLOOR, None, &root(1)));
    let (code, v, err) = tips(&at, "victim", &net, &[&forged]);
    assert_eq!(
        (code, v["verdict"].as_str()),
        (1, Some("not_trusted")),
        "{err}"
    );
    assert!(
        err.contains("least difficulty a header may claim: 32"),
        "the floor it remembers: half of what it agreed on: {err}"
    );
    // a wallet that has never agreed on anything: it took this chain before the first run linked; now it does not
    let (code, v, err) = tips(&at, "newcomer", &net, &[&forged]);
    assert_eq!(
        (code, v["verdict"].as_str()),
        (1, Some("not_trusted")),
        "{err}"
    );
    assert!(
        err.contains("must link the chain to the release checkpoint"),
        "{err}"
    );
}

#[test]
fn a_forged_chain_at_the_floor_cannot_make_a_wallet_on_its_first_run_prove_a_payment_that_was_never_made(
) {
    // What a false tip was worth before a first run linked to the checkpoint (finding 53): the forger
    // keeps a state tree of its own, with one box in it paying the victim, puts its root in every header it mines,
    // and answers the victim's request for a proof from that tree. Nothing of this box was ever on a real chain.
    use bytes::Bytes;
    use ergo_avltree_rust::authenticated_tree_ops::AuthenticatedTreeOps;
    use ergo_avltree_rust::batch_avl_prover::BatchAVLProver;
    use ergo_avltree_rust::batch_node::{AVLTree, Node, NodeHeader};
    use ergo_avltree_rust::operation::{KeyValue, Operation};
    use ergo_lib::chain::ergo_box::box_builder::ErgoBoxCandidateBuilder;
    use ergo_lib::wallet::derivation_path::DerivationPath;
    use ergo_lib::wallet::ext_secret_key::ExtSecretKey;
    use ergo_lib::wallet::mnemonic::Mnemonic;
    use ergotree_ir::chain::address::{Address, AddressEncoder, NetworkPrefix};
    use ergotree_ir::chain::ergo_box::{box_value::BoxValue, ErgoBox};
    use ergotree_ir::chain::tx_id::TxId;
    use ergotree_ir::serialization::SigmaSerializable;
    use std::io::{Read, Write};
    use std::str::FromStr;
    const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    let at = dir("fake-payment");
    let (cp, cp_json) = checkpoint();
    let net = net_file(&at, &cp_json);
    // the victim: a wallet of the test phrase that has never agreed on a tip
    let m = ExtSecretKey::derive_master(Mnemonic::to_seed(PHRASE, "")).unwrap();
    let me = Address::P2Pk(
        m.derive(DerivationPath::from_str("m/44'/429'/0'/0/0").unwrap())
            .unwrap()
            .public_image(),
    );
    let addr = AddressEncoder::new(NetworkPrefix::Testnet).address_to_str(&me);
    let victim = format!("{at}/victim");
    std::fs::create_dir_all(&victim).unwrap();
    std::fs::write(format!("{victim}/wallet.json"), serde_json::json!({ "name": "victim", "address": addr, "address_list": [addr], "birthday": 0, "mnemonic": PHRASE, "boxes": {} }).to_string()).unwrap();
    // the forger's state tree: 1,000 ERG to the victim
    let fake = ErgoBox::from_box_candidate(
        &ErgoBoxCandidateBuilder::new(
            BoxValue::try_from(1_000_000_000_000u64).unwrap(),
            me.script().unwrap(),
            cp.height,
        )
        .build()
        .unwrap(),
        TxId::zero(),
        0,
    )
    .unwrap();
    let (id, bytes) = (fake.box_id(), fake.sigma_serialize_bytes().unwrap());
    let key: Vec<u8> = id.as_ref().to_vec();
    let mut prover = BatchAVLProver::new(
        AVLTree::new(
            |d| Node::LabelOnly(NodeHeader::new(Some(*d), None)),
            32,
            None,
        ),
        true,
    );
    prover
        .perform_one_operation(&Operation::Insert(KeyValue {
            key: Bytes::copy_from_slice(&key),
            value: Bytes::from(bytes),
        }))
        .unwrap();
    prover.generate_proof();
    let digest = prover.digest().unwrap().to_vec();
    prover
        .perform_one_operation(&Operation::Lookup(Bytes::copy_from_slice(&key)))
        .unwrap();
    let proof = prover.generate_proof().to_vec();
    let root = ADDigest::try_from(digest).unwrap();
    // the forger mines ten headers and a few at the floor over that root, serves them, and answers proof requests
    let f = forger(&at, "forger", &net, &chain(&cp, 18, FLOOR, None, &root));
    let (code, v, err) = tips(&at, "victim", &net, &[&f]);
    // the first run now links to the checkpoint, and this chain cannot be linked: nothing is agreed, nothing stored
    assert_eq!(
        (code, v["verdict"].as_str()),
        (1, Some("not_trusted")),
        "{err}"
    );
    assert!(
        err.contains("must link the chain to the release checkpoint"),
        "{err}"
    );
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let node = format!("http://{}", l.local_addr().unwrap());
    let body = format!(
        "\"{}\"",
        proof.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { continue };
            let mut buf = [0u8; 8192];
            let _ = c.read(&mut buf);
            let _ = c.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
        }
    });
    let id_hex = String::from(id);
    let o = Command::new(BIN)
        .args([
            "wallet",
            "receive",
            "--state",
            &victim,
            "--node",
            &node,
            "--ids",
            &id_hex,
            "--net-file",
            &net,
            "--json",
        ])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null);
    let err = String::from_utf8_lossy(&o.stderr);
    let found = v["checked"]
        .as_array()
        .and_then(|a| a.iter().find(|c| c["id"] == id_hex.as_str()))
        .cloned()
        .unwrap_or_default();
    // with no agreed chain there is nothing to read a proof against: the forger's thousand ERG is not proven (it was,
    // with a confirmation, before the first run linked)
    assert_ne!(o.status.code(), Some(0), "{err}\n{v}");
    assert_ne!(found["state"].as_str(), Some("proven"), "{err}\n{v}");
}

#[test]
fn a_wallet_that_keeps_the_chain_does_not_fetch_it_again_from_the_checkpoint() {
    // `--keep-chain` went back to the checkpoint whenever fewer blocks than the agreed chain is long had come since the
    // last run (two review seats, one by running it): a serving wallet syncing every few minutes fetched everything again
    let at = dir("keep");
    let (cp, cp_json) = checkpoint();
    let net = net_file(&at, &cp_json);
    let f = forger(
        &at,
        "honest",
        &net,
        &chain(&cp, 18, CHECKPOINT_DIFFICULTY, None, &root(2)),
    );
    let (code, v, err) = tips_with(&at, "server", &net, &[&f], &["--keep-chain"]);
    assert_eq!(
        (code, &v["linked"]["headers"]),
        (0, &8.into()),
        "the first run links the eight below the ten judged: {err}"
    );
    // no new block since: nothing to fetch, and certainly not the chain from the checkpoint
    let (code, v, err) = tips_with(&at, "server", &net, &[&f], &["--keep-chain"]);
    assert_eq!((code, &v["linked"]["headers"]), (0, &0.into()), "{err}");
    assert!(
        v["linked"]["from_height"].as_u64().unwrap() > cp.height as u64,
        "from the kept chain, not the checkpoint: {v}"
    );
}

/// The headers in a chain file as `tips` writes it (two-byte length, then the header), or None if there is no file.
fn kept(at: &str) -> Option<usize> {
    let raw = std::fs::read(format!("{at}/linked-chain.bin")).ok()?;
    let (mut n, mut i) = (0, 0);
    while i + 2 <= raw.len() {
        i += 2 + u16::from_be_bytes([raw[i], raw[i + 1]]) as usize;
        n += 1;
    }
    Some(n)
}

#[test]
fn a_kept_chain_survives_a_later_run_whose_link_fails() {
    // A serving wallet (`--keep-chain`) whose link failed on a later run deleted its whole kept chain, and the next run
    // fetched it all again from the checkpoint, while a first run keeps what it linked so far. A later run whose link
    // fails keeps the chain it had.
    let at = dir("keep-fails");
    let (cp, cp_json) = checkpoint();
    let net = net_file(&at, &cp_json);
    let first = chain(&cp, 18, CHECKPOINT_DIFFICULTY, None, &root(2));
    // the same chain fifteen blocks later
    let mut longer = chain(&first[0], 15, CHECKPOINT_DIFFICULTY, None, &root(2));
    longer.extend(first.iter().cloned());
    let f = forger(&at, "honest", &net, &first);
    for st in ["grows", "fails"] {
        let (code, _, err) = tips_with(&at, st, &net, &[&f], &["--keep-chain"]);
        assert_eq!(
            (code, kept(&format!("{at}/{st}"))),
            (0, Some(18)),
            "the first run keeps the chain from the checkpoint: {err}"
        );
    }
    drop(f);
    // positive arm: a peer with the whole longer chain; the kept chain grows to it
    let whole = forger(&at, "whole", &net, &longer);
    let (code, _, err) = tips_with(&at, "grows", &net, &[&whole], &["--keep-chain"]);
    assert_eq!((code, kept(&format!("{at}/grows"))), (0, Some(33)), "{err}");
    drop(whole);
    // a peer that holds only the newest ten: the five between the kept chain and those cannot be linked this time
    let short = forger(&at, "short", &net, &longer[..10]);
    let (code, _, err) = tips_with(&at, "fails", &net, &[&short], &["--keep-chain"]);
    assert_eq!(code, 0, "the tip is still agreed: {err}");
    assert_eq!(
        kept(&format!("{at}/fails")),
        Some(18),
        "the chain kept before is still there after a link that failed: {err}"
    );
}

/// A peer that pushes a header one of whose 32 Autolykos table indexes is 0 made the library's `check_pow` panic
/// (<https://github.com/ergoplatform/sigma-rust/issues/964>): `tachys tips` ended with exit 70 at every run while that
/// peer was asked, and the phone app aborted. The header needs no work, only a scan of nonces (here mainnet's tip at
/// 1,915,819 with a nonce found by `mine_index_zero_nonces` in crates/tachys-verify/tests/pow.rs). It is now refused
/// for want of work, as any header without it.
#[test]
fn a_peer_pushing_a_header_with_a_table_index_of_0_is_refused_not_a_crash() {
    const ZERO_INDEX_NONCE: u64 = 20_713_447;
    let at = dir("index0");
    let (_, cp_json) = checkpoint();
    let net = net_file(&at, &cp_json);
    let p = ergo_nipopow::NipopowProof::scorex_parse_bytes(include_bytes!(
        "../../../vectors/mainnet-1915810.bin"
    ))
    .unwrap();
    let mut poison = p.suffix_tail.last().unwrap().clone();
    poison.autolykos_solution.nonce = ZERO_INDEX_NONCE.to_be_bytes().to_vec();
    let f = forger(&at, "poison", &net, &[sealed(&poison)]);
    let (code, v, err) = tips(&at, "victim", &net, &[&f]);
    assert_eq!(
        (code, v["verdict"].as_str()),
        (1, Some("not_trusted")),
        "{err}"
    );
    assert!(
        err.contains("(0 pass)"),
        "the header is pushed and refused: {err}"
    );
}

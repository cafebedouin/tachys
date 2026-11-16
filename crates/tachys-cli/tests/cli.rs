//! The command line as a program meets it: one JSON object on stdout with `--json`, sentences on stderr, an exit
//! code. Run against the built binary, with no network: where a node is needed, one on this machine stands in.
use std::io::{Read, Write};
use std::process::Command;
use std::str::FromStr;

use ergo_lib::chain::ergo_box::box_builder::ErgoBoxCandidateBuilder;
use ergo_lib::wallet::derivation_path::DerivationPath;
use ergo_lib::wallet::ext_secret_key::ExtSecretKey;
use ergo_lib::wallet::mnemonic::Mnemonic;
use ergotree_ir::chain::address::{Address, AddressEncoder, NetworkPrefix};
use ergotree_ir::chain::ergo_box::box_value::BoxValue;
use ergotree_ir::chain::ergo_box::ErgoBox;
use ergotree_ir::chain::tx_id::TxId;
use ergotree_ir::serialization::SigmaSerializable;
use serde_json::Value;
use sigma_ser::ScorexSerializable;

const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const ERG: u64 = 1_000_000_000;
const FEE: u64 = 1_100_000;

struct Ran { code: i32, out: String, err: String }
impl Ran {
    /// stdout as the one JSON object it must be
    fn json(&self) -> Value {
        assert_eq!(self.out.trim().lines().count(), 1, "exactly one line on stdout: {:?}", self.out);
        serde_json::from_str(&self.out).unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {:?}", self.out))
    }
}
fn tachys(args: &[&str]) -> Ran {
    let o = Command::new(env!("CARGO_BIN_EXE_tachys")).args(args).env_remove("TACHYS_PASS").output().unwrap();
    Ran { code: o.status.code().unwrap_or(-1), out: String::from_utf8_lossy(&o.stdout).to_string(), err: String::from_utf8_lossy(&o.stderr).to_string() }
}
/// A directory under the system's temporary one that is removed when the test is done with it, pass or fail.
struct Tmp(String);
impl std::ops::Deref for Tmp { type Target = str; fn deref(&self) -> &str { &self.0 } }
impl std::fmt::Display for Tmp { fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.write_str(&self.0) } }
impl Drop for Tmp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
fn dir(name: &str) -> Tmp {
    let d = std::env::temp_dir().join(format!("tachys-cli-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Tmp(d.to_string_lossy().to_string())
}
fn address(i: u64) -> Address {
    let m = ExtSecretKey::derive_master(Mnemonic::to_seed(PHRASE, "")).unwrap();
    Address::P2Pk(m.derive(DerivationPath::from_str(&format!("m/44'/429'/0'/0/{i}")).unwrap()).unwrap().public_image())
}
fn text(a: &Address) -> String { AddressEncoder::new(NetworkPrefix::Mainnet).address_to_str(a) }

/// A state directory with a wallet of the test phrase that tracks one box of ten ERG, and ten headers to sign under.
fn funded(name: &str) -> Tmp {
    let st = dir(name);
    let header: ergo_chain_types::Header = serde_json::from_str(include_str!("../../../anchors/mainnet.json")).unwrap();
    let bytes = header.scorex_serialize_bytes().unwrap();
    let mut chain = vec![];
    for _ in 0..10 { chain.extend_from_slice(&(bytes.len() as u16).to_be_bytes()); chain.extend_from_slice(&bytes); }
    std::fs::write(format!("{st}/agreed-chain.bin"), chain).unwrap();
    let me = address(0);
    let candidate = ErgoBoxCandidateBuilder::new(BoxValue::try_from(10 * ERG).unwrap(), me.script().unwrap(), header.height - 100).build().unwrap();
    let b = ErgoBox::from_box_candidate(&candidate, TxId::zero(), 0).unwrap();
    let id = String::from(b.box_id());
    let hex: String = b.sigma_serialize_bytes().unwrap().iter().map(|x| format!("{x:02x}")).collect();
    let w = serde_json::json!({ "name": "test", "address": text(&me), "address_list": [text(&me)], "birthday": 0, "mnemonic": PHRASE, "boxes": { id: { "value": 10 * ERG, "confirmed_at": 1, "bytes": hex } } });
    std::fs::write(format!("{st}/wallet.json"), w.to_string()).unwrap();
    st
}

/// A node's REST port on this machine that answers every request the same way. Returns its address and, when
/// the test is over, the request bodies it was sent.
fn node(status: u16, body: &'static str) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let s = seen.clone();
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { continue };
            let mut buf = vec![];
            let mut chunk = [0u8; 4096];
            // headers, then as many bytes of body as Content-Length says
            let body_of = |b: &[u8]| -> Option<String> {
                let t = String::from_utf8_lossy(b).to_string();
                let (head, rest) = t.split_once("\r\n\r\n")?;
                let want: usize = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
                (rest.len() >= want).then(|| rest.to_string())
            };
            let got = loop { match c.read(&mut chunk) { Ok(0) | Err(_) => break None, Ok(n) => { buf.extend_from_slice(&chunk[..n]); if let Some(b) = body_of(&buf) { break Some(b) } } } };
            if let Some(b) = got { s.lock().unwrap().push(b); }
            // status 0: a node that takes the request and never answers
            if status == 0 { continue }
            let _ = c.write_all(format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
        }
    });
    (url, seen)
}
fn dead_node() -> String { format!("http://{}", std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap()) }

#[test]
fn with_json_stdout_is_one_object_and_the_sentences_go_to_stderr() {
    let st = dir("new");
    let r = tachys(&["wallet", "new", "--state", &st, "--json"]);
    let j = r.json();
    assert_eq!((r.code, &j["ok"], &j["exit"], &j["v"], &j["command"]), (0, &Value::Bool(true), &0.into(), &1.into(), &"wallet new".into()));
    assert!(j["address"].as_str().unwrap().starts_with('9') && j["sealed"] == false && j.get("error").is_none());
    assert!(r.err.contains("new wallet: address") && !r.err.contains('{'), "the sentences are still said, on stderr");
    assert!(!r.out.contains(PHRASE) && !r.err.contains("abandon"), "and a new wallet never says its phrase");
    // the file it wrote carries the layout's version
    let w: Value = serde_json::from_str(&std::fs::read_to_string(format!("{st}/wallet.json")).unwrap()).unwrap();
    assert_eq!((&w["version"], &w["address"]), (&1.into(), &j["address"]));
    // without --json nothing changes: sentences on stdout, no object anywhere
    let plain = tachys(&["wallet", "address", "--state", &st]);
    assert_eq!((plain.code, plain.out.trim()), (0, format!("address 0: {}", j["address"].as_str().unwrap()).as_str()));
    let listed = tachys(&["wallet", "address", "--state", &st, "--json"]).json();
    assert_eq!(listed["addresses"], serde_json::json!([j["address"]]));
}

#[test]
fn a_refusal_is_ok_false_with_the_reason_and_the_exit_code() {
    let st = dir("refusals");
    assert_eq!(tachys(&["wallet", "new", "--state", &st, "--json"]).code, 0);
    // there already: misuse
    let again = tachys(&["wallet", "new", "--state", &st, "--json"]);
    let j = again.json();
    assert_eq!((again.code, &j["ok"], &j["exit"]), (2, &Value::Bool(false), &2.into()));
    assert!(j["error"].as_str().unwrap().contains("already exists"));
    // an option that does not exist, a value that cannot be read, a command that does not exist
    for (args, why) in [(vec!["wallet", "send", "--state", &st, "--nanoergs", "5", "--json"], "unknown option --nanoergs"),
        (vec!["tips", "--peers", "lots", "--json"], "--peers"), (vec!["wallet", "bogus", "--state", &st, "--json"], "no such command")] {
        let r = tachys(&args);
        assert_eq!(r.code, 2, "{args:?}");
        assert!(r.json()["error"].as_str().unwrap().contains(why), "{args:?}: {}", r.out);
    }
    // a wallet file from a newer build: refused in words, and left as it is
    let newer = r#"{ "version": 99, "address": "9abc", "boxes": {} }"#;
    std::fs::write(format!("{st}/wallet.json"), newer).unwrap();
    let r = tachys(&["wallet", "address", "--state", &st, "--json"]);
    assert_eq!(r.code, 1);
    assert!(r.json()["error"].as_str().unwrap().contains("newer tachys"));
    assert_eq!(std::fs::read_to_string(format!("{st}/wallet.json")).unwrap(), newer);
    // no tip: exit 1 and a verdict a program can read
    let empty = format!("{st}/no-seeds.txt");
    std::fs::write(&empty, "").unwrap();
    let r = tachys(&["tips", "--seeds", &empty, "--json"]);
    let j = r.json();
    assert_eq!((r.code, &j["ok"], &j["verdict"]), (1, &Value::Bool(false), &"not_trusted".into()));
}

#[test]
fn a_dry_run_reports_what_the_payment_would_do_and_signs_nothing() {
    let st = funded("dry");
    let payee = text(&address(1));
    let r = tachys(&["wallet", "send", "--state", &st, "--to", &payee, "--nanoerg", "2000000000", "--dry-run", "--json"]);
    let j = r.json();
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!((&j["signed"], &j["leaves_wallet_nanoerg"], &j["fee_nanoerg"], &j["payees"]), (&Value::Bool(false), &(2 * ERG + FEE).into(), &FEE.into(), &serde_json::json!([payee])));
    assert!(j.get("tx").is_none() && j.get("broadcast").is_none());
    assert!(!std::path::Path::new(&format!("{st}/last-tx.json")).exists(), "nothing was signed");
    // a dry run is held to the policy as a payment is, and counts nothing
    std::fs::write(format!("{st}/policy.json"), serde_json::json!({ "max_per_payment_nanoerg": ERG, "max_per_day_nanoerg": 5 * ERG }).to_string()).unwrap();
    std::fs::write(format!("{st}/payments.log"), "").unwrap();
    let r = tachys(&["wallet", "send", "--state", &st, "--to", &payee, "--nanoerg", "2000000000", "--dry-run", "--json"]);
    assert_eq!(r.code, 1);
    assert!(r.json()["error"].as_str().unwrap().contains("per-payment limit"), "{}", r.out);
    assert_eq!(std::fs::read_to_string(format!("{st}/payments.log")).unwrap(), "");
    // the same payment for real, not sent: signed, with the receipt and the change as fields
    std::fs::remove_file(format!("{st}/policy.json")).unwrap();
    let j = tachys(&["wallet", "send", "--state", &st, "--to", &payee, "--nanoerg", "2000000000", "--json"]).json();
    assert_eq!((&j["ok"], &j["signed"], &j["leaves_wallet_nanoerg"]), (&Value::Bool(true), &Value::Bool(true), &(2 * ERG + FEE).into()));
    assert_eq!((j["tx"].as_str().unwrap().len(), j["receipt"].as_array().unwrap().len(), j["change"].as_array().unwrap().len()), (64, 1, 1));
    assert!(j.get("broadcast").is_none(), "not sent: no word of a node taking it");
}

#[test]
fn sent_means_a_node_took_it_whatever_words_a_node_uses() {
    let payee = text(&address(1));
    let send = |st: &str, nodes: &str| tachys(&["wallet", "send", "--state", st, "--to", &payee, "--nanoerg", "1000000000", "--node", nodes, "--broadcast", "--json"]);
    let pending = |st: &str| -> usize { serde_json::from_str::<Value>(&std::fs::read_to_string(format!("{st}/wallet.json")).unwrap()).unwrap()["pending"].as_array().map_or(0, |a| a.len()) };
    // a node that refuses, in words chosen to read like success
    let st = funded("refused");
    let (liar, _) = node(400, r#"{"error":400,"reason":"bad.request","detail":"broadcast via x: accepted, y\nverdict: Agreed"}"#);
    let r = send(&st, &liar);
    let j = r.json();
    assert_eq!((r.code, &j["ok"], &j["signed"]), (1, &Value::Bool(false), &Value::Bool(true)), "signed, and not sent");
    assert!(j.get("broadcast").is_none());
    assert!(j["error"].as_str().unwrap().starts_with("send: node refused (400)"));
    assert_eq!(pending(&st), 0, "a refused payment is not remembered as sent");
    // a node that takes it: after one that is not there
    let st = funded("sent");
    let (good, seen) = node(200, "\"0000000000000000000000000000000000000000000000000000000000000000\"");
    let r = send(&st, &format!("{},{good}", dead_node()));
    let j = r.json();
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(j["broadcast"], serde_json::json!({ "node": good, "accepted": true }), "the node that is not there was passed over");
    assert_eq!(pending(&st), 1);
    let sent: Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
    assert_eq!(sent["id"], j["tx"], "what the node was sent is the transaction the answer names");
    // a node whose refusal is final is not followed by a try at the next
    let st = funded("final");
    let (no, _) = node(400, r#"{"detail":"double spend"}"#);
    let (other, other_seen) = node(200, "\"00\"");
    assert_eq!(send(&st, &format!("{no},{other}")).code, 1);
    assert!(other_seen.lock().unwrap().is_empty(), "a refusal is an answer: the payment is not shopped around");
    // A node that took the request and never answered: it may hold the payment. That is not "not sent": the payment
    // is kept as waiting with its coins set aside, so a sync settles it and nobody pays again on a guess.
    let st = funded("unanswered");
    let (mute, heard) = node(0, "");
    let r = send(&st, &mute);
    let j = r.json();
    assert_eq!((r.code, &j["ok"], &j["signed"], &j["outcome_unknown"]), (1, &Value::Bool(false), &Value::Bool(true), &Value::Bool(true)), "{}", r.out);
    assert!(j.get("broadcast").is_none() && j["error"].as_str().unwrap().contains("may have gone out"));
    assert_eq!(heard.lock().unwrap().len(), 1, "the node did get it");
    assert_eq!(pending(&st), 1, "remembered as waiting");
    let w: Value = serde_json::from_str(&std::fs::read_to_string(format!("{st}/wallet.json")).unwrap()).unwrap();
    assert_eq!(w["boxes"].as_object().unwrap().len(), 0, "and its coins are not there to spend a second time");
    assert_eq!(send(&st, &mute).code, 1, "so a second try has nothing to spend");
    assert_eq!(pending(&st), 1);
    // after a node that may hold it, another node's refusal ("already in the pool") is not the payment's refusal
    let st = funded("unanswered-then-refused");
    let (mute, _) = node(0, "");
    let (dup, _) = node(400, r#"{"detail":"already in the pool"}"#);
    let j = send(&st, &format!("{mute},{dup}")).json();
    assert_eq!((&j["outcome_unknown"], pending(&st)), (&Value::Bool(true), 1));
    // and one that takes it after the silent one settles it at once
    let st = funded("unanswered-then-taken");
    let (mute, _) = node(0, "");
    let (good, _) = node(200, "\"00\"");
    let r = send(&st, &format!("{mute},{good}"));
    assert_eq!((r.code, r.json()["broadcast"]["accepted"].clone()), (0, Value::Bool(true)));
    // a node that is not there at all: nothing left this machine, nothing is remembered, the coins stay
    let st = funded("nobody");
    let r = send(&st, &dead_node());
    assert_eq!((r.code, r.json().get("outcome_unknown").is_none(), pending(&st)), (1, true, 0));
    // a node in trouble (5xx) is passed over
    let st = funded("trouble");
    let (sick, _) = node(503, "busy");
    let (well, _) = node(200, "\"00\"");
    assert_eq!(send(&st, &format!("{sick},{well}")).json()["broadcast"]["node"], well.as_str());
}

#[test]
fn an_agent_can_get_a_wallet_and_learn_what_to_do_without_the_network() {
    // the recipes: one object, no wallet needed, every recipe a command this build knows
    let r = tachys(&["recipes"]);
    let j = r.json();
    assert_eq!((r.code, &j["ok"]), (0, &Value::Bool(true)));
    let known = ["setup", "address", "balance", "pay", "accept", "wait", "history"];
    for recipe in j["recipes"].as_array().unwrap() {
        let run = recipe["run"].as_str().unwrap();
        assert!(run.starts_with("tachys ") && known.contains(&run.split_whitespace().nth(1).unwrap()), "{run}");
        assert!(recipe["to"].as_str().is_some_and(|t| !t.is_empty()) && recipe["gives"].as_str().is_some());
        // every option a recipe names is one the build reads: a recipe cannot go stale without this failing
        for o in run.split_whitespace().filter(|w| w.starts_with("--")) { assert_ne!(tachys(&["recipes", o]).json()["refused"], format!("unknown option {o}").as_str(), "{o}"); assert_eq!(tachys(&["recipes", o]).code, 0, "{o}"); }
    }
    // setup makes the wallet once, and after that finds it
    let st = dir("agent");
    let first = tachys(&["setup", "--state", &st]).json();
    assert_eq!((&first["ok"], &first["created"], &first["can_pay"]), (&Value::Bool(true), &Value::Bool(true), &Value::Bool(false)));
    let again = tachys(&["setup", "--state", &st]).json();
    assert_eq!((&again["created"], &again["address"]), (&Value::Bool(false), &first["address"]));
    assert_eq!(tachys(&["address", "--state", &st]).json()["address"], first["address"]);
    let h = tachys(&["history", "--state", &st]).json();
    assert_eq!((&h["ok"], h["sent"].as_array().unwrap().len()), (&Value::Bool(true), 0));
    // nothing an agent command prints is ever the phrase
    let all = format!("{first}{again}{h}");
    let w: Value = serde_json::from_str(&std::fs::read_to_string(format!("{st}/wallet.json")).unwrap()).unwrap();
    assert!(!all.contains(w["mnemonic"].as_str().unwrap()));
    // misuse and refusals are one object too, with the code
    let r = tachys(&["pay", "--state", &st]);
    assert_eq!((r.code, r.json()["ok"].clone()), (2, Value::Bool(false)));
    let r = tachys(&["pay", "--state", &st, "--to", "9abc", "--erg", "1"]);
    assert_eq!(r.code, 1);
    assert!(r.json()["refused"].as_str().unwrap().contains("paying is off"));
    let r = tachys(&["wait", "--state", &st, "--tx", &"0".repeat(64)]);
    assert_eq!(r.code, 1);
    assert!(r.json()["refused"].as_str().unwrap().contains("no record"));
    let r = tachys(&["balance", "--state", &dir("no-wallet")]);
    assert!(r.code == 1 && r.json()["refused"].as_str().unwrap().contains("tachys setup"));
}

#[test]
fn an_agent_command_answers_in_one_shape_whatever_happens() {
    let st = dir("shapes");
    // every agent answer: one object, `v`, and `ok` true exactly when the exit code is 0
    for args in [vec!["recipes"], vec!["setup", "--state", &st], vec!["address", "--state", &st], vec!["history", "--state", &st]] {
        let r = tachys(&args);
        let j = r.json();
        assert_eq!((r.code, &j["ok"], &j["v"]), (0, &Value::Bool(true), &1.into()), "{args:?}");
    }
    // a refusal: `ok` false, a `code` to branch on, `refused` in words; and so for a slip caught before the command runs
    let other = dir("shapes-none");
    for (args, exit, code) in [(vec!["balance", "--state", &other], 1, "no_wallet"), (vec!["pay", "--state", &st, "--to", "9abc", "--erg", "1"], 1, "no_policy"),
        (vec!["pay", "--state", &st], 2, "misuse"), (vec!["address", "--state", &st, "--bogus"], 2, "misuse"), (vec!["setup", "--state", &st, "--network", "tesnet"], 2, "misuse"),
        (vec!["wait", "--state", &st, "--tx", "nonsense"], 2, "misuse"), (vec!["accept", "--state", &st, "--receipt", "short"], 2, "misuse")] {
        let r = tachys(&args);
        let j = r.json();
        assert_eq!((r.code, &j["ok"], &j["v"], j["code"].as_str()), (exit, &Value::Bool(false), &1.into(), Some(code)), "{args:?}: {}", r.out);
        assert!(j["refused"].as_str().is_some_and(|w| !w.is_empty()), "{args:?}");
    }
    // a mistyped network never makes a mainnet wallet
    let slip = dir("shapes-slip");
    assert_eq!(tachys(&["setup", "--state", &slip, "--network", "tesnet"]).code, 2);
    assert!(!std::path::Path::new(&format!("{slip}/wallet.json")).exists());
    assert_eq!(tachys(&["wallet", "new", "--state", &slip, "--network", "Testnet", "--json"]).code, 2);
    // a testnet wallet asked about as a mainnet one is refused, not answered with an address of the wrong network
    let t = dir("shapes-testnet");
    assert_eq!(tachys(&["setup", "--state", &t, "--network", "testnet"]).code, 0);
    let r = tachys(&["address", "--state", &t]);
    assert!(r.code == 1 && r.json()["refused"].as_str().unwrap().contains("--network"));
    // the names a program reads, held in place: taking one away or renaming it is a change of `v` (docs/json.md)
    let keys = |j: &Value| -> Vec<String> { let mut k: Vec<String> = j.as_object().unwrap().keys().cloned().collect(); k.sort(); k };
    assert_eq!(keys(&tachys(&["setup", "--state", &st]).json()), ["address", "can_pay", "created", "network", "note", "ok", "sealed", "v"]);
    assert_eq!(keys(&tachys(&["address", "--state", &st]).json()), ["address", "addresses", "network", "ok", "v"]);
    assert_eq!(keys(&tachys(&["history", "--state", &st]).json()), ["address", "expecting", "note", "ok", "payments_log", "sent", "v"]);
    assert_eq!(keys(&tachys(&["recipes"]).json()), ["about", "codes", "limits", "more", "ok", "recipes", "units", "v"]);
    assert_eq!(keys(&tachys(&["pay", "--state", &st, "--to", "9abc", "--erg", "1"]).json()), ["code", "ok", "refused", "v"]);
    assert_eq!(keys(&tachys(&["wallet", "address", "--state", &st, "--json"]).json()), ["addresses", "command", "exit", "ok", "v"]);
    // misuse the other commands used to let through
    for args in [vec!["tips", "--peers", "0"], vec!["tips", "--quorum", "0"], vec!["tips", "--depth", "0"], vec!["qr"], vec!["outbox", "--state", &st], vec!["token"], vec!["bogus"], vec![]] {
        assert_eq!(tachys(&args).code, 2, "{args:?}");
    }
    assert!(tachys(&["bogus"]).err.contains("no such command") && tachys(&[]).err.contains("usage: tachys COMMAND"));
    // a read-only wallet cannot sign, and a dry run does not say it could
    let w = dir("shapes-watch");
    let address = tachys(&["address", "--state", &st]).json()["address"].as_str().unwrap().to_string();
    assert_eq!(tachys(&["wallet", "watch", "--state", &w, "--address", &address]).code, 0);
    let with_chain = funded("shapes-chain");
    std::fs::write(format!("{w}/agreed-chain.bin"), std::fs::read(format!("{with_chain}/agreed-chain.bin")).unwrap()).unwrap();
    // (it holds no box either, so whichever refusal comes first, it is a refusal)
    assert_ne!(tachys(&["wallet", "send", "--state", &w, "--to", &address, "--nanoerg", "1000000", "--dry-run", "--json"]).code, 0);
    // the seed phrase, asked for as JSON, is in the object and nowhere else
    let r = tachys(&["wallet", "mnemonic", "--state", &st, "--json"]);
    let phrase = r.json()["mnemonic"].as_str().unwrap().to_string();
    assert!(phrase.split(' ').count() == 12 && !r.err.contains(&phrase));
}

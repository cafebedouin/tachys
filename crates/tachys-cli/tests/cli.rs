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

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const ERG: u64 = 1_000_000_000;
const FEE: u64 = 1_100_000;

struct Ran {
    code: i32,
    out: String,
    err: String,
}
impl Ran {
    /// stdout as the one JSON object it must be
    fn json(&self) -> Value {
        assert_eq!(
            self.out.trim().lines().count(),
            1,
            "exactly one line on stdout: {:?}",
            self.out
        );
        serde_json::from_str(&self.out)
            .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {:?}", self.out))
    }
}
fn tachys(args: &[&str]) -> Ran {
    let o = Command::new(env!("CARGO_BIN_EXE_tachys"))
        .args(args)
        .env_remove("TACHYS_PASS")
        .output()
        .unwrap();
    Ran {
        code: o.status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&o.stdout).to_string(),
        err: String::from_utf8_lossy(&o.stderr).to_string(),
    }
}
/// A directory under the system's temporary one that is removed when the test is done with it, pass or fail.
struct Tmp(String);
impl std::ops::Deref for Tmp {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Display for Tmp {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn dir(name: &str) -> Tmp {
    let d = std::env::temp_dir().join(format!("tachys-cli-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Tmp(d.to_string_lossy().to_string())
}
fn address(i: u64) -> Address {
    let m = ExtSecretKey::derive_master(Mnemonic::to_seed(PHRASE, "")).unwrap();
    Address::P2Pk(
        m.derive(DerivationPath::from_str(&format!("m/44'/429'/0'/0/{i}")).unwrap())
            .unwrap()
            .public_image(),
    )
}
fn text(a: &Address) -> String {
    AddressEncoder::new(NetworkPrefix::Mainnet).address_to_str(a)
}

/// A state directory with a wallet of the test phrase that tracks one box of ten ERG, and ten headers to sign under.
fn funded(name: &str) -> Tmp {
    let st = dir(name);
    let header: ergo_chain_types::Header =
        serde_json::from_str(include_str!("../../../anchors/mainnet.json")).unwrap();
    let bytes = header.scorex_serialize_bytes().unwrap();
    let mut chain = vec![];
    for _ in 0..10 {
        chain.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        chain.extend_from_slice(&bytes);
    }
    std::fs::write(format!("{st}/agreed-chain.bin"), chain).unwrap();
    let me = address(0);
    let candidate = ErgoBoxCandidateBuilder::new(
        BoxValue::try_from(10 * ERG).unwrap(),
        me.script().unwrap(),
        header.height - 100,
    )
    .build()
    .unwrap();
    let b = ErgoBox::from_box_candidate(&candidate, TxId::zero(), 0).unwrap();
    let id = String::from(b.box_id());
    let hex: String = b
        .sigma_serialize_bytes()
        .unwrap()
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect();
    let w = serde_json::json!({ "name": "test", "address": text(&me), "address_list": [text(&me)], "birthday": 0, "mnemonic": PHRASE, "boxes": { id: { "value": 10 * ERG, "confirmed_at": 1, "bytes": hex } } });
    std::fs::write(format!("{st}/wallet.json"), w.to_string()).unwrap();
    st
}

/// A node's REST port on this machine that answers every request the same way. Returns its address and, when
/// the test is over, the request bodies it was sent.
fn node(
    status: u16,
    body: &'static str,
) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
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
                let want: usize = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse().unwrap_or(0))
                    })
                    .unwrap_or(0);
                (rest.len() >= want).then(|| rest.to_string())
            };
            let got = loop {
                match c.read(&mut chunk) {
                    Ok(0) | Err(_) => break None,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(b) = body_of(&buf) {
                            break Some(b);
                        }
                    }
                }
            };
            if let Some(b) = got {
                s.lock().unwrap().push(b);
            }
            // status 0: a node that takes the request and never answers
            if status == 0 {
                continue;
            }
            let _ = c.write_all(format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
        }
    });
    (url, seen)
}
fn dead_node() -> String {
    format!(
        "http://{}",
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
    )
}

#[test]
fn with_json_stdout_is_one_object_and_the_sentences_go_to_stderr() {
    let st = dir("new");
    let r = tachys(&["wallet", "new", "--state", &st, "--json"]);
    let j = r.json();
    assert_eq!(
        (r.code, &j["ok"], &j["exit"], &j["v"], &j["command"]),
        (
            0,
            &Value::Bool(true),
            &0.into(),
            &1.into(),
            &"wallet new".into()
        )
    );
    assert!(
        j["address"].as_str().unwrap().starts_with('9')
            && j["sealed"] == false
            && j.get("error").is_none()
    );
    assert!(
        r.err.contains("new wallet: address") && !r.err.contains('{'),
        "the sentences are still said, on stderr"
    );
    assert!(
        !r.out.contains(PHRASE) && !r.err.contains("abandon"),
        "and a new wallet never says its phrase"
    );
    // the file it wrote carries the layout's version
    let w: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{st}/wallet.json")).unwrap())
            .unwrap();
    assert_eq!((&w["version"], &w["address"]), (&1.into(), &j["address"]));
    // without --json nothing changes: sentences on stdout, no object anywhere
    let plain = tachys(&["wallet", "address", "--state", &st]);
    assert_eq!(
        (plain.code, plain.out.trim()),
        (
            0,
            format!("address 0: {}", j["address"].as_str().unwrap()).as_str()
        )
    );
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
    assert_eq!(
        (again.code, &j["ok"], &j["exit"]),
        (2, &Value::Bool(false), &2.into())
    );
    assert!(j["error"].as_str().unwrap().contains("already exists"));
    // an option that does not exist, a value that cannot be read, a command that does not exist
    for (args, why) in [
        (
            vec![
                "wallet",
                "send",
                "--state",
                &st,
                "--nanoergs",
                "5",
                "--json",
            ],
            "unknown option --nanoergs",
        ),
        (vec!["tips", "--peers", "lots", "--json"], "--peers"),
        (
            vec!["wallet", "bogus", "--state", &st, "--json"],
            "no such command",
        ),
    ] {
        let r = tachys(&args);
        assert_eq!(r.code, 2, "{args:?}");
        assert!(
            r.json()["error"].as_str().unwrap().contains(why),
            "{args:?}: {}",
            r.out
        );
    }
    // a wallet file from a newer build: refused in words, and left as it is
    let newer = r#"{ "version": 99, "address": "9abc", "boxes": {} }"#;
    std::fs::write(format!("{st}/wallet.json"), newer).unwrap();
    let r = tachys(&["wallet", "address", "--state", &st, "--json"]);
    assert_eq!(r.code, 1);
    assert!(r.json()["error"].as_str().unwrap().contains("newer tachys"));
    assert_eq!(
        std::fs::read_to_string(format!("{st}/wallet.json")).unwrap(),
        newer
    );
    // no tip: exit 1 and a verdict a program can read
    let empty = format!("{st}/no-seeds.txt");
    std::fs::write(&empty, "").unwrap();
    let r = tachys(&["tips", "--seeds", &empty, "--json"]);
    let j = r.json();
    assert_eq!(
        (r.code, &j["ok"], &j["verdict"]),
        (1, &Value::Bool(false), &"not_trusted".into())
    );
}

#[test]
fn a_dry_run_reports_what_the_payment_would_do_and_signs_nothing() {
    let st = funded("dry");
    let payee = text(&address(1));
    let r = tachys(&[
        "wallet",
        "send",
        "--state",
        &st,
        "--to",
        &payee,
        "--nanoerg",
        "2000000000",
        "--dry-run",
        "--json",
    ]);
    let j = r.json();
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        (
            &j["signed"],
            &j["leaves_wallet_nanoerg"],
            &j["fee_nanoerg"],
            &j["payees"]
        ),
        (
            &Value::Bool(false),
            &(2 * ERG + FEE).into(),
            &FEE.into(),
            &serde_json::json!([payee])
        )
    );
    assert!(j.get("tx").is_none() && j.get("broadcast").is_none());
    assert!(
        !std::path::Path::new(&format!("{st}/last-tx.json")).exists(),
        "nothing was signed"
    );
    // a dry run is held to the policy as a payment is, and counts nothing
    std::fs::write(
        format!("{st}/policy.json"),
        serde_json::json!({ "max_per_payment_nanoerg": ERG, "max_per_day_nanoerg": 5 * ERG })
            .to_string(),
    )
    .unwrap();
    std::fs::write(format!("{st}/payments.log"), "").unwrap();
    let r = tachys(&[
        "wallet",
        "send",
        "--state",
        &st,
        "--to",
        &payee,
        "--nanoerg",
        "2000000000",
        "--dry-run",
        "--json",
    ]);
    assert_eq!(r.code, 1);
    assert!(
        r.json()["error"]
            .as_str()
            .unwrap()
            .contains("per-payment limit"),
        "{}",
        r.out
    );
    assert_eq!(
        std::fs::read_to_string(format!("{st}/payments.log")).unwrap(),
        ""
    );
    // the same payment for real, not sent: signed, with the receipt and the change as fields
    std::fs::remove_file(format!("{st}/policy.json")).unwrap();
    let j = tachys(&[
        "wallet",
        "send",
        "--state",
        &st,
        "--to",
        &payee,
        "--nanoerg",
        "2000000000",
        "--json",
    ])
    .json();
    assert_eq!(
        (&j["ok"], &j["signed"], &j["leaves_wallet_nanoerg"]),
        (
            &Value::Bool(true),
            &Value::Bool(true),
            &(2 * ERG + FEE).into()
        )
    );
    assert_eq!(
        (
            j["tx"].as_str().unwrap().len(),
            j["receipt"].as_array().unwrap().len(),
            j["change"].as_array().unwrap().len()
        ),
        (64, 1, 1)
    );
    assert!(
        j.get("broadcast").is_none(),
        "not sent: no word of a node taking it"
    );
}

#[test]
fn a_carried_payment_whose_stored_bytes_are_not_its_own_serialisation_is_set_aside_and_never_sent()
{
    let payer = funded("canon-payer");
    let parcel = format!("{payer}/parcel.json");
    assert_eq!(
        tachys(&[
            "wallet",
            "send",
            "--state",
            &payer,
            "--to",
            &text(&address(1)),
            "--nanoerg",
            "2000000000",
            "--parcel-out",
            &parcel,
            "--json"
        ])
        .code,
        0
    );
    let good: Value = serde_json::from_slice(&std::fs::read(&parcel).unwrap()).unwrap();
    let id = good["tx_id"].as_str().unwrap().to_string();
    // the same transaction with one byte after its end: it still reads as that transaction, with that id
    let mut bad = good.clone();
    bad["tx_bytes"] = format!("{}00", good["tx_bytes"].as_str().unwrap()).into();
    // a carrier with no chain of its own and a peer that is not there: nothing here can reach a network
    let peer = dead_node().replace("http://", "");
    let carrier = dir("canon-carrier");
    std::fs::create_dir_all(format!("{carrier}/outbox")).unwrap();
    std::fs::write(format!("{carrier}/outbox/{id}.json"), bad.to_string()).unwrap();
    let r = tachys(&["outbox", "--state", &carrier, "--peer", &peer, "--json"]);
    let j = r.json();
    assert_eq!(
        (r.code, &j["submitted"], &j["refused"]),
        (0, &0.into(), &1.into()),
        "{}",
        r.err
    );
    assert!(
        r.err.contains("not the transaction's own serialisation"),
        "{}",
        r.err
    );
    assert!(
        !std::path::Path::new(&format!("{carrier}/outbox/{id}.json")).exists(),
        "out of the queue"
    );
    assert_eq!(
        std::fs::read_to_string(format!("{carrier}/given-up/{id}.json")).unwrap(),
        bad.to_string(),
        "and kept as it was, not deleted"
    );
    // the payment as it was written passes this check and waits for the ones that need a chain
    std::fs::write(format!("{carrier}/outbox/{id}.json"), good.to_string()).unwrap();
    let r = tachys(&["outbox", "--state", &carrier, "--peer", &peer, "--json"]);
    assert_eq!((r.code, &r.json()["refused"]), (0, &0.into()), "{}", r.err);
    assert!(
        r.err.contains("could not be checked")
            && std::path::Path::new(&format!("{carrier}/outbox/{id}.json")).exists(),
        "{}",
        r.err
    );
}

#[test]
fn sent_means_a_node_took_it_whatever_words_a_node_uses() {
    let payee = text(&address(1));
    let send = |st: &str, nodes: &str| {
        tachys(&[
            "wallet",
            "send",
            "--state",
            st,
            "--to",
            &payee,
            "--nanoerg",
            "1000000000",
            "--node",
            nodes,
            "--broadcast",
            "--json",
        ])
    };
    let pending = |st: &str| -> usize {
        serde_json::from_str::<Value>(
            &std::fs::read_to_string(format!("{st}/wallet.json")).unwrap(),
        )
        .unwrap()["pending"]
            .as_array()
            .map_or(0, |a| a.len())
    };
    // a node that refuses, in words chosen to read like success
    let st = funded("refused");
    let (liar, _) = node(
        400,
        r#"{"error":400,"reason":"bad.request","detail":"broadcast via x: accepted, y\nverdict: Agreed"}"#,
    );
    let r = send(&st, &liar);
    let j = r.json();
    assert_eq!(
        (r.code, &j["ok"], &j["signed"]),
        (1, &Value::Bool(false), &Value::Bool(true)),
        "signed, and not sent"
    );
    assert!(j.get("broadcast").is_none());
    assert!(j["error"]
        .as_str()
        .unwrap()
        .starts_with("send: node refused (400)"));
    assert_eq!(
        pending(&st),
        0,
        "a refused payment is not remembered as sent"
    );
    // a node that takes it: after one that is not there
    let st = funded("sent");
    let (good, seen) = node(
        200,
        "\"0000000000000000000000000000000000000000000000000000000000000000\"",
    );
    let r = send(&st, &format!("{},{good}", dead_node()));
    let j = r.json();
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        j["broadcast"],
        serde_json::json!({ "node": good, "accepted": true }),
        "the node that is not there was passed over"
    );
    assert_eq!(pending(&st), 1);
    let sent: Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
    assert_eq!(
        sent["id"], j["tx"],
        "what the node was sent is the transaction the answer names"
    );
    // a node whose refusal is final is not followed by a try at the next
    let st = funded("final");
    let (no, _) = node(400, r#"{"detail":"double spend"}"#);
    let (other, other_seen) = node(200, "\"00\"");
    assert_eq!(send(&st, &format!("{no},{other}")).code, 1);
    assert!(
        other_seen.lock().unwrap().is_empty(),
        "a refusal is an answer: the payment is not shopped around"
    );
    // A node that took the request and never answered: it may hold the payment. That is not "not sent": the payment
    // is kept as waiting with its coins set aside, so a sync settles it and nobody pays again on a guess.
    let st = funded("unanswered");
    let (mute, heard) = node(0, "");
    let r = send(&st, &mute);
    let j = r.json();
    assert_eq!(
        (r.code, &j["ok"], &j["signed"], &j["outcome_unknown"]),
        (
            1,
            &Value::Bool(false),
            &Value::Bool(true),
            &Value::Bool(true)
        ),
        "{}",
        r.out
    );
    assert!(
        j.get("broadcast").is_none() && j["error"].as_str().unwrap().contains("may have gone out")
    );
    assert_eq!(heard.lock().unwrap().len(), 1, "the node did get it");
    assert_eq!(pending(&st), 1, "remembered as waiting");
    let w: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{st}/wallet.json")).unwrap())
            .unwrap();
    assert_eq!(
        w["boxes"].as_object().unwrap().len(),
        0,
        "and its coins are not there to spend a second time"
    );
    assert_eq!(
        send(&st, &mute).code,
        1,
        "so a second try has nothing to spend"
    );
    assert_eq!(pending(&st), 1);
    // after a node that may hold it, another node's refusal ("already in the pool") is not the payment's refusal
    let st = funded("unanswered-then-refused");
    let (mute, _) = node(0, "");
    let (dup, _) = node(400, r#"{"detail":"already in the pool"}"#);
    let j = send(&st, &format!("{mute},{dup}")).json();
    assert_eq!(
        (&j["outcome_unknown"], pending(&st)),
        (&Value::Bool(true), 1)
    );
    // and one that takes it after the silent one settles it at once
    let st = funded("unanswered-then-taken");
    let (mute, _) = node(0, "");
    let (good, _) = node(200, "\"00\"");
    let r = send(&st, &format!("{mute},{good}"));
    assert_eq!(
        (r.code, r.json()["broadcast"]["accepted"].clone()),
        (0, Value::Bool(true))
    );
    // a node that is not there at all: nothing left this machine, nothing is remembered, the coins stay
    let st = funded("nobody");
    let r = send(&st, &dead_node());
    assert_eq!(
        (
            r.code,
            r.json().get("outcome_unknown").is_none(),
            pending(&st)
        ),
        (1, true, 0)
    );
    // a node in trouble (5xx) is passed over
    let st = funded("trouble");
    let (sick, _) = node(503, "busy");
    let (well, _) = node(200, "\"00\"");
    assert_eq!(
        send(&st, &format!("{sick},{well}")).json()["broadcast"]["node"],
        well.as_str()
    );
}

#[test]
fn an_agent_can_get_a_wallet_and_learn_what_to_do_without_the_network() {
    // the recipes: one object, no wallet needed, every recipe a command this build knows
    let r = tachys(&["recipes"]);
    let j = r.json();
    assert_eq!((r.code, &j["ok"]), (0, &Value::Bool(true)));
    let known = [
        "setup", "address", "balance", "pay", "accept", "wait", "history",
    ];
    for recipe in j["recipes"].as_array().unwrap() {
        let run = recipe["run"].as_str().unwrap();
        assert!(
            run.starts_with("tachys ") && known.contains(&run.split_whitespace().nth(1).unwrap()),
            "{run}"
        );
        assert!(
            recipe["to"].as_str().is_some_and(|t| !t.is_empty())
                && recipe["gives"].as_str().is_some()
        );
        // every option a recipe names is one the build reads: a recipe cannot go stale without this failing
        for o in run.split_whitespace().filter(|w| w.starts_with("--")) {
            assert_ne!(
                tachys(&["recipes", o]).json()["refused"],
                format!("unknown option {o}").as_str(),
                "{o}"
            );
            assert_eq!(tachys(&["recipes", o]).code, 0, "{o}");
        }
    }
    // setup makes the wallet once, and after that finds it
    let st = dir("agent");
    let first = tachys(&["setup", "--state", &st]).json();
    assert_eq!(
        (&first["ok"], &first["created"], &first["can_pay"]),
        (&Value::Bool(true), &Value::Bool(true), &Value::Bool(false))
    );
    let again = tachys(&["setup", "--state", &st]).json();
    assert_eq!(
        (&again["created"], &again["address"]),
        (&Value::Bool(false), &first["address"])
    );
    assert_eq!(
        tachys(&["address", "--state", &st]).json()["address"],
        first["address"]
    );
    let h = tachys(&["history", "--state", &st]).json();
    assert_eq!(
        (&h["ok"], h["sent"].as_array().unwrap().len()),
        (&Value::Bool(true), 0)
    );
    // nothing an agent command prints is ever the phrase
    let all = format!("{first}{again}{h}");
    let w: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{st}/wallet.json")).unwrap())
            .unwrap();
    assert!(!all.contains(w["mnemonic"].as_str().unwrap()));
    // misuse and refusals are one object too, with the code
    let r = tachys(&["pay", "--state", &st]);
    assert_eq!((r.code, r.json()["ok"].clone()), (2, Value::Bool(false)));
    let r = tachys(&["pay", "--state", &st, "--to", "9abc", "--erg", "1"]);
    assert_eq!(r.code, 1);
    assert!(r.json()["refused"]
        .as_str()
        .unwrap()
        .contains("paying is off"));
    let r = tachys(&["wait", "--state", &st, "--tx", &"0".repeat(64)]);
    assert_eq!(r.code, 1);
    assert!(r.json()["refused"].as_str().unwrap().contains("no record"));
    let r = tachys(&["balance", "--state", &dir("no-wallet")]);
    assert!(
        r.code == 1
            && r.json()["refused"]
                .as_str()
                .unwrap()
                .contains("tachys setup")
    );
}

#[test]
fn an_agent_command_answers_in_one_shape_whatever_happens() {
    let st = dir("shapes");
    // every agent answer: one object, `v`, and `ok` true exactly when the exit code is 0
    for args in [
        vec!["recipes"],
        vec!["setup", "--state", &st],
        vec!["address", "--state", &st],
        vec!["history", "--state", &st],
    ] {
        let r = tachys(&args);
        let j = r.json();
        assert_eq!(
            (r.code, &j["ok"], &j["v"]),
            (0, &Value::Bool(true), &1.into()),
            "{args:?}"
        );
    }
    // a refusal: `ok` false, a `code` to branch on, `refused` in words; and so for a slip caught before the command runs
    let other = dir("shapes-none");
    for (args, exit, code) in [
        (vec!["balance", "--state", &other], 1, "no_wallet"),
        (
            vec!["pay", "--state", &st, "--to", "9abc", "--erg", "1"],
            1,
            "no_policy",
        ),
        (vec!["pay", "--state", &st], 2, "misuse"),
        (vec!["address", "--state", &st, "--bogus"], 2, "misuse"),
        (
            vec!["setup", "--state", &st, "--network", "tesnet"],
            2,
            "misuse",
        ),
        (
            vec!["wait", "--state", &st, "--tx", "nonsense"],
            2,
            "misuse",
        ),
        (
            vec!["accept", "--state", &st, "--receipt", "short"],
            2,
            "misuse",
        ),
    ] {
        let r = tachys(&args);
        let j = r.json();
        assert_eq!(
            (r.code, &j["ok"], &j["v"], j["code"].as_str()),
            (exit, &Value::Bool(false), &1.into(), Some(code)),
            "{args:?}: {}",
            r.out
        );
        assert!(
            j["refused"].as_str().is_some_and(|w| !w.is_empty()),
            "{args:?}"
        );
    }
    // a mistyped network never makes a mainnet wallet
    let slip = dir("shapes-slip");
    assert_eq!(
        tachys(&["setup", "--state", &slip, "--network", "tesnet"]).code,
        2
    );
    assert!(!std::path::Path::new(&format!("{slip}/wallet.json")).exists());
    assert_eq!(
        tachys(&[
            "wallet",
            "new",
            "--state",
            &slip,
            "--network",
            "Testnet",
            "--json"
        ])
        .code,
        2
    );
    // a testnet wallet asked about as a mainnet one is refused, not answered with an address of the wrong network
    let t = dir("shapes-testnet");
    assert_eq!(
        tachys(&["setup", "--state", &t, "--network", "testnet"]).code,
        0
    );
    let r = tachys(&["address", "--state", &t]);
    assert!(r.code == 1 && r.json()["refused"].as_str().unwrap().contains("--network"));
    // the names a program reads, held in place: taking one away or renaming it is a change of `v` (docs/json.md)
    let keys = |j: &Value| -> Vec<String> {
        let mut k: Vec<String> = j.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    };
    assert_eq!(
        keys(&tachys(&["setup", "--state", &st]).json()),
        ["address", "can_pay", "created", "network", "note", "ok", "sealed", "v"]
    );
    assert_eq!(
        keys(&tachys(&["address", "--state", &st]).json()),
        ["address", "addresses", "network", "ok", "v"]
    );
    assert_eq!(
        keys(&tachys(&["history", "--state", &st]).json()),
        [
            "address",
            "expecting",
            "note",
            "ok",
            "payments_log",
            "sent",
            "v"
        ]
    );
    assert_eq!(
        keys(&tachys(&["recipes"]).json()),
        ["about", "codes", "limits", "more", "ok", "recipes", "units", "v"]
    );
    assert_eq!(
        keys(&tachys(&["pay", "--state", &st, "--to", "9abc", "--erg", "1"]).json()),
        ["code", "ok", "refused", "v"]
    );
    assert_eq!(
        keys(&tachys(&["wallet", "address", "--state", &st, "--json"]).json()),
        ["addresses", "command", "exit", "ok", "v"]
    );
    // misuse the other commands used to let through
    for args in [
        vec!["tips", "--peers", "0"],
        vec!["tips", "--quorum", "0"],
        vec!["tips", "--depth", "0"],
        vec!["qr"],
        vec!["outbox", "--state", &st],
        vec!["token"],
        vec!["bogus"],
        vec![],
    ] {
        assert_eq!(tachys(&args).code, 2, "{args:?}");
    }
    assert!(
        tachys(&["bogus"]).err.contains("no such command")
            && tachys(&[]).err.contains("usage: tachys COMMAND")
    );
    // a read-only wallet cannot sign, and a dry run does not say it could
    let w = dir("shapes-watch");
    let address = tachys(&["address", "--state", &st]).json()["address"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        tachys(&["wallet", "watch", "--state", &w, "--address", &address]).code,
        0
    );
    let with_chain = funded("shapes-chain");
    std::fs::write(
        format!("{w}/agreed-chain.bin"),
        std::fs::read(format!("{with_chain}/agreed-chain.bin")).unwrap(),
    )
    .unwrap();
    // (it holds no box either, so whichever refusal comes first, it is a refusal)
    assert_ne!(
        tachys(&[
            "wallet",
            "send",
            "--state",
            &w,
            "--to",
            &address,
            "--nanoerg",
            "1000000",
            "--dry-run",
            "--json"
        ])
        .code,
        0
    );
    // the seed phrase, asked for as JSON, is in the object and nowhere else
    let r = tachys(&["wallet", "mnemonic", "--state", &st, "--json"]);
    let phrase = r.json()["mnemonic"].as_str().unwrap().to_string();
    assert!(phrase.split(' ').count() == 12 && !r.err.contains(&phrase));
}

#[test]
fn a_node_that_answers_with_a_proof_nested_too_deep_is_refused_and_the_wallet_carries_on() {
    // A proof 100,000 levels deep (3.4 MB, under what a node's answer may be): before the proof was walked first,
    // labelling it overflowed the stack and the process aborted, in every build.
    let st = funded("deep-proof");
    // the stored chain must be recent for a proof to be asked for at all; nothing checks its work once it is stored
    let mut h: ergo_chain_types::Header =
        serde_json::from_str(include_str!("../../../anchors/mainnet.json")).unwrap();
    h.timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let b = h.scorex_serialize_bytes().unwrap();
    let mut chain = vec![];
    for _ in 0..10 {
        chain.extend_from_slice(&(b.len() as u16).to_be_bytes());
        chain.extend_from_slice(&b);
    }
    std::fs::write(format!("{st}/agreed-chain.bin"), chain).unwrap();
    let mut p = vec![3u8];
    p.extend([7u8; 32]);
    for _ in 0..100_000 {
        p.push(3);
        p.extend([8u8; 32]);
        p.push(0);
    }
    p.push(4);
    p.extend([0u8; 64]);
    let body: &'static str = Box::leak(
        format!(
            "\"{}\"",
            p.iter().map(|x| format!("{x:02x}")).collect::<String>()
        )
        .into_boxed_str(),
    );
    let (url, _) = node(200, body);
    let r = tachys(&[
        "wallet",
        "receive",
        "--state",
        &st,
        "--node",
        &url,
        "--ids",
        &"ab".repeat(32),
        "--json",
    ]);
    assert_eq!(r.code, 1, "{}", r.err);
    assert_eq!(r.json()["ok"], false, "{}", r.out);
}

/// Ten headers stored as the agreed chain: the release's checkpoint, made recent, with `root` as its state root, and
/// agreed at its height, so that a proof against `root` is settled. Returns the header.
fn chain_over(st: &str, root: &[u8]) -> ergo_chain_types::Header {
    chain_over_at(st, root, 0)
}
/// The same, the header `lower` heights below the checkpoint's: a chain as a node that is behind would show it.
fn chain_over_at(st: &str, root: &[u8], lower: u32) -> ergo_chain_types::Header {
    let mut h: ergo_chain_types::Header =
        serde_json::from_str(include_str!("../../../anchors/mainnet.json")).unwrap();
    h.height -= lower;
    h.timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    h.state_root = ergo_chain_types::ADDigest::try_from(root.to_vec()).unwrap();
    let b = h.scorex_serialize_bytes().unwrap();
    let h = ergo_chain_types::Header::scorex_parse_bytes(&b).unwrap();
    let mut chain = vec![];
    for _ in 0..10 {
        chain.extend_from_slice(&(b.len() as u16).to_be_bytes());
        chain.extend_from_slice(&b);
    }
    std::fs::write(format!("{st}/agreed-chain.bin"), chain).unwrap();
    std::fs::write(format!("{st}/agreed-tip.bin"), &b).unwrap();
    h
}
/// A state tree holding `held` (box bytes, each filed under its hash) and one box of filler.
fn tree(held: &[Vec<u8>]) -> ergo_avltree_rust::batch_avl_prover::BatchAVLProver {
    use ergo_avltree_rust::batch_node::{AVLTree, Node, NodeHeader};
    use ergo_avltree_rust::operation::{KeyValue, Operation};
    let mut p = ergo_avltree_rust::batch_avl_prover::BatchAVLProver::new(
        AVLTree::new(
            |d| Node::LabelOnly(NodeHeader::new(Some(*d), None)),
            32,
            None,
        ),
        true,
    );
    for v in held.iter().cloned().chain(std::iter::once(vec![7u8; 60])) {
        let k = sigma_util::hash::blake2b256_hash(&v);
        p.perform_one_operation(&Operation::Insert(KeyValue {
            key: bytes::Bytes::copy_from_slice(k.as_ref()),
            value: bytes::Bytes::from(v),
        }))
        .unwrap();
    }
    p.generate_proof();
    p
}
/// A node on this machine with the state tree of `held`: it proves what it is asked from that tree, refuses every
/// payment as a double spend, and has nothing else. Returns its address and the tree's root.
fn tree_node(held: Vec<Vec<u8>>) -> (String, Vec<u8>) {
    tree_node_with(held, None)
}
/// The same, with a payment in its pool of payments not yet in a block (the transaction's id and its JSON), served as a
/// node serves `/transactions/unconfirmed/...`.
fn tree_node_with(held: Vec<Vec<u8>>, pool: Option<(String, String)>) -> (String, Vec<u8>) {
    tree_node_full(held, pool, None)
}
/// The same, also listing one address's unspent boxes (the address, a box id), as `/blockchain/box/unspent/byAddress`
/// does; every other address holds nothing.
fn tree_node_full(
    held: Vec<Vec<u8>>,
    pool: Option<(String, String)>,
    listed: Option<(String, String)>,
) -> (String, Vec<u8>) {
    use ergo_avltree_rust::authenticated_tree_ops::AuthenticatedTreeOps;
    use ergo_avltree_rust::operation::Operation;
    let root = tree(&held).digest().unwrap().to_vec();
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { continue };
            let (mut buf, mut chunk) = (vec![], [0u8; 4096]);
            let (head, body) = loop {
                match c.read(&mut chunk) {
                    Ok(0) | Err(_) => break (String::new(), String::new()),
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
                let t = String::from_utf8_lossy(&buf).to_string();
                if let Some((h, b)) = t.split_once("\r\n\r\n") {
                    let want: usize = h
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if b.len() >= want {
                        break (h.to_string(), b.to_string());
                    }
                }
            };
            let (status, answer) = if head.contains("/blockchain/box/unspent/byAddress") {
                match &listed {
                    Some((a, b)) if body.trim() == a => (200, format!(r#"[{{"boxId":"{b}"}}]"#)),
                    _ => (200, "[]".to_string()),
                }
            } else if head.contains("/utxo/getBoxesBinaryProof") {
                let ids: Vec<String> = serde_json::from_str(&body).unwrap_or_default();
                let mut p = tree(&held);
                for id in &ids {
                    let k: Vec<u8> = (0..id.len() / 2)
                        .map(|i| u8::from_str_radix(&id[2 * i..2 * i + 2], 16).unwrap())
                        .collect();
                    p.perform_one_operation(&Operation::Lookup(bytes::Bytes::from(k)))
                        .unwrap();
                }
                (
                    200,
                    format!(
                        "\"{}\"",
                        p.generate_proof()
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>()
                    ),
                )
            } else if let (true, Some((txid, json))) =
                (head.contains("/transactions/unconfirmed/"), &pool)
            {
                // the pool: the payment's outputs by box id, and the payment by its id
                let tx: Value = serde_json::from_str(json).unwrap();
                let line = head.lines().next().unwrap_or("");
                match tx["outputs"].as_array().unwrap().iter().find(|o| {
                    line.contains(&format!(
                        "/outputs/byBoxId/{}",
                        o["boxId"].as_str().unwrap()
                    ))
                }) {
                    Some(o) => {
                        let mut o = o.clone();
                        o["transactionId"] = txid.clone().into();
                        (200, o.to_string())
                    }
                    None if line.contains(&format!("/byTransactionId/{txid}")) => {
                        (200, json.clone())
                    }
                    None => (404, r#"{"error":404}"#.to_string()),
                }
            } else if head.contains("/transactions/unconfirmed/") {
                (404, r#"{"error":404}"#.to_string())
            } else if head.contains("/transactions") {
                (
                    400,
                    r#"{"error":400,"reason":"bad.request","detail":"double spend"}"#.to_string(),
                )
            } else {
                (404, "{}".to_string())
            };
            let _ = c.write_all(format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len()).as_bytes());
        }
    });
    (url, root)
}
fn state_of(r: &Ran, id: &str) -> Option<String> {
    r.json()["checked"]
        .as_array()?
        .iter()
        .find(|c| c["id"] == id)
        .and_then(|c| c["state"].as_str())
        .map(String::from)
}

#[test]
fn a_promised_box_whose_payment_can_never_confirm_is_void_not_expected_for_ever_and_never_deleted()
{
    // Seen on the rig (`rig/dishonest-payer.sh`): a payee held a signed payment; the payer spent the same coin first.
    // The payee's own outbox proved the coin gone and dropped the payment, but the box it promised stayed "expected",
    // so `accept` and `wait --receipt` answered "not yet" for ever.
    let payer = funded("void-payer");
    let payee = dir("void-payee");
    let to = tachys(&["wallet", "new", "--state", &payee, "--json"]).json()["address"]
        .as_str()
        .unwrap()
        .to_string();
    // the payer's coin, read before it pays
    let w: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{payer}/wallet.json")).unwrap())
            .unwrap();
    let coin = w["boxes"].as_object().unwrap().values().next().unwrap()["bytes"]
        .as_str()
        .unwrap()
        .to_string();
    let coin: Vec<u8> = (0..coin.len() / 2)
        .map(|i| u8::from_str_radix(&coin[2 * i..2 * i + 2], 16).unwrap())
        .collect();
    let parcel = format!("{payer}/parcel.json");
    let sent = tachys(&[
        "wallet",
        "send",
        "--state",
        &payer,
        "--to",
        &to,
        "--nanoerg",
        "2000000000",
        "--parcel-out",
        &parcel,
        "--json",
    ])
    .json();
    let receipt = sent["receipt"][0].as_str().unwrap().to_string();
    let p: Value = serde_json::from_slice(&std::fs::read(&parcel).unwrap()).unwrap();
    assert_eq!(
        tachys(&[
            "wallet",
            "promise",
            "--state",
            &payee,
            "--tx",
            p["tx_bytes"].as_str().unwrap(),
            "--json"
        ])
        .code,
        0
    );
    // first the payer's coin is unspent: the payment is on its way
    let (before, root) = tree_node(vec![coin]);
    chain_over(&payee, &root);
    let r = tachys(&[
        "wallet", "receive", "--state", &payee, "--node", &before, "--ids", &receipt, "--json",
    ]);
    assert_eq!(
        state_of(&r, &receipt).as_deref(),
        Some("expected"),
        "before: {}",
        r.err
    );
    // then the coin is in no box (spent by another payment), and the promised box neither. The outbox, refused, keeps
    // the payment (it may be the payee's only copy, and the outbox does not know which coins the wallet saw, or at what
    // height); the payee's own sync, which does, calls it void
    let (node, root) = tree_node(vec![]);
    chain_over(&payee, &root);
    let r = tachys(&["outbox", "--state", &payee, "--rest", &node, "--json"]);
    assert_eq!((r.code, &r.json()["dropped"]), (0, &0.into()), "{}", r.err);
    let r = tachys(&[
        "wallet", "receive", "--state", &payee, "--node", &node, "--ids", &receipt, "--json",
    ]);
    assert_eq!(
        state_of(&r, &receipt).as_deref(),
        Some("void"),
        "after: {}",
        r.err
    );
    assert!(
        std::fs::read_dir(format!("{payee}/given-up"))
            .map(|d| d.count())
            .unwrap_or(0)
            == 1,
        "put aside, not deleted"
    );
    let w: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{payee}/wallet.json")).unwrap())
            .unwrap();
    assert!(
        w["expect"]
            .as_array()
            .is_none_or(|e| !e.contains(&receipt.clone().into())),
        "no longer expected: {w}"
    );
}

#[test]
fn a_box_whose_confirming_block_left_the_chain_is_undone_not_spent_and_is_the_same_payment_if_it_comes_back(
) {
    // Seen on the rig: a payment proven with a confirmation was undone by a longer chain that spent its coin elsewhere,
    // and the payee's wallet said its box was "spent (not by a payment of this wallet)".
    let undone_id = |st: &str| -> (String, Vec<u8>) {
        let w: Value =
            serde_json::from_str(&std::fs::read_to_string(format!("{st}/wallet.json")).unwrap())
                .unwrap();
        let (id, b) = w["boxes"].as_object().unwrap().iter().next().unwrap();
        let hex = b["bytes"].as_str().unwrap();
        (
            id.clone(),
            (0..hex.len() / 2)
                .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap())
                .collect(),
        )
    };
    let set_confirmed = |st: &str, at: u64, under: &str| {
        let mut w: Value =
            serde_json::from_str(&std::fs::read_to_string(format!("{st}/wallet.json")).unwrap())
                .unwrap();
        for b in w["boxes"].as_object_mut().unwrap().values_mut() {
            b["confirmed_at"] = at.into();
            b["confirmed_in"] = under.into();
        }
        std::fs::write(format!("{st}/wallet.json"), w.to_string()).unwrap();
    };
    let (gone, root) = tree_node(vec![]);
    // confirmed under a header the chain no longer has at that height: undone
    let st = funded("undone");
    let (id, bytes) = undone_id(&st);
    let h = chain_over(&st, &root);
    set_confirmed(&st, h.height as u64, &"ff".repeat(32));
    let r = tachys(&[
        "wallet", "receive", "--state", &st, "--node", &gone, "--ids", &id, "--json",
    ]);
    assert_eq!(state_of(&r, &id).as_deref(), Some("undone"), "{}", r.err);
    assert!(r.err.contains("UNDONE by a reorganisation"), "{}", r.err);
    // asked about again while it is still gone: still undone, not "absent" (an outside seat found it said so once only)
    let r = tachys(&[
        "wallet", "receive", "--state", &st, "--node", &gone, "--ids", &id, "--json",
    ]);
    assert_eq!(state_of(&r, &id).as_deref(), Some("undone"), "{}", r.err);
    // it comes back: proven, and not new
    let (back, root2) = tree_node(vec![bytes]);
    chain_over(&st, &root2);
    let r = tachys(&[
        "wallet", "receive", "--state", &st, "--node", &back, "--ids", &id, "--json",
    ]);
    let c = r.json()["checked"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id.as_str())
        .cloned()
        .unwrap();
    assert_eq!(
        (c["state"].as_str(), &c["added"]),
        (Some("proven"), &Value::Bool(false)),
        "the same payment back: {}",
        r.err
    );
    // control: confirmed under the header the chain still has there, so its block is still in the chain: spent
    let st = funded("spent-not-undone");
    let (id, _) = undone_id(&st);
    let h = chain_over(&st, &root);
    let hid: Vec<u8> = h.id.0.into();
    set_confirmed(
        &st,
        h.height as u64,
        &hid.iter().map(|b| format!("{b:02x}")).collect::<String>(),
    );
    let r = tachys(&[
        "wallet", "receive", "--state", &st, "--node", &gone, "--ids", &id, "--json",
    ]);
    assert_eq!(state_of(&r, &id).as_deref(), Some("spent"), "{}", r.err);
}

#[test]
fn a_receipt_known_only_by_its_box_id_is_told_late_or_double_spent_by_the_payment_a_node_holds_for_it(
) {
    // A payee given only a box id could not tell a payment on its way from one whose coin was spent elsewhere: both are
    // "no such box". With `--pools` the wallet takes from a node's pool the payment that makes the box, and from then
    // on proves its coins with the box.
    let payer = funded("pool-payer");
    let payee = dir("pool-payee");
    let to = tachys(&["wallet", "new", "--state", &payee, "--json"]).json()["address"]
        .as_str()
        .unwrap()
        .to_string();
    // the payer's coin, read before it pays (its wallet then holds it back)
    let w: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{payer}/wallet.json")).unwrap())
            .unwrap();
    let coin = w["boxes"].as_object().unwrap().values().next().unwrap()["bytes"]
        .as_str()
        .unwrap()
        .to_string();
    let coin: Vec<u8> = (0..coin.len() / 2)
        .map(|i| u8::from_str_radix(&coin[2 * i..2 * i + 2], 16).unwrap())
        .collect();
    let parcel = format!("{payer}/parcel.json");
    let receipt = tachys(&[
        "wallet",
        "send",
        "--state",
        &payer,
        "--to",
        &to,
        "--nanoerg",
        "2000000000",
        "--parcel-out",
        &parcel,
        "--json",
    ])
    .json()["receipt"][0]
        .as_str()
        .unwrap()
        .to_string();
    let p: Value = serde_json::from_slice(&std::fs::read(&parcel).unwrap()).unwrap();
    let pool = Some((
        p["tx_id"].as_str().unwrap().to_string(),
        p["tx_json"].as_str().unwrap().to_string(),
    ));
    // no pool to look in: nothing but "absent"
    let (bare, root) = tree_node(vec![coin.clone()]);
    chain_over(&payee, &root);
    let r = tachys(&[
        "wallet", "receive", "--state", &payee, "--node", &bare, "--ids", &receipt, "--pools",
        "--json",
    ]);
    assert_eq!(
        state_of(&r, &receipt).as_deref(),
        Some("absent"),
        "{}",
        r.err
    );
    // a node holds the payment's body with a spending proof changed: its id and the box's are the same (a payment's id
    // does not cover its signatures), and it is not held
    let mut forged: Value = serde_json::from_str(&pool.as_ref().unwrap().1).unwrap();
    let pb = forged["inputs"][0]["spendingProof"]["proofBytes"]
        .as_str()
        .unwrap()
        .to_string();
    forged["inputs"][0]["spendingProof"]["proofBytes"] = format!(
        "{}{}",
        if pb.starts_with('0') { "1" } else { "0" },
        &pb[1..]
    )
    .into();
    let (bad, root) = tree_node_with(
        vec![coin.clone()],
        Some((pool.as_ref().unwrap().0.clone(), forged.to_string())),
    );
    chain_over(&payee, &root);
    let r = tachys(&[
        "wallet", "receive", "--state", &payee, "--node", &bad, "--ids", &receipt, "--pools",
        "--json",
    ]);
    assert_eq!(
        state_of(&r, &receipt).as_deref(),
        Some("absent"),
        "{}",
        r.err
    );
    assert!(r.err.contains("not held: spending proof"), "{}", r.err);
    // a node holds the payment and the payer's coin is unspent: on its way
    let (late, root) = tree_node_with(vec![coin], pool.clone());
    chain_over(&payee, &root);
    let r = tachys(&[
        "wallet", "receive", "--state", &payee, "--node", &late, "--ids", &receipt, "--pools",
        "--json",
    ]);
    assert_eq!(
        state_of(&r, &receipt).as_deref(),
        Some("expected"),
        "{}",
        r.err
    );
    assert!(r.err.contains("taken and held"), "{}", r.err);
    // the payer's coin is then spent by another payment: the payment held is dead, and so is the receipt, told by the
    // payee's own sync (no outbox run)
    let (spent, root) = tree_node_with(vec![], pool);
    chain_over(&payee, &root);
    let r = tachys(&[
        "wallet", "receive", "--state", &payee, "--node", &spent, "--ids", &receipt, "--json",
    ]);
    assert_eq!(state_of(&r, &receipt).as_deref(), Some("void"), "{}", r.err);
    assert!(
        std::fs::read_dir(format!("{payee}/given-up"))
            .map(|d| d.count())
            .unwrap_or(0)
            == 1,
        "the dead payment is put aside"
    );
}

#[test]
fn a_mutated_transaction_box_or_signing_request_is_refused_without_a_panic() {
    // Bytes from nodes, peers and pages reach the transaction, box and reduced-transaction decoders of the signing
    // library (not ours to change). Mutated from real ones (bits, bytes, inserts, cuts, repeats, extremes), none of
    // 300,000 made one panic when measured (finding 57); this keeps a share of that in the suite.
    use base64::Engine;
    use ergo_lib::chain::transaction::reduced::ReducedTransaction;
    use ergo_lib::chain::transaction::Transaction;
    let payer = funded("fuzz-seeds");
    let parcel = format!("{payer}/parcel.json");
    let to = text(&address(1));
    assert_eq!(
        tachys(&[
            "wallet",
            "send",
            "--state",
            &payer,
            "--to",
            &to,
            "--nanoerg",
            "2000000000",
            "--parcel-out",
            &parcel,
            "--json"
        ])
        .code,
        0
    );
    let p: Value = serde_json::from_slice(&std::fs::read(&parcel).unwrap()).unwrap();
    let h = p["tx_bytes"].as_str().unwrap();
    let tx: Vec<u8> = (0..h.len() / 2)
        .map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap())
        .collect();
    let payer2 = funded("fuzz-seeds2");
    let req = tachys(&[
        "wallet",
        "send",
        "--state",
        &payer2,
        "--to",
        &to,
        "--nanoerg",
        "2000000000",
        "--request-out",
        "--json",
    ])
    .json()["request"]
        .as_str()
        .unwrap()
        .to_string();
    let red = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(req.strip_prefix("ergopay:").unwrap())
        .unwrap();
    let parsed = Transaction::sigma_parse_bytes(&tx).unwrap();
    let boxes: Vec<Vec<u8>> = parsed
        .outputs
        .iter()
        .map(|o| o.sigma_serialize_bytes().unwrap())
        .collect();
    let mut x: u64 = 0x5eed;
    let mut rnd = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mutate = |b: &[u8], r: &mut dyn FnMut() -> u64| -> Vec<u8> {
        let mut v = b.to_vec();
        for _ in 0..(1 + r() % 4) {
            if v.is_empty() {
                break;
            }
            let i = (r() as usize) % v.len();
            match r() % 6 {
                0 => v[i] ^= 1 << (r() % 8),
                1 => v[i] = r() as u8,
                2 => v.insert(i, r() as u8),
                3 => {
                    v.remove(i);
                }
                4 => {
                    let j = (r() as usize) % v.len();
                    let (a, b) = (i.min(j), i.max(j));
                    let seg: Vec<u8> = v[a..b.min(a + 40)].to_vec();
                    v.splice(i..i, seg);
                }
                _ => v[i] = [0, 0xff, 0x7f, 0x80][(r() % 4) as usize],
            }
        }
        v
    };
    let n = 10_000usize;
    let mut counts = [0usize; 3];
    for _ in 0..n {
        let v = mutate(&tx, &mut rnd);
        if std::panic::catch_unwind(|| Transaction::sigma_parse_bytes(&v)).is_err() {
            counts[0] += 1
        }
    }
    for _ in 0..n {
        let b = &boxes[(rnd() as usize) % boxes.len()];
        let v = mutate(b, &mut rnd);
        if std::panic::catch_unwind(|| ergotree_ir::chain::ergo_box::ErgoBox::sigma_parse_bytes(&v))
            .is_err()
        {
            counts[1] += 1
        }
    }
    for _ in 0..n {
        let v = mutate(&red, &mut rnd);
        if std::panic::catch_unwind(|| ReducedTransaction::sigma_parse_bytes(&v)).is_err() {
            counts[2] += 1
        }
    }
    assert_eq!(
        counts,
        [0, 0, 0],
        "panics: transactions, boxes, signing requests"
    );
}

#[test]
fn a_mutated_signing_request_never_crashes_the_wallet() {
    // the built binary, given a page's request with bytes changed: it shows or refuses, it does not crash
    use base64::Engine;
    let payer = funded("fuzz-req");
    let to = text(&address(1));
    let req = tachys(&[
        "wallet",
        "send",
        "--state",
        &payer,
        "--to",
        &to,
        "--nanoerg",
        "2000000000",
        "--request-out",
        "--json",
    ])
    .json()["request"]
        .as_str()
        .unwrap()
        .to_string();
    let red = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(req.strip_prefix("ergopay:").unwrap())
        .unwrap();
    let signer = funded("fuzz-req-signer");
    let mut x: u64 = 0x77;
    let mut rnd = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let (mut crash, mut parsed) = (vec![], 0);
    for _ in 0..150 {
        let mut v = red.clone();
        for _ in 0..(1 + rnd() % 3) {
            let i = (rnd() as usize) % v.len();
            match rnd() % 3 {
                0 => v[i] ^= 1 << (rnd() % 8),
                1 => v[i] = rnd() as u8,
                _ => v[i] = [0, 0xff, 0x7f, 0x80][(rnd() % 4) as usize],
            }
        }
        let code = format!(
            "ergopay:{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&v)
        );
        let r = tachys(&[
            "wallet", "request", "--state", &signer, "--code", &code, "--json",
        ]);
        if r.code != 0 && r.code != 1 && r.code != 2 {
            crash.push((r.code, r.err.lines().last().unwrap_or("").to_string()));
        }
        if !r.err.contains("does not hold a transaction") {
            parsed += 1;
        }
    }
    assert!(crash.is_empty(), "{crash:?}");
    assert!(
        parsed > 0,
        "some mutated requests still read as requests and were shown"
    );
}

#[test]
fn a_carried_payment_goes_to_a_node_as_its_bytes_say_and_padded_bytes_are_turned_away_at_the_door()
{
    // left open by finding 51: `outbox --rest` sent the stored `tx_json`, which nothing compared with the bytes, and
    // `serve --accept-parcels` took bytes that are not the transaction's own serialisation
    let payer = funded("rest-canon-payer");
    let parcel = format!("{payer}/parcel.json");
    assert_eq!(
        tachys(&[
            "wallet",
            "send",
            "--state",
            &payer,
            "--to",
            &text(&address(1)),
            "--nanoerg",
            "2000000000",
            "--parcel-out",
            &parcel,
            "--json"
        ])
        .code,
        0
    );
    let good: Value = serde_json::from_slice(&std::fs::read(&parcel).unwrap()).unwrap();
    let id = good["tx_id"].as_str().unwrap().to_string();
    // the stored JSON says something else: the node is sent the transaction the bytes hold
    let mut lying = good.clone();
    lying["tx_json"] = r#"{"id":"0000000000000000000000000000000000000000000000000000000000000000","inputs":[],"dataInputs":[],"outputs":[]}"#.into();
    let carrier = dir("rest-canon-carrier");
    std::fs::create_dir_all(format!("{carrier}/outbox")).unwrap();
    std::fs::write(format!("{carrier}/outbox/{id}.json"), lying.to_string()).unwrap();
    let (taker, seen) = node(200, &*Box::leak(format!("\"{id}\"").into_boxed_str()));
    let r = tachys(&["outbox", "--state", &carrier, "--rest", &taker, "--json"]);
    assert_eq!((r.code, &r.json()["sent"]), (0, &1.into()), "{}", r.err);
    let sent: Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
    assert_eq!(
        sent["id"].as_str(),
        Some(id.as_str()),
        "the transaction the bytes hold, not the stored JSON"
    );
    // the same payment with a byte after its end: not sent, set aside
    let mut padded = good.clone();
    padded["tx_bytes"] = format!("{}00", good["tx_bytes"].as_str().unwrap()).into();
    std::fs::write(format!("{carrier}/outbox/{id}.json"), padded.to_string()).unwrap();
    let (node2, seen2) = node(200, "\"00\"");
    let r = tachys(&["outbox", "--state", &carrier, "--rest", &node2, "--json"]);
    assert_eq!((r.code, &r.json()["dropped"]), (0, &1.into()), "{}", r.err);
    assert!(seen2.lock().unwrap().is_empty(), "nothing reached the node");
    assert!(std::path::Path::new(&format!("{carrier}/given-up/{id}.json")).exists());
    // at a serving wallet's door
    let server = dir("door-server");
    chain_over(&server, &[1u8; 33]);
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let addr = format!("127.0.0.1:{port}");
    let mut child = Command::new(env!("CARGO_BIN_EXE_tachys"))
        .args([
            "serve",
            "--state",
            &server,
            "--listen",
            &addr,
            "--secs",
            "20",
            "--accept-parcels",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if std::net::TcpStream::connect(&addr).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let pf = format!("{payer}/padded.json");
    std::fs::write(
        &pf,
        serde_json::json!({ "tx_id": id, "tx_bytes": padded["tx_bytes"] }).to_string(),
    )
    .unwrap();
    let bad = tachys(&["handoff", "--peer", &addr, "--parcel", &pf, "--json"]);
    let gf = format!("{payer}/good.json");
    std::fs::write(
        &gf,
        serde_json::json!({ "tx_id": id, "tx_bytes": good["tx_bytes"] }).to_string(),
    )
    .unwrap();
    let ok = tachys(&["handoff", "--peer", &addr, "--parcel", &gf, "--json"]);
    let _ = child.kill();
    let _ = child.wait();
    assert_ne!(
        bad.code, 0,
        "padded bytes are not acknowledged: {}",
        bad.err
    );
    assert_eq!(ok.code, 0, "the payment as written is: {}", ok.err);
    assert_eq!(
        std::fs::read_dir(format!("{server}/outbox"))
            .unwrap()
            .count(),
        1,
        "only the payment as written was taken"
    );
    // what was written is what was checked, nothing of the sender's (a review seat: a parcel's `coins_seen` and `tx_id`
    // went into the file as sent)
    let kept: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{server}/outbox/{id}.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        kept.as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["tx_bytes".to_string(), "tx_id".to_string()]
    );
}

#[test]
fn a_restored_wallet_looks_over_all_twenty_addresses_again_when_it_reconciles() {
    // A restored wallet whose twenty addresses were all empty kept only the first; a payment made later to another of
    // them (by another wallet of the same phrase) was never seen (roadmap). Reconciling now looks over all twenty again.
    let st = dir("rescan");
    let me3 = address(3);
    let b = ErgoBox::from_box_candidate(
        &ErgoBoxCandidateBuilder::new(
            BoxValue::try_from(3 * ERG).unwrap(),
            me3.script().unwrap(),
            1_900_000,
        )
        .build()
        .unwrap(),
        TxId::zero(),
        0,
    )
    .unwrap();
    let id = String::from(b.box_id());
    std::fs::write(format!("{st}/wallet.json"), serde_json::json!({ "name": "r", "address": text(&address(0)), "address_list": [text(&address(0))], "addresses": 1, "birthday": 0, "mnemonic": PHRASE, "restored": true, "boxes": {} }).to_string()).unwrap();
    let (node, root) = tree_node_full(
        vec![b.sigma_serialize_bytes().unwrap()],
        None,
        Some((text(&me3), id.clone())),
    );
    chain_over(&st, &root);
    // a plain sync looks at the address it keeps, and finds nothing
    let r = tachys(&[
        "wallet", "status", "--state", &st, "--node", &node, "--json",
    ]);
    assert_eq!(r.json()["nanoerg"], 0, "{}", r.err);
    let r = tachys(&[
        "wallet",
        "status",
        "--state",
        &st,
        "--node",
        &node,
        "--reconcile",
        "--json",
    ]);
    assert_eq!(
        (r.code, &r.json()["nanoerg"]),
        (0, &(3 * ERG).into()),
        "{}",
        r.err
    );
    let w: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{st}/wallet.json")).unwrap())
            .unwrap();
    assert_eq!(
        w["addresses"], 4,
        "the four up to the one that holds a box are kept: {w}"
    );
}

#[test]
fn a_held_payment_whose_coin_was_never_seen_is_not_called_void() {
    // A payer that chains payments spends a coin made by a payment not yet in a block: until that one is, the coin is in
    // no box, as a coin spent elsewhere is. One review seat found such a payment called void ("can never confirm"). A
    // coin is now taken for gone only if this wallet saw it unspent before.
    let payer = funded("chain-payer");
    let payee = dir("chain-payee");
    let to = tachys(&["wallet", "new", "--state", &payee, "--json"]).json()["address"]
        .as_str()
        .unwrap()
        .to_string();
    let w: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{payer}/wallet.json")).unwrap())
            .unwrap();
    let coin = w["boxes"].as_object().unwrap().values().next().unwrap()["bytes"]
        .as_str()
        .unwrap()
        .to_string();
    let coin: Vec<u8> = (0..coin.len() / 2)
        .map(|i| u8::from_str_radix(&coin[2 * i..2 * i + 2], 16).unwrap())
        .collect();
    let parcel = format!("{payer}/parcel.json");
    let receipt = tachys(&[
        "wallet",
        "send",
        "--state",
        &payer,
        "--to",
        &to,
        "--nanoerg",
        "2000000000",
        "--parcel-out",
        &parcel,
        "--json",
    ])
    .json()["receipt"][0]
        .as_str()
        .unwrap()
        .to_string();
    let p: Value = serde_json::from_slice(&std::fs::read(&parcel).unwrap()).unwrap();
    assert_eq!(
        tachys(&[
            "wallet",
            "promise",
            "--state",
            &payee,
            "--tx",
            p["tx_bytes"].as_str().unwrap(),
            "--json"
        ])
        .code,
        0
    );
    // the coin in no box, never seen: still expected
    let (none, root) = tree_node(vec![]);
    chain_over(&payee, &root);
    let r = tachys(&[
        "wallet", "receive", "--state", &payee, "--node", &none, "--ids", &receipt, "--json",
    ]);
    assert_eq!(
        state_of(&r, &receipt).as_deref(),
        Some("expected"),
        "{}",
        r.err
    );
    let r = tachys(&["outbox", "--state", &payee, "--rest", &none, "--json"]);
    assert_eq!(r.json()["dropped"], 0, "the outbox keeps it too: {}", r.err);
    // the coin appears (its payment is in a block), then is spent by another payment: now it is void
    let coin_back = coin.clone();
    let (some, root) = tree_node(vec![coin]);
    chain_over(&payee, &root);
    let r = tachys(&[
        "wallet", "receive", "--state", &payee, "--node", &some, "--ids", &receipt, "--json",
    ]);
    assert_eq!(
        state_of(&r, &receipt).as_deref(),
        Some("expected"),
        "{}",
        r.err
    );
    // a node a block behind, where the coin is not made yet: that says nothing (a review seat: it voided the payment)
    let (gone, root) = tree_node(vec![]);
    chain_over_at(&payee, &root, 1);
    let r = tachys(&[
        "wallet", "receive", "--state", &payee, "--node", &gone, "--ids", &receipt, "--json",
    ]);
    assert_eq!(
        state_of(&r, &receipt).as_deref(),
        Some("expected"),
        "a proof from below the height it was seen at: {}",
        r.err
    );
    chain_over(&payee, &root);
    let r = tachys(&[
        "wallet", "receive", "--state", &payee, "--node", &gone, "--ids", &receipt, "--json",
    ]);
    assert_eq!(state_of(&r, &receipt).as_deref(), Some("void"), "{}", r.err);
    // the payment that spent the coin is itself undone and the coin is back: the payment is held again (an outside seat)
    let (back, root) = tree_node(vec![coin_back]);
    chain_over(&payee, &root);
    let r = tachys(&[
        "wallet", "receive", "--state", &payee, "--node", &back, "--ids", &receipt, "--json",
    ]);
    assert_eq!(
        state_of(&r, &receipt).as_deref(),
        Some("expected"),
        "{}",
        r.err
    );
    assert!(
        std::fs::read_dir(format!("{payee}/outbox"))
            .map(|d| d.count())
            .unwrap_or(0)
            == 1,
        "back in the outbox, to be sent"
    );
}

/// A JSON value with every leaf replaced by the name of its type: what a reader of the answer relies on.
fn shape(v: &Value) -> Value {
    match v {
        Value::Null => "null".into(),
        Value::Bool(_) => "bool".into(),
        Value::Number(_) => "number".into(),
        Value::String(_) => "text".into(),
        Value::Array(a) => Value::Array(a.first().map(shape).into_iter().collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), shape(v))).collect()),
    }
}

#[test]
fn the_answers_the_app_reads_keep_the_fields_and_types_of_the_ones_it_is_tested_on() {
    // `vectors/app/` holds answers of the core that the app's own tests read (android/shared/src/desktopTest). Each is made
    // again here, and must have the same fields with the same types: a field renamed, retyped or dropped fails this
    // test before it reaches a phone. (Values differ from run to run, and are not compared.)
    let with = |args: &[&str], pass: Option<&str>, input: &str| -> Ran {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tachys"));
        c.args(args)
            .env_remove("TACHYS_PASS")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(p) = pass {
            c.env("TACHYS_PASS", p);
        }
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let o = child.wait_with_output().unwrap();
        Ran {
            code: o.status.code().unwrap_or(-1),
            out: String::from_utf8_lossy(&o.stdout).to_string(),
            err: String::from_utf8_lossy(&o.stderr).to_string(),
        }
    };
    let payee = text(&address(1));
    let mut made: Vec<(&str, i32, Value)> = vec![];
    let r = with(
        &["wallet", "derive", "--mnemonic-file", "-", "--json"],
        None,
        &format!("{PHRASE}\n"),
    );
    made.push(("derive", r.code, r.json()));
    let st = funded("app-queued");
    let r = tachys(&[
        "wallet",
        "send",
        "--state",
        &st,
        "--to",
        &payee,
        "--nanoerg",
        "1000000000",
        "--parcel-out",
        &format!("{st}/parcel.json"),
        "--json",
    ]);
    made.push(("send-queued", r.code, r.json()));
    for (name, status, body) in [
        (
            "send-sent",
            200,
            "\"0000000000000000000000000000000000000000000000000000000000000000\"",
        ),
        (
            "send-refused",
            400,
            r#"{"error":400,"reason":"bad.request","detail":"double spend"}"#,
        ),
        ("send-unsure", 500, "{}"),
    ] {
        let st = funded(&format!("app-{name}"));
        let (n, _) = node(status, body);
        let r = tachys(&[
            "wallet",
            "send",
            "--state",
            &st,
            "--to",
            &payee,
            "--nanoerg",
            "1000000000",
            "--node",
            &n,
            "--broadcast",
            "--json",
        ]);
        made.push((name, r.code, r.json()));
    }
    let st = dir("app-sealed");
    assert_eq!(
        with(
            &["wallet", "new", "--state", &st, "--json"],
            Some("one password"),
            ""
        )
        .code,
        0
    );
    let r = with(
        &["wallet", "mnemonic", "--state", &st, "--json"],
        Some("another"),
        "",
    );
    made.push(("bad-password", r.code, r.json()));
    for (name, code, answer) in made {
        let file = format!(
            "{}/../../vectors/app/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let kept: Value = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| {
                panic!(
                    "{file} is missing or not JSON; the core answers:\n{}",
                    serde_json::json!({ "code": code, "answer": answer })
                )
            });
        assert_eq!(kept["code"], code, "{name}: the exit code");
        assert_eq!(shape(&kept["answer"]), shape(&answer), "{name}: the core answers\n{answer}\nand vectors/app/{name}.json, which the app is tested on, holds\n{}", kept["answer"]);
    }
}

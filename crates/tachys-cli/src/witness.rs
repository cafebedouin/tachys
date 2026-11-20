//! Test for review finding W1, written to fail on the code before its fix (f7e5f0a) and pass after it.
//! It asserts the correct behaviour. W2 to W6 need a node and the sync
//! path, and are in `tests/witness.rs`.
use ergo_lib::chain::ergo_box::box_builder::ErgoBoxCandidateBuilder;
use ergo_lib::chain::transaction::Transaction;
use ergo_lib::wallet::derivation_path::DerivationPath;
use ergo_lib::wallet::ext_secret_key::ExtSecretKey;
use ergo_lib::wallet::mnemonic::Mnemonic;
use ergotree_ir::chain::address::{Address, AddressEncoder, NetworkPrefix};
use ergotree_ir::chain::ergo_box::box_value::BoxValue;
use ergotree_ir::chain::ergo_box::ErgoBox;
use ergotree_ir::chain::tx_id::TxId;
use ergotree_ir::serialization::SigmaSerializable;
use sigma_ser::ScorexSerializable;
use std::str::FromStr;

const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const ERG: u64 = 1_000_000_000;

fn address(i: u32) -> Address {
    let m = ExtSecretKey::derive_master(Mnemonic::to_seed(PHRASE, "")).unwrap();
    Address::P2Pk(m.derive(DerivationPath::from_str(&format!("m/44'/429'/0'/0/{i}")).unwrap()).unwrap().public_image())
}
fn text(a: &Address) -> String { AddressEncoder::new(NetworkPrefix::Mainnet).address_to_str(a) }
fn ergo_box(value: u64, creation: u32, index: u16) -> ErgoBox {
    ErgoBox::from_box_candidate(&ErgoBoxCandidateBuilder::new(BoxValue::try_from(value).unwrap(), address(0).script().unwrap(), creation).build().unwrap(), TxId::zero(), index).unwrap()
}

/// A state directory with ten headers to sign under and a wallet of the test phrase whose one confirmed `boxes`
/// entry is filed under `key` and holds `held`'s bytes.
fn state(name: &str, key: &ErgoBox, held: &ErgoBox) -> String {
    let dir = std::env::temp_dir().join(format!("tachys-witness-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.to_string_lossy().to_string();
    let header = crate::network::checkpoint_header();
    let bytes = header.scorex_serialize_bytes().unwrap();
    let mut chain = vec![];
    for _ in 0..10 { chain.extend_from_slice(&(bytes.len() as u16).to_be_bytes()); chain.extend_from_slice(&bytes); }
    std::fs::write(format!("{dir}/agreed-chain.bin"), chain).unwrap();
    let me = text(&address(0));
    let entry = serde_json::json!({ "value": *key.value.as_u64(), "confirmed_at": 1, "bytes": crate::util::hex(&held.sigma_serialize_bytes().unwrap()) });
    let w = serde_json::json!({ "name": "w1", "address": me, "address_list": [me], "birthday": 0, "mnemonic": PHRASE, "boxes": { String::from(key.box_id()): entry } });
    std::fs::write(format!("{dir}/wallet.json"), w.to_string()).unwrap();
    dir
}

/// `tachys wallet send` (signed, not broadcast) through the library's one way in; the box ids the signed
/// transaction spends, or None if nothing was signed.
fn send(st: &str) -> (crate::Answer, Option<Vec<String>>) {
    let args: Vec<String> = ["tachys", "wallet", "send", "--to", &text(&address(1)), "--nanoerg", "1000000000", "--state", st].map(String::from).to_vec();
    let a = crate::call(&args, &[]);
    let tx: Option<Transaction> = std::fs::read_to_string(format!("{st}/last-tx.json")).ok().and_then(|t| serde_json::from_str(&t).ok());
    (a, tx.map(|t| t.inputs.iter().map(|i| String::from(i.box_id)).collect()))
}

/// W1 (`wallet.rs:570`): `send` takes a confirmed box's stored bytes without checking that they hash to the id they
/// are filed under (`cmd_request`, `wallet.rs:431`, does check). Correct: an entry whose bytes are another box's is
/// not spent (send refuses or skips it).
#[test]
fn w1_send_does_not_spend_stored_bytes_that_are_not_the_box_they_are_filed_under() {
    let filed = ergo_box(10 * ERG, crate::network::checkpoint_header().height - 100, 0);
    let other = ergo_box(50 * ERG, 1, 1);
    assert_ne!(filed.box_id(), other.box_id());
    // positive arm: the entry's bytes are its own box; send signs a payment spending it
    let st = state("w1-match", &filed, &filed);
    let (a, spent) = send(&st);
    assert_eq!(a.code, 0, "positive arm: a wallet whose entry is whole pays ({}{})", a.out, a.err);
    assert_eq!(spent, Some(vec![String::from(filed.box_id())]), "positive arm: the box filed is the box spent");
    let _ = std::fs::remove_dir_all(&st);
    // the lead: the entry filed under `filed`'s id holds `other`'s bytes
    let st = state("w1-mismatch", &filed, &other);
    let (a, spent) = send(&st);
    let _ = std::fs::remove_dir_all(&st);
    let spent = spent.unwrap_or_default();
    assert!(!spent.contains(&String::from(other.box_id())) && !spent.contains(&String::from(filed.box_id())),
        "W1: an entry filed under {} holding the bytes of {} was spent anyway: send exit {}, signed inputs {spent:?}",
        String::from(filed.box_id()), String::from(other.box_id()), a.code);
    // refused, not skipped, and the refusal names the box
    assert!(a.code != 0 && a.err.contains(&String::from(filed.box_id())), "W1: refused, naming the box: exit {}, {}", a.code, a.err);
}

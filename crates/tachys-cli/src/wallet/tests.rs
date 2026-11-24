//! Tests of the wallet's commands and their parts: no network, wallets under the system's temporary directory.

use super::*;
use ergo_lib::chain::ergo_box::box_builder::ErgoBoxCandidateBuilder;
use ergo_lib::chain::transaction::Transaction;
use ergotree_ir::chain::ergo_box::box_value::BoxValue;
use ergotree_ir::chain::tx_id::TxId;
use sigma_ser::ScorexSerializable;

// the BIP-39 test vector for sixteen zero bytes of entropy
const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const ERG: u64 = 1_000_000_000;
const FEE: u64 = 1_100_000;

fn address(i: u64) -> Address {
    Address::P2Pk(key_at(PHRASE, i).unwrap().public_image())
}
fn text(a: &Address) -> String {
    AddressEncoder::new(NetworkPrefix::Mainnet).address_to_str(a)
}

/// A state directory with a wallet of the test phrase (its first address only), one box of ten ERG that the
/// wallet tracks, and ten headers to sign under. No node is involved: the box exists nowhere else.
fn state(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("tachys-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.to_string_lossy().to_string();
    let header = crate::network::checkpoint_header();
    let bytes = header.scorex_serialize_bytes().unwrap();
    let mut chain = vec![];
    for _ in 0..10 {
        chain.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        chain.extend_from_slice(&bytes);
    }
    std::fs::write(format!("{dir}/agreed-chain.bin"), chain).unwrap();
    let me = address(0);
    let candidate = ErgoBoxCandidateBuilder::new(
        BoxValue::try_from(10 * ERG).unwrap(),
        me.script().unwrap(),
        header.height - 100,
    )
    .build()
    .unwrap();
    let b = ErgoBox::from_box_candidate(&candidate, TxId::zero(), 0).unwrap();
    let mut boxes = serde_json::Map::new();
    boxes.insert(String::from(b.box_id()), serde_json::json!({ "value": 10 * ERG, "confirmed_at": 1, "bytes": hex(&b.sigma_serialize_bytes().unwrap()) }));
    let w = serde_json::json!({ "name": "test", "address": text(&me), "address_list": [text(&me)], "birthday": 0, "mnemonic": PHRASE, "boxes": boxes });
    std::fs::write(format!("{dir}/wallet.json"), w.to_string()).unwrap();
    dir
}

/// `tachys wallet REST --state STATE`, as the command line would run it, short of printing a refusal and stopping.
fn run(state: &str, rest: &[&str]) -> Result<(), String> {
    let args: Vec<String> = ["tachys", "wallet"]
        .iter()
        .chain(rest)
        .chain(&["--state", state])
        .map(|s| s.to_string())
        .collect();
    let c = Ctx::new(&args).map_err(|f| f.text().to_string())?;
    let o = open(&c).map_err(|f| f.text().to_string())?;
    (if c.sub == "request" {
        cmd_request(&c, o)
    } else {
        cmd_send(&c, o)
    })
    .map_err(|f| f.text().to_string())
}
fn signed(state: &str) -> Option<Transaction> {
    serde_json::from_str(&std::fs::read_to_string(format!("{state}/last-tx.json")).ok()?).ok()
}

fn token(n: u8, amount: u64) -> Token {
    Token {
        token_id: TokenId::from(ergo_chain_types::Digest32::from([n; 32])),
        amount: TokenAmount::try_from(amount).unwrap(),
    }
}
fn candidate(value: u64, tree: &ErgoTree, tokens: &[Token]) -> ErgoBoxCandidate {
    let mut b = ErgoBoxCandidateBuilder::new(BoxValue::try_from(value).unwrap(), tree.clone(), 1);
    for t in tokens {
        b.add_token(*t);
    }
    b.build().unwrap()
}
fn boxed(value: u64, tree: &ErgoTree, tokens: &[Token], index: u16) -> ErgoBox {
    ErgoBox::from_box_candidate(&candidate(value, tree, tokens), TxId::zero(), index).unwrap()
}
fn fee_script() -> ErgoTree {
    ErgoTree::sigma_parse_bytes(&unhex(&fee_tree_hex()).unwrap()).unwrap()
}
fn id_of(t: Token) -> String {
    String::from(t.token_id)
}

#[test]
fn what_leaves_the_wallet_is_what_its_boxes_put_in_less_what_comes_back() {
    let enc = AddressEncoder::new(NetworkPrefix::Mainnet);
    let (mine, payee, other) = (
        [address(0).script().unwrap(), address(1).script().unwrap()],
        address(5),
        address(6),
    );
    // two boxes of ours at two of our addresses go in, with 100 of one token and 7 of another
    let spent = [
        boxed(10 * ERG, &mine[0], &[token(1, 100)], 0),
        boxed(3 * ERG, &mine[1], &[token(2, 7)], 1),
    ];
    // 2 ERG and 40 of the first token to a payee, the fee, and the rest back to our first address, less 2 of the second token (burned)
    let outs = [
        candidate(2 * ERG, &payee.script().unwrap(), &[token(1, 40)]),
        candidate(FEE, &fee_script(), &[]),
        candidate(11 * ERG - FEE, &mine[0], &[token(1, 60), token(2, 5)]),
    ];
    let (nano, tokens, payees) = leaves_wallet(&spent, &outs, &mine, None, &enc);
    assert_eq!(nano, 2 * ERG + FEE, "what the payee gets and the fee");
    assert_eq!(
        tokens,
        [(id_of(token(1, 1)), 40), (id_of(token(2, 1)), 2)]
            .into_iter()
            .collect(),
        "tokens paid and tokens burned both leave"
    );
    assert_eq!(
        payees,
        vec![text(&payee)],
        "the fee is not a payee, nor is our own address"
    );
    // nothing but change: only the fee leaves, no token and no payee
    let (nano, tokens, payees) = leaves_wallet(
        &spent,
        &[
            candidate(FEE, &fee_script(), &[]),
            candidate(13 * ERG - FEE, &mine[1], &[token(1, 100), token(2, 7)]),
        ],
        &mine,
        None,
        &enc,
    );
    assert_eq!((nano, tokens.len(), payees.len()), (FEE, 0, 0));
    // a box of someone else's among those spent puts in nothing of ours, and what goes back to them is a payee's
    let theirs = boxed(4 * ERG, &other.script().unwrap(), &[token(3, 9)], 2);
    let with_theirs = [spent[0].clone(), theirs];
    let outs = [
        candidate(4 * ERG, &other.script().unwrap(), &[token(3, 9)]),
        candidate(FEE, &fee_script(), &[]),
        candidate(10 * ERG - FEE, &mine[0], &[token(1, 100)]),
    ];
    let (nano, tokens, payees) = leaves_wallet(&with_theirs, &outs, &mine, None, &enc);
    assert_eq!((nano, tokens.len(), payees), (FEE, 0, vec![text(&other)]));
    // each payee is named once, however many boxes it is paid in
    let outs = [
        candidate(ERG, &payee.script().unwrap(), &[]),
        candidate(ERG, &payee.script().unwrap(), &[]),
        candidate(ERG, &other.script().unwrap(), &[]),
        candidate(10 * ERG, &mine[0], &[token(1, 100), token(2, 7)]),
    ];
    let (nano, _, payees) = leaves_wallet(&spent, &outs, &mine, None, &enc);
    assert_eq!((nano, payees), (3 * ERG, vec![text(&payee), text(&other)]));
}

#[test]
fn only_the_offer_box_a_token_fee_puts_back_is_not_a_payee() {
    let enc = AddressEncoder::new(NetworkPrefix::Mainnet);
    let mine = [address(0).script().unwrap()];
    // a stand-in for the offer's contract: all that is read of it is that the box put back is under the same script
    let (offer_script, payee) = (address(7).script().unwrap(), address(5));
    let offer = boxed(5 * ERG, &offer_script, &[token(4, 10)], 3);
    let spent = [boxed(ERG / 2, &mine[0], &[token(4, 50)], 0), offer.clone()];
    let told = |at: i32| {
        let mut e = ContextExtension::empty();
        e.values.insert(0, Constant::from(at));
        (offer.box_id(), e)
    };
    // the payee, the offer put back with less ERG and 3 more of the token, the fee, our change
    let outs = |back: u64, also: Option<u64>| {
        let mut o = vec![
            candidate(ERG, &payee.script().unwrap(), &[]),
            candidate(back, &offer_script, &[token(4, 13)]),
            candidate(FEE, &fee_script(), &[]),
            candidate(1_000_000, &mine[0], &[token(4, 47)]),
        ];
        if let Some(v) = also {
            o.push(candidate(v, &offer_script, &[]));
        }
        o
    };
    let (nano, tokens, payees) =
        leaves_wallet(&spent, &outs(4 * ERG, None), &mine, Some(&told(1)), &enc);
    assert_eq!(
        nano,
        ERG / 2 - 1_000_000,
        "all the ERG we put in but the change"
    );
    assert_eq!(
        tokens,
        [(id_of(token(4, 1)), 3)].into_iter().collect(),
        "the token units the fee cost still leave"
    );
    assert_eq!(
        payees,
        vec![text(&payee)],
        "the offer put back is the fee's other half, not a payee"
    );
    let under_offer = text(&Address::recreate_from_ergo_tree(&offer_script).unwrap());
    // a second box under the offer's script is a payee
    assert_eq!(
        leaves_wallet(
            &spent,
            &outs(4 * ERG, Some(ERG)),
            &mine,
            Some(&told(1)),
            &enc
        )
        .2,
        vec![text(&payee), under_offer.clone()]
    );
    // so is the box put back if it holds more ERG than the offer had: that ERG would be ours
    assert_eq!(
        leaves_wallet(
            &spent,
            &outs(5 * ERG + 1, None),
            &mine,
            Some(&told(1)),
            &enc
        )
        .2,
        vec![text(&payee), under_offer.clone()]
    );
    // and a box under that script the contract is not told is its own, or with no offer spent at all
    assert_eq!(
        leaves_wallet(&spent, &outs(4 * ERG, None), &mine, Some(&told(0)), &enc).2,
        vec![text(&payee), under_offer.clone()]
    );
    assert_eq!(
        leaves_wallet(
            &spent[..1],
            &outs(4 * ERG, None),
            &mine,
            Some(&told(1)),
            &enc
        )
        .2,
        vec![text(&payee), under_offer.clone()]
    );
    assert_eq!(
        leaves_wallet(&spent, &outs(4 * ERG, None), &mine, None, &enc).2,
        vec![text(&payee), under_offer]
    );
}

#[test]
fn a_payment_pays_the_payee_and_the_fee_and_returns_the_rest() {
    let st = state("send");
    let (me, payee) = (address(0).script().unwrap(), address(1));
    run(
        &st,
        &["send", "--to", &text(&payee), "--nanoerg", "2000000000"],
    )
    .unwrap();
    let tx = signed(&st).expect("a signed transaction is written");
    assert_eq!(tx.inputs.len(), 1);
    let to = |tree: &ErgoTree| -> Vec<u64> {
        tx.outputs
            .iter()
            .filter(|o| &o.ergo_tree == tree)
            .map(|o| *o.value.as_u64())
            .collect()
    };
    assert_eq!(
        to(&payee.script().unwrap()),
        vec![2 * ERG],
        "the payee gets what was asked"
    );
    assert_eq!(to(&me), vec![8 * ERG - FEE], "the rest comes back");
    let fee: Vec<u64> = tx
        .outputs
        .iter()
        .filter(|o| {
            o.ergo_tree
                .sigma_serialize_bytes()
                .map(|b| hex(&b))
                .unwrap_or_default()
                == fee_tree_hex()
        })
        .map(|o| *o.value.as_u64())
        .collect();
    assert_eq!(fee, vec![FEE]);
    assert_eq!(tx.outputs.len(), 3, "and nothing else is paid");
    // signed, not sent: the wallet still counts its box
    let w =
        crate::walletfile::parse(&std::fs::read_to_string(format!("{st}/wallet.json")).unwrap())
            .unwrap();
    assert_eq!(tracked_count(&w), 1);
    let _ = std::fs::remove_dir_all(&st);
}

#[test]
fn a_refusal_is_handed_back_and_nothing_is_signed() {
    let st = state("refusals");
    let payee = text(&address(1));
    let refused = |rest: &[&str]| run(&st, rest).expect_err("refused");
    assert!(
        refused(&["send", "--to", "nonsense", "--nanoerg", "1000000000"]).contains("--to is not a")
    );
    assert!(
        refused(&["send", "--to", &payee, "--nanoerg", "99000000000"]).contains("box selection")
    );
    assert!(refused(&[
        "send",
        "--to",
        &payee,
        "--nanoerg",
        "1000000000",
        "--pay",
        "nonsense:5"
    ])
    .contains("--pay wants"));
    assert!(refused(&[
        "send",
        "--to",
        &payee,
        "--nanoerg",
        "1000000000",
        "--also-token",
        "nonsense:0"
    ])
    .contains("--also-token wants"));
    assert!(refused(&[
        "send",
        "--to",
        &payee,
        "--nanoerg",
        "1000000000",
        "--babel",
        "nonsense"
    ])
    .contains("--babel wants"));
    assert!(refused(&["consolidate"]).contains("fewer than two boxes"));
    assert!(refused(&["burn"]).contains("burn needs --token"));
    assert!(refused(&["request", "--code", "nonsense"]).contains("not an ergopay"));
    assert!(refused(&["request", "--code", "ergopay:AAAA"]).contains("does not hold a transaction"));
    assert!(signed(&st).is_none());
    let _ = std::fs::remove_dir_all(&st);
}

/// `--pay` and `--also-token` were read with `filter_map` over the word after each, so one given last, with no
/// value, was dropped: the payment was signed without that payee or token, and the command said yes. Each is
/// refused now, as any option without its value is, and nothing is signed.
#[test]
fn a_pay_or_also_token_given_last_with_no_value_is_refused() {
    let st = state("no-value");
    let payee = text(&address(1));
    // `run` puts `--state` last; here the option itself must be the last word
    let last_word = |last: &str| -> Result<(), String> {
        let args: Vec<String> = [
            "tachys",
            "wallet",
            "send",
            "--state",
            &st,
            "--to",
            &payee,
            "--nanoerg",
            "1000000000",
            last,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let c = Ctx::new(&args).map_err(|f| f.text().to_string())?;
        let o = open(&c).map_err(|f| f.text().to_string())?;
        cmd_send(&c, o).map_err(|f| f.text().to_string())
    };
    for last in ["--pay", "--also-token"] {
        let e = last_word(last).expect_err("refused");
        assert!(e.contains(&format!("{last} needs a value")), "{last}: {e}");
        assert!(signed(&st).is_none(), "{last}: nothing is signed");
    }
    let _ = std::fs::remove_dir_all(&st);
}

#[test]
fn a_wallet_file_that_cannot_be_read_is_not_no_wallet() {
    let st = state("damaged");
    let args = ["tachys", "wallet", "new", "--state", st.as_str()].map(String::from);
    let c = |_: &str| Ctx::new(&args).unwrap();
    assert!(matches!(c(&st).read(), Ok(Some(_))));
    // cut short, as a crash or a full disk leaves it: the phrase may still be in there
    let whole = std::fs::read_to_string(format!("{st}/wallet.json")).unwrap();
    for damaged in [&whole[..whole.len() / 2], "", "[]", "null"] {
        std::fs::write(format!("{st}/wallet.json"), damaged).unwrap();
        assert!(
            c(&st).read().unwrap_err().contains("was not touched"),
            "{damaged:?}"
        );
    }
    std::fs::remove_file(format!("{st}/wallet.json")).unwrap();
    assert_eq!(c(&st).read(), Ok(None), "only a missing file is no wallet");
    let _ = std::fs::remove_dir_all(&st);
}

#[test]
fn the_boxes_a_payment_creates_are_read_from_what_was_signed() {
    let st = state("outputs");
    let (me, payee) = (address(0).script().unwrap(), address(1));
    run(
        &st,
        &["send", "--to", &text(&payee), "--nanoerg", "2000000000"],
    )
    .unwrap();
    let tx = signed(&st).unwrap();
    let mut w = WalletFile::default();
    note_pending(
        &mut w,
        &tx,
        &std::fs::read_to_string(format!("{st}/last-tx.json")).unwrap(),
    );
    let outs = outputs_of(&w.pending[0], std::slice::from_ref(&me));
    assert_eq!(outs.len(), 3);
    for (o, (id, ours)) in tx.outputs.iter().zip(&outs) {
        assert_eq!(*id, String::from(o.box_id()));
        assert_eq!(*ours, o.ergo_tree == me, "only the change is ours");
    }
    assert_eq!(outs.iter().filter(|x| x.1).count(), 1);
    let _ = std::fs::remove_dir_all(&st);
}

#[test]
fn a_payment_after_one_given_up_spends_one_of_its_coins() {
    let st = state("released");
    let (me, payee) = (address(0), text(&address(1)));
    // a second, larger box beside the first: box selection alone would take it and leave the first untouched
    let path = format!("{st}/wallet.json");
    let mut w = crate::walletfile::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let small = w.boxes.keys().next().unwrap().clone();
    // box selection takes the box with the greatest id first, and the first box's id moves with the release
    // checkpoint (it is created a hundred blocks below it): the large box is one whose id is the greater
    let big = (1u16..)
        .map(|i| {
            ErgoBox::from_box_candidate(
                &ErgoBoxCandidateBuilder::new(
                    BoxValue::try_from(50 * ERG).unwrap(),
                    me.script().unwrap(),
                    1,
                )
                .build()
                .unwrap(),
                TxId::zero(),
                i,
            )
            .unwrap()
        })
        .find(|b| String::from(b.box_id()) > small)
        .unwrap();
    w.boxes.insert(
        String::from(big.box_id()),
        TrackedBox {
            value: 50 * ERG,
            confirmed_at: Some(1),
            bytes: hex(&big.sigma_serialize_bytes().unwrap()),
            ..Default::default()
        },
    );
    std::fs::write(&path, crate::walletfile::text(&w).unwrap()).unwrap();
    let spends = |st: &str| -> Vec<String> {
        signed(st)
            .unwrap()
            .inputs
            .iter()
            .map(|i| String::from(i.box_id))
            .collect()
    };
    // with nothing given up, the large box alone pays
    run(&st, &["send", "--to", &payee, "--nanoerg", "1000000000"]).unwrap();
    assert_eq!(spends(&st), vec![String::from(big.box_id())]);
    // an earlier payment spent the small box, was never included, and was given up: its signed transaction is still
    // out there and still valid. The next payment must spend that box too, so that only one of the two can confirm
    w.done.push(Done {
        tx: "aa".repeat(32),
        inputs: vec![small.clone()],
        result: "not included".into(),
        ..Default::default()
    });
    std::fs::write(&path, crate::walletfile::text(&w).unwrap()).unwrap();
    run(&st, &["send", "--to", &payee, "--nanoerg", "1000000000"]).unwrap();
    let ins = spends(&st);
    assert!(
        ins.contains(&small),
        "the coin of the payment given up is spent: {ins:?}"
    );
    // and nothing is lost by it: what went in comes back, less the payment and the fee
    let tx = signed(&st).unwrap();
    let back: u64 = tx
        .outputs
        .iter()
        .filter(|o| o.ergo_tree == me.script().unwrap())
        .map(|o| *o.value.as_u64())
        .sum();
    assert_eq!(back, 60 * ERG - ERG - FEE);
    let _ = std::fs::remove_dir_all(&st);
}

#[test]
fn a_box_only_one_node_has_shown_is_not_spent() {
    let st = state("seen");
    let path = format!("{st}/wallet.json");
    let mut w = crate::walletfile::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
    // the wallet's one box, as a proof above the agreed tip leaves it: tracked, and not confirmed
    for b in w.boxes.values_mut() {
        b.confirmed_at = None;
    }
    std::fs::write(&path, crate::walletfile::text(&w).unwrap()).unwrap();
    let e = run(
        &st,
        &[
            "send",
            "--to",
            &text(&address(1)),
            "--nanoerg",
            "1000000000",
        ],
    )
    .unwrap_err();
    assert!(
        e.contains("confirmed under a tip the peers agree on"),
        "{e}"
    );
    assert!(signed(&st).is_none());
    let _ = std::fs::remove_dir_all(&st);
}

#[test]
fn a_spending_policy_holds_what_send_signs() {
    let st = state("policy");
    let (payee, stranger) = (text(&address(1)), text(&address(2)));
    std::fs::write(policy::path(&st), serde_json::json!({ "max_per_payment_nanoerg": ERG / 2, "max_per_day_nanoerg": ERG, "allowed_payees": [payee] }).to_string()).unwrap();
    let send =
        |to: &str, nano: u64| run(&st, &["send", "--to", to, "--nanoerg", &nano.to_string()]);
    assert!(
        send(&payee, ERG / 10)
            .unwrap_err()
            .contains("no payments log"),
        "a policy with no log to count in allows nothing"
    );
    std::fs::write(policy::log_path(&st), "").unwrap();
    assert!(send(&payee, ERG)
        .unwrap_err()
        .contains("over the per-payment limit"));
    assert!(
        send(&payee, ERG / 2)
            .unwrap_err()
            .contains("over the per-payment limit"),
        "the fee counts"
    );
    assert!(send(&stranger, ERG / 10)
        .unwrap_err()
        .contains("not on the policy's list of payees"));
    assert!(signed(&st).is_none(), "nothing refused was signed");
    assert_eq!(
        std::fs::read_to_string(policy::log_path(&st)).unwrap(),
        "",
        "and nothing refused was counted"
    );
    send(&payee, ERG / 10).unwrap();
    assert!(signed(&st).is_some());
    let log = std::fs::read_to_string(policy::log_path(&st)).unwrap();
    let first: serde_json::Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    assert_eq!(
        first["nanoerg"].as_u64(),
        Some(ERG / 10 + FEE),
        "what left the wallet, fee included, is on the log"
    );
    // the day's limit counts what was signed, sent or not
    send(&payee, ERG / 2 - FEE).unwrap();
    assert!(send(&payee, ERG / 2 - FEE)
        .unwrap_err()
        .contains("over the daily limit"));
    let _ = std::fs::remove_dir_all(&st);
}

/// A policy whose list of payees is under a name one letter short was a policy with no list: the stranger was paid.
#[test]
fn a_policy_with_a_rule_misspelt_signs_nothing() {
    let st = state("policy-misspelt");
    let (payee, stranger) = (text(&address(1)), text(&address(2)));
    std::fs::write(policy::path(&st), serde_json::json!({ "max_per_payment_nanoerg": ERG, "max_per_day_nanoerg": ERG, "allowed_payee": [payee] }).to_string()).unwrap();
    std::fs::write(policy::log_path(&st), "").unwrap();
    for to in [&stranger, &payee] {
        let refused = run(
            &st,
            &["send", "--to", to, "--nanoerg", &(ERG / 10).to_string()],
        )
        .unwrap_err();
        assert!(
            refused.contains("spending policy") && refused.contains("not a rule"),
            "{refused}"
        );
    }
    assert!(signed(&st).is_none(), "nothing was signed");
    assert_eq!(
        std::fs::read_to_string(policy::log_path(&st)).unwrap(),
        "",
        "and nothing counted"
    );
    let _ = std::fs::remove_dir_all(&st);
}

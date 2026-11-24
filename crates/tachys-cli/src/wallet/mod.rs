//! tachys wallet: keys, addresses, the boxes this wallet tracks, signing and sending.
//!
//! Here: what every `wallet` subcommand works from (`Ctx`, `Open`), the door they come in by (`wallet`), and the small
//! ones that make, name and show a wallet. A signing request is read and signed in `request.rs`; a payment is built,
//! held to the spending policy and handed on in `send.rs`; the wallet's boxes and payments are proven and settled
//! in `settle.rs`.

use ergo_chain_types::Header;
use ergo_chain_types::PreHeader;
use ergo_lib::chain::ergo_box::box_builder::ErgoBoxCandidateBuilder;
use ergo_lib::chain::ergo_state_context::ErgoStateContext;
use ergo_lib::chain::parameters::Parameters;
use ergo_lib::chain::transaction::unsigned::UnsignedTransaction;
use ergo_lib::chain::transaction::Transaction;
use ergo_lib::wallet::box_selector::{
    BoxSelection, BoxSelector, ErgoBoxAssetsData, SimpleBoxSelector,
};
use ergo_lib::wallet::derivation_path::DerivationPath;
use ergo_lib::wallet::ext_secret_key::ExtSecretKey;
use ergo_lib::wallet::mnemonic::Mnemonic;
use ergo_lib::wallet::mnemonic_generator::{Language, MnemonicGenerator};
use ergo_lib::wallet::secret_key::SecretKey;
use ergo_lib::wallet::signing::TransactionContext;
use ergo_lib::wallet::tx_builder::TxBuilder;
use ergo_lib::wallet::Wallet;
use ergotree_ir::chain::address::{Address, AddressEncoder, NetworkPrefix};
use ergotree_ir::chain::context_extension::ContextExtension;
use ergotree_ir::chain::ergo_box::box_value::BoxValue;
use ergotree_ir::chain::ergo_box::ErgoBox;
use ergotree_ir::chain::ergo_box::NonMandatoryRegisterId::{R4, R5, R6};
use ergotree_ir::chain::ergo_box::{BoxId, BoxTokens, ErgoBoxCandidate};
use ergotree_ir::chain::token::{Token, TokenAmount, TokenId};
use ergotree_ir::ergo_tree::ErgoTree;
use ergotree_ir::mir::constant::{Constant, TryExtractInto};
use ergotree_ir::serialization::SigmaSerializable;
use std::str::FromStr;

use crate::chain::{agreed_height, chain_bytes, load_chain};
use crate::network::{net, private_net};
use crate::out::{Ended, Fail};
use crate::policy;
use crate::proofs::{
    extend_chain, prove_any, prove_any_agreed, prove_included, prove_p2p, prove_reading, send_tx,
    NotSent, Proven, RestCost,
};
use crate::seal::{open_phrase, phrase_problem, seal_phrase};
use crate::util::{
    arg, clip, hex, http, one_line, private_dir, stdin_line, unhex, write_private, write_whole,
};
use crate::walletfile::{Asset, Done, Pending, TrackedBox, WalletFile};

mod request;
mod send;
mod settle;
#[cfg(test)]
mod tests;

use self::request::*;
use self::send::*;
pub(crate) use self::settle::*;

/// Does a spending condition name one of these keys? True wherever the key appears, alone or inside an AND, OR or
/// threshold: if this wallet's signature can be part of what unlocks a box, the box is counted as this wallet's.
pub(crate) fn names(
    p: &ergotree_ir::sigma_protocol::sigma_boolean::SigmaBoolean,
    keys: &[ergotree_ir::sigma_protocol::sigma_boolean::ProveDlog],
) -> bool {
    use ergotree_ir::sigma_protocol::sigma_boolean::{
        SigmaBoolean, SigmaConjecture, SigmaProofOfKnowledgeTree,
    };
    match p {
        SigmaBoolean::TrivialProp(_) => false,
        SigmaBoolean::ProofOfKnowledge(SigmaProofOfKnowledgeTree::ProveDlog(d)) => keys.contains(d),
        SigmaBoolean::ProofOfKnowledge(SigmaProofOfKnowledgeTree::ProveDhTuple(_)) => false,
        SigmaBoolean::SigmaConjecture(SigmaConjecture::Cand(c)) => {
            c.items.iter().any(|x| names(x, keys))
        }
        SigmaBoolean::SigmaConjecture(SigmaConjecture::Cor(c)) => {
            c.items.iter().any(|x| names(x, keys))
        }
        SigmaBoolean::SigmaConjecture(SigmaConjecture::Cthreshold(c)) => {
            c.children.iter().any(|x| names(x, keys))
        }
    }
}

/// The script of a Babel fee offer (EIP-31) for a token, in hex: ERG locked under it may be taken by putting enough of
/// that token back into the same box, at the price in R5.
pub(crate) fn babel_hex(token: &str) -> String {
    format!("100604000e20{token}0400040005000500d803d601e30004d602e4c6a70408d603e4c6a7050595e67201d804d604b2a5e4720100d605b2db63087204730000d606db6308a7d60799c1a7c17204d1968302019683050193c27204c2a7938c720501730193e4c672040408720293e4c672040505720393e4c67204060ec5a796830201929c998c7205029591b1720673028cb272067303000273047203720792720773057202")
}

/// The script every network fee is paid to, in hex.
pub(crate) fn fee_tree_hex() -> String {
    if net().fee_tree.is_empty() {
        ergo_lib::wallet::miner_fee::MINERS_FEE_BASE16_BYTES.to_string()
    } else {
        net().fee_tree.to_string()
    }
}

/// The payments this wallet holds that pay it boxes it expects and are not its own, in `DIR/outbox` or set aside in
/// `DIR/given-up` (still judged: one set aside may yet confirm, or be dead): (the file, its JSON, every coin it spends,
/// the boxes it promises this wallet). A payment of this wallet's own (it is waiting) is left to its own bookkeeping.
type Held = (
    std::path::PathBuf,
    serde_json::Value,
    Vec<String>,
    Vec<String>,
);
fn held_promises(state: &str, w: &WalletFile) -> Vec<Held> {
    let own: Vec<&String> = w.pending.iter().map(|p| &p.tx).collect();
    let files: Vec<std::path::PathBuf> = ["outbox", "given-up"]
        .iter()
        .flat_map(|d| {
            std::fs::read_dir(format!("{state}/{d}"))
                .map(|d| {
                    d.filter_map(|e| e.ok().map(|e| e.path()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .collect();
    files
        .into_iter()
        .filter_map(|f| {
            let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&f).ok()?).ok()?;
            let tx = ergo_lib::chain::transaction::Transaction::sigma_parse_bytes(&unhex(
                v["tx_bytes"].as_str()?,
            )?)
            .ok()?;
            if own.contains(&&String::from(tx.id())) {
                return None;
            }
            // what it promises: boxes expected, or held as void (which come back if its coins do)
            let outs: Vec<String> = tx
                .outputs
                .iter()
                .map(|o| String::from(o.box_id()))
                .filter(|o| w.expect.contains(o) || w.void.contains(o))
                .collect();
            if outs.is_empty() {
                return None;
            }
            Some((
                f,
                v,
                tx.inputs.iter().map(|i| String::from(i.box_id)).collect(),
                outs,
            ))
        })
        .collect()
}

/// Box `id` as the payment that makes it would make it, read from that signed payment: one of this wallet's own still
/// waiting, or one it holds for someone else (`held_promises`). `None` if no such payment makes it. Not in a block yet,
/// and only as good as that payment is known to be.
pub(crate) fn expected_box(state: &str, w: &WalletFile, id: &str) -> Option<ErgoBox> {
    let own = w.pending.iter().filter_map(|p| {
        serde_json::from_str::<ergo_lib::chain::transaction::Transaction>(&p.json).ok()
    });
    let held = held_promises(state, w)
        .into_iter()
        .filter_map(|(_, v, _, _)| {
            ergo_lib::chain::transaction::Transaction::sigma_parse_bytes(&unhex(
                v["tx_bytes"].as_str()?,
            )?)
            .ok()
        });
    own.chain(held)
        .flat_map(|t| t.outputs.into_iter().collect::<Vec<_>>())
        .find(|o| String::from(o.box_id()) == id)
}

/// The payment that makes box `id`, from a node's pool of payments not yet in a block, and whether any node given
/// answered at all. A node's word that it holds the payment is all this rests on, and only the payment's body is
/// checked here: a box's id is a hash over its payment's id and the box, and the payment's id a hash over the payment
/// without its signatures, so a payment taken here is that payment's body only if, as read, it makes the box `id`. Its
/// signatures are the caller's to verify. Nodes over the peer protocol are not asked.
pub(crate) fn pending_payment(
    nodes: &[String],
    id: &str,
) -> (Option<ergo_lib::chain::transaction::Transaction>, bool) {
    let mut answered = false;
    for n in nodes.iter().filter(|n| !n.starts_with("p2p://")) {
        let r = match http()
            .get(&format!(
                "{n}/transactions/unconfirmed/outputs/byBoxId/{id}"
            ))
            .call()
        {
            Ok(r) => r,
            Err(ureq::Error::Status(404, _)) => {
                answered = true;
                continue;
            }
            Err(_) => continue,
        };
        answered = true;
        let Some(out) = r
            .into_string()
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        else {
            continue;
        };
        let Some(txid) = out["transactionId"]
            .as_str()
            .filter(|t| t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit()))
        else {
            continue;
        };
        let Some(tx) = http()
            .get(&format!(
                "{n}/transactions/unconfirmed/byTransactionId/{txid}"
            ))
            .call()
            .ok()
            .and_then(|r| r.into_string().ok())
            .and_then(|t| {
                serde_json::from_str::<ergo_lib::chain::transaction::Transaction>(&t).ok()
            })
        else {
            continue;
        };
        if tx.outputs.iter().any(|o| String::from(o.box_id()) == id) {
            return (Some(tx), true);
        }
    }
    (None, answered)
}

/// Hold a payment someone else made to this wallet (its signed bytes, in hex): keep it in `DIR/outbox`, to be submitted
/// by this wallet itself, and expect the boxes it pays this wallet. Nothing is accepted by this; the wallet file is
/// changed in `w` and saved by the caller. Returns the boxes promised, their nanoERG and how many tokens they hold.
/// `seen`: coins the payment spends that the caller has just proven unspent (kept with it as `coins_seen`: one of them
/// gone later means the payment is dead; a coin never seen may only not have been made yet).
fn hold(
    state: &str,
    w: &mut WalletFile,
    my_trees: &[ErgoTree],
    hexs: &str,
    seen: &[String],
    seen_at: u64,
) -> Result<(Vec<String>, u64, usize), String> {
    let Some(tx) = unhex(hexs)
        .and_then(|b| ergo_lib::chain::transaction::Transaction::sigma_parse_bytes(&b).ok())
    else {
        return Err("not a transaction".into());
    };
    let mine: Vec<&ErgoBox> = tx
        .outputs
        .iter()
        .filter(|o| my_trees.contains(&o.ergo_tree))
        .collect();
    if mine.is_empty() {
        return Err("that transaction pays nothing to this wallet".into());
    }
    let json = serde_json::to_string(&tx).unwrap_or_default();
    let id = String::from(tx.id());
    // this file may be the only copy of the payment: nothing is promised unless it is on disk
    let kept = std::fs::create_dir_all(format!("{state}/outbox")).and_then(|_| write_whole(&format!("{state}/outbox/{id}.json"), serde_json::json!({ "tx_id": id, "tx_bytes": hexs, "tx_json": json, "proven": !seen.is_empty(), "coins_seen": seen.iter().map(|c| (c.clone(), serde_json::json!(seen_at))).collect::<serde_json::Map<_, _>>() }).to_string().as_bytes()));
    if let Err(e) = kept {
        return Err(format!(
            "could not keep the transaction in {state}/outbox ({e}); nothing promised"
        ));
    }
    for b in &mine {
        let bid = String::from(b.box_id());
        if !w.expect.contains(&bid) {
            w.expect.push(bid);
        }
    }
    let value: u64 = mine.iter().map(|b| *b.value.as_u64()).sum();
    let tokens: usize = mine
        .iter()
        .map(|b| b.tokens.as_ref().map_or(0, |t| t.len()))
        .sum();
    Ok((
        mine.iter().map(|b| String::from(b.box_id())).collect(),
        value,
        tokens,
    ))
}

#[cfg(test)]
pub(crate) fn tracked_count(w: &WalletFile) -> usize {
    w.boxes.len()
}

/// What every `wallet` subcommand works from: the command line, the state directory it names, the agreed chain kept
/// there, and the password if one was given.
struct Ctx<'a> {
    args: &'a [String],
    sub: &'a str,
    state: String,
    node: String,
    /// the wallet file
    path: String,
    chain: Vec<Header>,
    enc: AddressEncoder,
    /// The password that seals and opens the phrase. On stdin it is the first line; a phrase given as `--mnemonic-file -` is the next.
    pass: Option<Secret>,
}

/// A password or a seed phrase: wiped from memory when it goes out of use. (What the signing library makes of a
/// phrase, the seed and the keys, is its own to wipe; this covers what this wallet holds.)
type Secret = zeroize::Zeroizing<String>;

impl<'a> Ctx<'a> {
    fn new(args: &'a [String]) -> Result<Self, Fail> {
        let state: String = arg(args, "--state", "state".to_string())?;
        Ok(Ctx {
            args,
            sub: args.get(2).map(|s| s.as_str()).unwrap_or(""),
            // `--node auto`: the best few nodes found (`nodes.rs`), as a list given by hand would be
            node: crate::nodes::resolve(&arg(args, "--node", String::new())?, &state)?,
            path: format!("{state}/wallet.json"),
            chain: load_chain(&state),
            enc: AddressEncoder::new(if net().testnet {
                NetworkPrefix::Testnet
            } else {
                NetworkPrefix::Mainnet
            }),
            pass: (if args.iter().any(|a| a == "--pass-stdin") {
                Some(stdin_line())
            } else {
                std::env::var("TACHYS_PASS").ok()
            })
            .filter(|p| !p.is_empty())
            .map(Secret::new),
            state,
        })
    }

    /// The wallet file: `None` only if there is no file. A file that is there but cannot be read as a wallet is an
    /// error, never "no wallet": `new` and `restore` write where there is none, and the file may hold the only copy of a seed phrase.
    fn read(&self) -> Result<Option<WalletFile>, String> {
        let path = &self.path;
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{path} cannot be read ({e}); it was not touched")),
        };
        crate::walletfile::parse(&text).map(Some).map_err(|why| format!("{path} exists but is not a wallet file this build can read: {why}. It was not touched. It may hold a seed phrase: move it aside by hand before making a new wallet here"))
    }
    fn load(&self) -> Result<Option<WalletFile>, Fail> {
        self.read().map_err(Fail::from)
    }

    /// Written beside the file and moved into place, so a crash mid-write cannot leave half a wallet (and half a sealed
    /// phrase), and readable by its owner alone: it holds the seed phrase, sealed or not.
    fn save(&self, w: &WalletFile) -> Ended {
        let path = &self.path;
        private_dir(&self.state);
        let Ok(text) = crate::walletfile::text(w) else {
            return Err(crate::out::Fail::new(1, format!("could not write {path}")));
        };
        crate::util::write_whole(path, text.as_bytes())
            .map_err(|e| crate::out::Fail::new(1, format!("could not write {path} ({e})")))
    }

    /// A phrase someone typed is checked before it is used (word count, word list, checksum): a slip would otherwise open
    /// an empty wallet at another address and look like lost funds. `--any-phrase` takes it as it is.
    fn checked_phrase(&self, phrase: String) -> Result<String, Fail> {
        if self.args.iter().any(|a| a == "--any-phrase") {
            return Ok(phrase);
        }
        if let Some(why) = phrase_problem(&phrase) {
            return Err(crate::out::Fail::new(
                2,
                format!("seed phrase not accepted: {why}"),
            ));
        }
        Ok(phrase
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(if phrase.contains('\u{3000}') {
                "\u{3000}"
            } else {
                " "
            }))
    }

    /// Puts the phrase in the wallet file: sealed if there is a password, in plain text if not.
    fn keep(&self, w: &mut WalletFile, phrase: &str) -> Ended {
        (w.mnemonic, w.mnemonic_file, w.secret, w.warning) = (None, None, None, None);
        match self.pass.as_deref() {
            Some(pw) => {
                let sealed =
                    seal_phrase(phrase, pw).filter(|s| open_phrase(s, pw).as_deref() == Ok(phrase));
                let Some(sealed) = sealed else {
                    return Err(crate::out::Fail::new(
                        1,
                        "could not seal the seed phrase; nothing written".to_string(),
                    ));
                };
                w.secret = Some(sealed);
            }
            None => {
                w.mnemonic = Some(phrase.to_string());
                w.warning = Some("seed phrase stored in plain text: no password was given".into());
            }
        }
        Ok(())
    }
}

/// The wallet's keys: m/44'/429'/0'/0/i, the same path every Ergo wallet uses, so a phrase restores anywhere.
fn key_at(phrase: &str, i: u64) -> Option<ExtSecretKey> {
    let master = ExtSecretKey::derive_master(Mnemonic::to_seed(phrase, "")).ok()?;
    master
        .derive(DerivationPath::from_str(&format!("m/44'/429'/0'/0/{i}")).ok()?)
        .ok()
}
/// The first address of a phrase.
fn derive(phrase: &str) -> Option<Address> {
    Some(Address::P2Pk(key_at(phrase, 0)?.public_image()))
}
/// The signing keys of the first `n_addr` addresses of a phrase.
fn secrets(phrase: &str, n_addr: u64) -> Vec<SecretKey> {
    (0..n_addr)
        .filter_map(|i| key_at(phrase, i).map(|k| k.secret_key()))
        .collect()
}
fn read_phrase(file: &str) -> Result<String, Fail> {
    if file == "-" {
        return Ok(stdin_line());
    }
    Ok(std::fs::read_to_string(file)
        .map_err(|e| crate::out::Fail::new(2, format!("--mnemonic-file {file}: {e}")))?
        .trim()
        .to_string())
}

/// A wallet file that has been read, and what follows from it: its phrase if it can be opened, its addresses and
/// their scripts.
struct Open {
    w: WalletFile,
    sealed: bool,
    phrase: Option<Secret>,
    /// sealed, and no password came with the command, or one that does not open it
    locked: bool,
    /// a password came with the command and does not open the phrase
    bad_password: bool,
    /// why there is no phrase, for a command that needs one
    no_phrase: &'static str,
    addr: Address,
    my_tree: ErgoTree,
    /// a restored wallet not yet looked over
    scanning: bool,
    n_addr: u64,
    my_addrs: Vec<Address>,
    my_trees: Vec<ErgoTree>,
    addr_strs: Vec<String>,
}

/// tachys wallet new | receive | status | send ... With a password (`--pass-stdin`, or TACHYS_PASS) the seed phrase
/// is kept sealed in the wallet file and is opened only by a command given that password again; without one it is
/// kept in plain text, as before.
pub(crate) fn wallet(args: &[String]) -> Ended {
    let c = Ctx::new(args)?;
    // One command at a time writes a wallet (`util::turn`): its turn is taken before the file is read and kept until
    // the command ends. The ones that only read it take none, so the words or the key can be shown beside a sync.
    // (A directory that is not there has no wallet to take a turn at, unless this command is about to make one.)
    let makes = matches!(c.sub, "new" | "restore" | "watch");
    let reads_only = matches!(c.sub, "derive" | "mnemonic" | "pubkey" | "babel-list")
        || (c.sub == "address" && !args.iter().any(|a| a == "--add"));
    if makes {
        private_dir(&c.state);
    }
    let _turn = if !reads_only && std::path::Path::new(&c.state).is_dir() {
        Some(crate::util::turn(&c.state).map_err(Fail::from)?)
    } else {
        None
    };
    match c.sub {
        "derive" => return cmd_derive(&c),
        "new" => return cmd_new(&c),
        "watch" => return cmd_watch(&c),
        // a restore writes the wallet file and then goes on to look for its boxes, as `status` does
        "restore" => cmd_restore(&c)?,
        _ => {}
    }
    let o = open(&c)?;
    match c.sub {
        "encrypt" => cmd_encrypt(&c, o),
        "name" => cmd_name(&c, o),
        "address" => cmd_address(&c, o),
        "mnemonic" => cmd_mnemonic(&c, o),
        "pubkey" => cmd_pubkey(o),
        "promise" => cmd_promise(&c, o),
        // a refusal comes back as the reason; here it is given the words it is known by
        "request" => cmd_request(&c, o).map_err(|f| { if let Some(m) = &f.msg { say!("request refused: {m}"); } Fail::said(f.code) }),
        "send" | "consolidate" | "burn" | "babel-offer" | "babel-list" => cmd_send(&c, o).map_err(|f| Fail { code: f.code, msg: f.msg.map(|m| format!("send: {m}")) }),
        "receive" | "status" | "restore" => {
            cmd_sync(&c, o)?;
            // A sync that saw another wallet's hand (a box gone that no payment of ours spent, a payment the proof could
            // not settle) looks again at once, this time asking the nodes what the addresses hold: the balance it
            // leaves is then the true one, not the one from before the other wallet's coins were found.
            if c.sub == "status" && c.load()?.is_some_and(|w| w.refresh) { crate::out::unset("checked"); cmd_sync(&c, open(&c)?)?; }
            Ok(())
        }
        other => Err(crate::out::Fail::new(2, format!("tachys wallet: no such command \"{}\"\nusage: tachys wallet new|restore|watch|derive|receive|status|send|request|promise|consolidate|burn|babel-offer|babel-list|address|name|encrypt|mnemonic|pubkey ...", clip(other, 40)))),
    }
}

/// wallet derive: the address a phrase leads to.
fn cmd_derive(c: &Ctx) -> Ended {
    let Ctx { args, enc, .. } = c;
    // the address a phrase leads to, without making a wallet: lets a caller see whether it already has that wallet
    let file: String = arg(args, "--mnemonic-file", String::new())?;
    let phrase = Secret::new(c.checked_phrase(read_phrase(&file)?)?);
    let Some(addr) = derive(&phrase) else {
        return Err(crate::out::Fail::new(
            1,
            "key derivation failed".to_string(),
        ));
    };
    crate::out::set("address", enc.address_to_str(&addr));
    say!("address {}", enc.address_to_str(&addr));
    Ok(())
}

/// wallet new: a new phrase, its first address, and a wallet file.
fn cmd_new(c: &Ctx) -> Ended {
    let Ctx {
        args,
        path,
        chain,
        enc,
        pass,
        ..
    } = c;
    if c.load()?.is_some() {
        return Err(crate::out::Fail::new(
            2,
            format!("{path} already exists; not overwriting"),
        ));
    }
    let phrase: Secret = match MnemonicGenerator::new(Language::English, 128) {
        Ok(g) => Secret::new(g.generate()),
        Err(e) => return Err(crate::out::Fail::new(1, format!("mnemonic: {e:?}"))),
    };
    let Some(addr) = derive(&phrase) else {
        return Err(crate::out::Fail::new(
            1,
            "key derivation failed".to_string(),
        ));
    };
    let birthday = chain.first().map(|h| h.height).unwrap_or(0);
    let a = enc.address_to_str(&addr);
    let mut w = WalletFile {
        address_list: Some(vec![a.clone()]),
        birthday: birthday as u64,
        ..WalletFile::new(arg(args, "--name", "Wallet".to_string())?, a.clone())
    };
    c.keep(&mut w, &phrase)?;
    c.save(&w)?;
    crate::out::set("address", &a);
    crate::out::set("birthday", birthday);
    crate::out::set("sealed", pass.is_some());
    say!("new wallet: address {a}\nbirthday height {birthday} (nothing before it is ever looked for)\nsaved to {path}{}", if pass.is_some() { "\nseed phrase sealed with the password given" } else { "\nseed phrase in plain text (no password given)" });
    Ok(())
}

/// wallet restore: a wallet file from a phrase. The boxes are looked for afterwards (`cmd_sync`).
fn cmd_restore(c: &Ctx) -> Ended {
    let Ctx {
        args, path, enc, ..
    } = c;
    // Restore = seed phrase -> addresses -> current boxes. No history is fetched or kept.
    let file: String = arg(args, "--mnemonic-file", String::new())?;
    let phrase = Secret::new(c.checked_phrase(read_phrase(&file)?)?);
    let Some(addr) = derive(&phrase) else {
        return Err(crate::out::Fail::new(
            1,
            "key derivation failed".to_string(),
        ));
    };
    let a = enc.address_to_str(&addr);
    // the phrase is referenced, not copied, unless told to keep it (--embed: a phone has nowhere else to hold it)
    let embed = args.iter().any(|x| x == "--embed");
    if file == "-" && !embed {
        return Err(crate::out::Fail::new(
            2,
            "a phrase given on stdin has to be kept in the wallet file: add --embed".to_string(),
        ));
    }
    // A restore does not know how many addresses were in use. All twenty are written down and the wallet is marked;
    // the first sync that asks a node what each holds keeps those up to the last one with coins (see `scan_addresses` below).
    let look_at_all = |w: &mut WalletFile| {
        let all: Vec<String> = (0..20)
            .filter_map(|i| {
                key_at(&phrase, i).map(|k| enc.address_to_str(&Address::P2Pk(k.public_image())))
            })
            .collect();
        if all.first() == Some(&a) {
            w.address_list = Some(all);
            w.scan_addresses = true;
        }
    };
    // --reseal: the wallet is already here but its phrase can no longer be opened (its password is gone); the same phrase puts it back
    let reseal = args.iter().any(|x| x == "--reseal");
    match c.load()? {
        // restoring over a read-only wallet of the same address gives it its key and keeps what it already tracks
        Some(mut w) if w.address == a && (w.watch || reseal) => {
            let was_watch = std::mem::take(&mut w.watch);
            if embed {
                c.keep(&mut w, &phrase)?;
            } else {
                (w.mnemonic, w.secret, w.warning) = (None, None, None);
                w.mnemonic_file = Some(file.clone());
            }
            if w.address_list.is_none() {
                w.address_list = Some(vec![a.clone()]);
            }
            if was_watch {
                look_at_all(&mut w);
            }
            w.restored = true;
            c.save(&w)?;
            crate::out::set("address", &a);
            crate::out::set("restored", if was_watch { "upgraded" } else { "resealed" });
            say!(
                "{}: address {a}",
                if was_watch {
                    "upgraded read-only wallet"
                } else {
                    "resealed wallet"
                }
            );
        }
        Some(_) => {
            return Err(crate::out::Fail::new(
                2,
                format!(
                    "{path} already exists and is not a read-only wallet of {a}; not overwriting"
                ),
            ))
        }
        None => {
            let mut w = WalletFile {
                address_list: Some(vec![a.clone()]),
                restored: true,
                ..WalletFile::new(arg(args, "--name", "Wallet".to_string())?, a.clone())
            };
            if embed {
                c.keep(&mut w, &phrase)?;
            } else {
                w.mnemonic_file = Some(file.clone());
            }
            look_at_all(&mut w);
            c.save(&w)?;
            crate::out::set("address", &a);
            crate::out::set("restored", "new");
            say!("restored wallet: address {a}");
        }
    }
    Ok(())
}

/// wallet watch: a read-only wallet.
fn cmd_watch(c: &Ctx) -> Ended {
    let Ctx {
        args, path, enc, ..
    } = c;
    // a read-only wallet: an address and nothing else; no key ever touches this device
    if c.load()?.is_some() {
        return Err(crate::out::Fail::new(
            2,
            format!("{path} already exists; not overwriting"),
        ));
    }
    let a: String = arg(args, "--address", String::new())?;
    if enc.parse_address_from_str(&a).is_err() {
        return Err(crate::out::Fail::new(
            2,
            format!("--address is not a {} address", net().name),
        ));
    }
    let name: String = arg(args, "--name", "Wallet".to_string())?;
    c.save(&WalletFile {
        watch: true,
        ..WalletFile::new(name, a.clone())
    })?;
    crate::out::set("address", &a);
    crate::out::set("watch", true);
    say!("watching {a}");
    Ok(())
}

/// Reads the wallet file and opens its phrase if it can: every subcommand below starts from this.
fn open(c: &Ctx) -> Result<Open, Fail> {
    let Ctx {
        state,
        path,
        enc,
        pass,
        ..
    } = c;
    let Some(w) = c.load()? else {
        return Err(crate::out::Fail::new(
            2,
            format!("no wallet at {path}; run `tachys wallet new --state {state}`"),
        ));
    };
    // A sealed phrase is opened only when the password comes with the command. Without it the wallet still knows its
    // addresses (they are written beside the sealed phrase), so it can watch, prove and receive; it cannot sign or show the words.
    // A wrong password leaves it as closed as no password does (`bad_password` says so): a command that needs the phrase
    // is refused, and one that does not goes on. A sealed phrase that cannot be read at all is an error for every command.
    let sealed = w.secret.as_ref().is_some_and(|s| s.is_object());
    let mut bad_password = false;
    let phrase: Option<Secret> = if sealed {
        match pass
            .as_deref()
            .map(|pw| open_phrase(w.secret.as_ref().unwrap_or(&serde_json::Value::Null), pw))
        {
            None => None,
            Some(Ok(p)) => Some(Secret::new(p)),
            Some(Err(e)) if e == "bad wallet password" => {
                crate::out::set("bad_password", true);
                bad_password = true;
                None
            }
            Some(Err(e)) => {
                return Err(crate::out::Fail::new(
                    1,
                    format!("wallet: the seed phrase could not be opened: {e}"),
                ))
            }
        }
    } else {
        w.mnemonic
            .clone()
            .or_else(|| {
                w.mnemonic_file
                    .as_deref()
                    .and_then(|f| std::fs::read_to_string(f).ok())
                    .map(|p| p.trim().to_string())
            })
            .map(Secret::new)
    };
    let locked = sealed && phrase.is_none();
    let no_phrase = if bad_password {
        "wallet: the seed phrase could not be opened: bad wallet password"
    } else if locked {
        "the seed phrase is sealed: give the wallet's password (--pass-stdin, or TACHYS_PASS)"
    } else {
        "this wallet holds no phrase"
    };
    let Some(addr) = phrase
        .as_deref()
        .and_then(|p| derive(p))
        .or_else(|| enc.parse_address_from_str(&w.address).ok())
    else {
        return Err(crate::out::Fail::new(
            1,
            "wallet file has no usable seed phrase or address".to_string(),
        ));
    };
    if sealed && w.address != enc.address_to_str(&addr) {
        return Err(crate::out::Fail::new(
            1,
            "wallet: the sealed phrase does not lead to this wallet's address; refusing to use it"
                .to_string(),
        ));
    }
    let Ok(my_tree) = addr.script() else {
        return Err(crate::out::Fail::new(
            1,
            "address has no script".to_string(),
        ));
    };
    // Further addresses of the same phrase (indices 1..). Everything at any of them is this wallet's; change goes to the first.
    let listed: Vec<Address> = w
        .address_list
        .iter()
        .flatten()
        .filter_map(|a| enc.parse_address_from_str(a).ok())
        .take(20)
        .collect();
    // A restored wallet not yet looked over has all twenty listed, with or without the phrase to hand.
    // A restored wallet that found all twenty empty kept the first only, so a payment made later to another of them went
    // unseen. Asked to look again (`status --reconcile`, or a sync after a sign that another wallet of the phrase is at
    // work), it looks over all twenty again and keeps any up to the last that holds a box; never fewer than before.
    let rescan = c.sub == "status"
        && w.restored
        && !w.scan_addresses
        && phrase.is_some()
        && (c.args.iter().any(|a| a == "--reconcile") || w.refresh);
    let scanning = w.scan_addresses || rescan;
    let n_addr = if scanning && phrase.is_some() {
        20
    } else if phrase.is_some() {
        w.addresses.unwrap_or(1).clamp(1, 20)
    } else if locked {
        (listed.len() as u64).max(1)
    } else {
        1
    };
    let my_addrs: Vec<Address> = match phrase.as_deref() {
        Some(p) => (0..n_addr)
            .filter_map(|i| key_at(p, i).map(|k| Address::P2Pk(k.public_image())))
            .collect(),
        None if locked && listed.first() == Some(&addr) => listed,
        None => vec![addr.clone()],
    };
    let my_trees: Vec<_> = my_addrs.iter().filter_map(|a| a.script().ok()).collect();
    let addr_strs: Vec<String> = my_addrs.iter().map(|a| enc.address_to_str(a)).collect();
    Ok(Open {
        w,
        sealed,
        phrase,
        locked,
        bad_password,
        no_phrase,
        addr,
        my_tree,
        scanning,
        n_addr,
        my_addrs,
        my_trees,
        addr_strs,
    })
}

/// wallet encrypt: seal a phrase that is in plain text.
fn cmd_encrypt(c: &Ctx, o: Open) -> Ended {
    let Open {
        mut w,
        sealed,
        addr_strs,
        ..
    } = o;
    let pass = &c.pass;
    // a phrase kept in plain text becomes a sealed one; `keep` opens what it sealed before anything is written
    if sealed {
        crate::out::set("sealed", true);
        say!("already sealed: {}", addr_strs[0]);
        return Ok(());
    }
    if pass.is_none() {
        return Err(crate::out::Fail::new(
            2,
            "encrypt needs the password to seal with (--pass-stdin, or TACHYS_PASS)".to_string(),
        ));
    }
    let Some(p) = w.mnemonic.clone().map(Secret::new) else {
        return Err(crate::out::Fail::new(
            2,
            "this wallet file holds no phrase of its own to seal".to_string(),
        ));
    };
    c.keep(&mut w, &p)?;
    w.address_list = Some(addr_strs.clone());
    c.save(&w)?;
    crate::out::set("sealed", true);
    say!(
        "sealed: the seed phrase of {} is no longer in plain text",
        addr_strs[0]
    );
    Ok(())
}

/// wallet name: give the wallet a name.
fn cmd_name(c: &Ctx, o: Open) -> Ended {
    let (args, mut w) = (c.args, o.w);
    w.name = arg(args, "--name", "Wallet".to_string())?;
    c.save(&w)?;
    crate::out::set("name", &w.name);
    say!("named {}", w.name);
    Ok(())
}

/// wallet address: list the wallet's addresses, or add the next one (`--add`).
fn cmd_address(c: &Ctx, o: Open) -> Ended {
    let Ctx { args, enc, .. } = c;
    let Open {
        mut w,
        phrase,
        locked,
        bad_password,
        no_phrase,
        n_addr,
        addr_strs,
        ..
    } = o;
    if args.iter().any(|a| a == "--add") {
        let Some(p) = phrase.as_deref() else {
            return Err(crate::out::Fail::new(
                if bad_password { 1 } else { 2 },
                (if locked {
                    no_phrase
                } else {
                    "a read-only wallet has one address"
                })
                .to_string(),
            ));
        };
        if n_addr >= 20 {
            return Err(crate::out::Fail::new(
                2,
                "twenty addresses is this build's limit".to_string(),
            ));
        }
        let Some(k) = key_at(p, n_addr) else {
            return Err(crate::out::Fail::new(
                1,
                "key derivation failed".to_string(),
            ));
        };
        let mut list = addr_strs.clone();
        list.push(enc.address_to_str(&Address::P2Pk(k.public_image())));
        w.addresses = Some(n_addr + 1);
        w.address_list = Some(list);
        c.save(&w)?;
        crate::out::set(
            "added",
            enc.address_to_str(&Address::P2Pk(k.public_image())),
        );
        crate::out::set("addresses", &w.address_list);
        say!(
            "address {}: {}",
            n_addr,
            enc.address_to_str(&Address::P2Pk(k.public_image()))
        );
    } else {
        crate::out::set("addresses", &addr_strs);
        for (i, a) in addr_strs.iter().enumerate() {
            say!("address {i}: {a}");
        }
    }
    Ok(())
}

/// wallet mnemonic: show the seed phrase.
fn cmd_mnemonic(c: &Ctx, o: Open) -> Ended {
    let state = &c.state;
    let Open {
        phrase,
        no_phrase,
        bad_password,
        ..
    } = o;
    // for the owner to write down; the caller decides who may ask
    if policy::present(state) {
        return Err(crate::out::Fail::new(
            2,
            format!(
                "this wallet is under a spending policy ({}); it does not show its seed phrase",
                policy::path(state)
            ),
        ));
    }
    match phrase.as_deref() {
        Some(p) => {
            crate::out::set("mnemonic", p);
            if !crate::out::json() {
                say!("{p}")
            }
        }
        None => {
            return Err(crate::out::Fail::new(
                if bad_password { 1 } else { 2 },
                no_phrase.to_string(),
            ))
        }
    }
    Ok(())
}

/// wallet pubkey: the extended public key, for a watching wallet.
fn cmd_pubkey(o: Open) -> Ended {
    let Open {
        phrase,
        no_phrase,
        bad_password,
        addr,
        ..
    } = o;
    // The extended public key at m/44'/429'/0', laid out as ergo-appkit's Bip32Serialization does: it lets
    // another wallet watch every address of this one without being able to spend.
    use sha2::Digest;
    let Some(p) = phrase.as_deref() else {
        return Err(crate::out::Fail::new(
            if bad_password { 1 } else { 2 },
            no_phrase.to_string(),
        ));
    };
    let at = |path: &str| {
        ExtSecretKey::derive_master(Mnemonic::to_seed(p, ""))
            .ok()
            .and_then(|m| m.derive(DerivationPath::from_str(path).ok()?).ok())
            .and_then(|k| k.public_key().ok())
    };
    let (Some(parent), Some(grand)) = (at("m/44'/429'/0'"), at("m/44'/429'")) else {
        return Err(crate::out::Fail::new(
            1,
            "key derivation failed".to_string(),
        ));
    };
    let finger = ripemd::Ripemd160::digest(sha2::Sha256::digest(grand.pub_key_bytes()));
    let mut out: Vec<u8> = if net().testnet {
        vec![0x04, 0x35, 0x87, 0xCF]
    } else {
        vec![0x04, 0x88, 0xB2, 0x1E]
    };
    out.push(3);
    out.extend(&finger[..4]);
    out.extend([0u8; 4]);
    out.extend(parent.chain_code());
    out.extend(parent.pub_key_bytes());
    // check what is about to be shown: read the key back out of those bytes and derive the first address from it
    let back = <[u8; 33]>::try_from(&out[45..78])
        .ok()
        .zip(<[u8; 32]>::try_from(&out[13..45]).ok())
        .and_then(|(k, c)| {
            ergo_lib::wallet::ext_pub_key::ExtPubKey::new(
                k,
                c,
                DerivationPath::from_str("m/44'/429'/0'").ok()?,
            )
            .ok()
        })
        .and_then(|x| {
            x.derive(DerivationPath::from_str("m/44'/429'/0'/0/0").ok()?)
                .ok()
        })
        .map(Address::from);
    if back.as_ref() != Some(&addr) {
        return Err(crate::out::Fail::new(
            1,
            "public key does not lead back to this wallet's address; not shown".to_string(),
        ));
    }
    crate::out::set(
        "hex",
        out.iter().map(|b| format!("{b:02x}")).collect::<String>(),
    );
    crate::out::set("base58", bs58::encode(&out).with_check().into_string());
    say!(
        "hex {}",
        out.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    say!("base58 {}", bs58::encode(&out).with_check().into_string());
    Ok(())
}

/// wallet promise: keep a payer's signed transaction to submit later.
fn cmd_promise(c: &Ctx, o: Open) -> Ended {
    let Ctx { args, state, .. } = c;
    let Open {
        mut w, my_trees, ..
    } = o;
    // The payee's side of an offline payment: keep the payer's signed transaction, so this wallet can
    // submit it itself once it has network, and expect the boxes that pay it. Nothing is accepted yet:
    // the payer can still spend the same coins elsewhere until this is in a block.
    let hexs: String = arg(args, "--tx", String::new())?;
    match hold(state, &mut w, &my_trees, &hexs, &[], 0) {
        Ok((ids, value, tokens)) => {
            c.save(&w)?;
            crate::out::set("promised", &ids);
            crate::out::set("nanoerg", value);
            crate::out::set("tokens", tokens);
            say!("promised {} {} nanoERG {} tokens", ids[0], value, tokens);
            Ok(())
        }
        Err(why) => {
            say!("{why}");
            Err(crate::out::Fail::said(1))
        }
    }
}

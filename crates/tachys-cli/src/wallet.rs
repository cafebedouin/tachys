//! tachys wallet: keys, addresses, the boxes this wallet tracks, signing and sending.

use std::str::FromStr;
use ergo_chain_types::PreHeader;
use ergo_chain_types::Header;
use ergo_lib::chain::ergo_box::box_builder::ErgoBoxCandidateBuilder;
use ergo_lib::chain::ergo_state_context::ErgoStateContext;
use ergo_lib::chain::parameters::Parameters;
use ergo_lib::chain::transaction::unsigned::UnsignedTransaction;
use ergo_lib::chain::transaction::Transaction;
use ergo_lib::wallet::box_selector::{BoxSelection, BoxSelector, ErgoBoxAssetsData, SimpleBoxSelector};
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
use ergotree_ir::chain::ergo_box::NonMandatoryRegisterId::{R4, R5, R6};
use ergotree_ir::chain::ergo_box::{BoxId, BoxTokens, ErgoBoxCandidate};
use ergotree_ir::chain::token::{Token, TokenAmount, TokenId};
use ergotree_ir::chain::ergo_box::ErgoBox;
use ergotree_ir::ergo_tree::ErgoTree;
use ergotree_ir::mir::constant::{Constant, TryExtractInto};
use ergotree_ir::serialization::SigmaSerializable;

use crate::chain::{agreed_height, chain_bytes, load_chain};
use crate::network::{net, private_net};
use crate::out::{Ended, Fail};
use crate::policy;
use crate::walletfile::{Asset, Done, Pending, TrackedBox, WalletFile};
use crate::proofs::{extend_chain, prove_any, prove_any_agreed, prove_included, prove_p2p, prove_reading, send_tx, NotSent, Proven, RestCost};
use crate::seal::{open_phrase, phrase_problem, seal_phrase};
use crate::util::{arg, clip, hex, http, one_line, private_dir, stdin_line, unhex, write_private, write_whole};

/// Does a spending condition name one of these keys? True wherever the key appears, alone or inside an AND, OR or
/// threshold: if this wallet's signature can be part of what unlocks a box, the box is counted as this wallet's.
pub(crate) fn names(p: &ergotree_ir::sigma_protocol::sigma_boolean::SigmaBoolean, keys: &[ergotree_ir::sigma_protocol::sigma_boolean::ProveDlog]) -> bool {
    use ergotree_ir::sigma_protocol::sigma_boolean::{SigmaBoolean, SigmaConjecture, SigmaProofOfKnowledgeTree};
    match p {
        SigmaBoolean::TrivialProp(_) => false,
        SigmaBoolean::ProofOfKnowledge(SigmaProofOfKnowledgeTree::ProveDlog(d)) => keys.contains(d),
        SigmaBoolean::ProofOfKnowledge(SigmaProofOfKnowledgeTree::ProveDhTuple(_)) => false,
        SigmaBoolean::SigmaConjecture(SigmaConjecture::Cand(c)) => c.items.iter().any(|x| names(x, keys)),
        SigmaBoolean::SigmaConjecture(SigmaConjecture::Cor(c)) => c.items.iter().any(|x| names(x, keys)),
        SigmaBoolean::SigmaConjecture(SigmaConjecture::Cthreshold(c)) => c.children.iter().any(|x| names(x, keys)),
    }
}

/// The script of a Babel fee offer (EIP-31) for a token, in hex: ERG locked under it may be taken by putting enough of
/// that token back into the same box, at the price in R5.
pub(crate) fn babel_hex(token: &str) -> String { format!("100604000e20{token}0400040005000500d803d601e30004d602e4c6a70408d603e4c6a7050595e67201d804d604b2a5e4720100d605b2db63087204730000d606db6308a7d60799c1a7c17204d1968302019683050193c27204c2a7938c720501730193e4c672040408720293e4c672040505720393e4c67204060ec5a796830201929c998c7205029591b1720673028cb272067303000273047203720792720773057202") }

/// The script every network fee is paid to, in hex.
pub(crate) fn fee_tree_hex() -> String { if net().fee_tree.is_empty() { ergo_lib::wallet::miner_fee::MINERS_FEE_BASE16_BYTES.to_string() } else { net().fee_tree.to_string() } }

/// The payments this wallet holds that pay it boxes it expects and are not its own, in `DIR/outbox` or set aside in
/// `DIR/given-up` (still judged: one set aside may yet confirm, or be dead): (the file, its JSON, every coin it spends,
/// the boxes it promises this wallet). A payment of this wallet's own (it is waiting) is left to its own bookkeeping.
type Held = (std::path::PathBuf, serde_json::Value, Vec<String>, Vec<String>);
fn held_promises(state: &str, w: &WalletFile) -> Vec<Held> {
    let own: Vec<&String> = w.pending.iter().map(|p| &p.tx).collect();
    let files: Vec<std::path::PathBuf> = ["outbox", "given-up"].iter().flat_map(|d| std::fs::read_dir(format!("{state}/{d}")).map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect::<Vec<_>>()).unwrap_or_default()).collect();
    files.into_iter().filter_map(|f| {
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&f).ok()?).ok()?;
        let tx = ergo_lib::chain::transaction::Transaction::sigma_parse_bytes(&unhex(v["tx_bytes"].as_str()?)?).ok()?;
        if own.contains(&&String::from(tx.id())) { return None }
        // what it promises: boxes expected, or held as void (which come back if its coins do)
        let outs: Vec<String> = tx.outputs.iter().map(|o| String::from(o.box_id())).filter(|o| w.expect.contains(o) || w.void.contains(o)).collect();
        if outs.is_empty() { return None }
        Some((f, v, tx.inputs.iter().map(|i| String::from(i.box_id)).collect(), outs))
    }).collect()
}

/// Box `id` as the payment that makes it would make it, read from that signed payment: one of this wallet's own still
/// waiting, or one it holds for someone else (`held_promises`). `None` if no such payment makes it. Not in a block yet,
/// and only as good as that payment is known to be.
pub(crate) fn expected_box(state: &str, w: &WalletFile, id: &str) -> Option<ErgoBox> {
    let own = w.pending.iter().filter_map(|p| serde_json::from_str::<ergo_lib::chain::transaction::Transaction>(&p.json).ok());
    let held = held_promises(state, w).into_iter().filter_map(|(_, v, _, _)| ergo_lib::chain::transaction::Transaction::sigma_parse_bytes(&unhex(v["tx_bytes"].as_str()?)?).ok());
    own.chain(held).flat_map(|t| t.outputs.into_iter().collect::<Vec<_>>()).find(|o| String::from(o.box_id()) == id)
}

/// The payment that makes box `id`, from a node's pool of payments not yet in a block, and whether any node given
/// answered at all. A node's word that it holds the payment is all this rests on, and only the payment's body is
/// checked here: a box's id is a hash over its payment's id and the box, and the payment's id a hash over the payment
/// without its signatures, so a payment taken here is that payment's body only if, as read, it makes the box `id`. Its
/// signatures are the caller's to verify. Nodes over the peer protocol are not asked.
pub(crate) fn pending_payment(nodes: &[String], id: &str) -> (Option<ergo_lib::chain::transaction::Transaction>, bool) {
    let mut answered = false;
    for n in nodes.iter().filter(|n| !n.starts_with("p2p://")) {
        let r = match http().get(&format!("{n}/transactions/unconfirmed/outputs/byBoxId/{id}")).call() { Ok(r) => r, Err(ureq::Error::Status(404, _)) => { answered = true; continue } Err(_) => continue };
        answered = true;
        let Some(out) = r.into_string().ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { continue };
        let Some(txid) = out["transactionId"].as_str().filter(|t| t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit())) else { continue };
        let Some(tx) = http().get(&format!("{n}/transactions/unconfirmed/byTransactionId/{txid}")).call().ok().and_then(|r| r.into_string().ok())
            .and_then(|t| serde_json::from_str::<ergo_lib::chain::transaction::Transaction>(&t).ok()) else { continue };
        if tx.outputs.iter().any(|o| String::from(o.box_id()) == id) { return (Some(tx), true) }
    }
    (None, answered)
}

/// Hold a payment someone else made to this wallet (its signed bytes, in hex): keep it in `DIR/outbox`, to be submitted
/// by this wallet itself, and expect the boxes it pays this wallet. Nothing is accepted by this; the wallet file is
/// changed in `w` and saved by the caller. Returns the boxes promised, their nanoERG and how many tokens they hold.
/// `seen`: coins the payment spends that the caller has just proven unspent (kept with it as `coins_seen`: one of them
/// gone later means the payment is dead; a coin never seen may only not have been made yet).
fn hold(state: &str, w: &mut WalletFile, my_trees: &[ErgoTree], hexs: &str, seen: &[String], seen_at: u64) -> Result<(Vec<String>, u64, usize), String> {
    let Some(tx) = unhex(hexs).and_then(|b| ergo_lib::chain::transaction::Transaction::sigma_parse_bytes(&b).ok()) else { return Err("not a transaction".into()) };
    let mine: Vec<&ErgoBox> = tx.outputs.iter().filter(|o| my_trees.contains(&o.ergo_tree)).collect();
    if mine.is_empty() { return Err("that transaction pays nothing to this wallet".into()) }
    let json = serde_json::to_string(&tx).unwrap_or_default();
    let id = String::from(tx.id());
    // this file may be the only copy of the payment: nothing is promised unless it is on disk
    let kept = std::fs::create_dir_all(format!("{state}/outbox")).and_then(|_| write_whole(&format!("{state}/outbox/{id}.json"), serde_json::json!({ "tx_id": id, "tx_bytes": hexs, "tx_json": json, "proven": !seen.is_empty(), "coins_seen": seen.iter().map(|c| (c.clone(), serde_json::json!(seen_at))).collect::<serde_json::Map<_, _>>() }).to_string().as_bytes()));
    if let Err(e) = kept { return Err(format!("could not keep the transaction in {state}/outbox ({e}); nothing promised")) }
    for b in &mine { let bid = String::from(b.box_id()); if !w.expect.contains(&bid) { w.expect.push(bid); } }
    let value: u64 = mine.iter().map(|b| *b.value.as_u64()).sum();
    let tokens: usize = mine.iter().map(|b| b.tokens.as_ref().map_or(0, |t| t.len())).sum();
    Ok((mine.iter().map(|b| String::from(b.box_id())).collect(), value, tokens))
}

#[cfg(test)]
pub(crate) fn tracked_count(w: &WalletFile) -> usize { w.boxes.len() }

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
            enc: AddressEncoder::new(if net().testnet { NetworkPrefix::Testnet } else { NetworkPrefix::Mainnet }),
            pass: (if args.iter().any(|a| a == "--pass-stdin") { Some(stdin_line()) } else { std::env::var("TACHYS_PASS").ok() }).filter(|p| !p.is_empty()).map(Secret::new),
            state,
        })
    }

    /// The wallet file: `None` only if there is no file. A file that is there but cannot be read as a wallet is an
    /// error, never "no wallet": `new` and `restore` write where there is none, and the file may hold the only copy of a seed phrase.
    fn read(&self) -> Result<Option<WalletFile>, String> {
        let path = &self.path;
        let text = match std::fs::read_to_string(path) { Ok(t) => t, Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None), Err(e) => return Err(format!("{path} cannot be read ({e}); it was not touched")) };
        crate::walletfile::parse(&text).map(Some).map_err(|why| format!("{path} exists but is not a wallet file this build can read: {why}. It was not touched. It may hold a seed phrase: move it aside by hand before making a new wallet here"))
    }
    fn load(&self) -> Result<Option<WalletFile>, Fail> { self.read().map_err(Fail::from) }

    /// Written beside the file and moved into place, so a crash mid-write cannot leave half a wallet (and half a sealed
    /// phrase), and readable by its owner alone: it holds the seed phrase, sealed or not.
    fn save(&self, w: &WalletFile) -> Ended {
        let path = &self.path;
        private_dir(&self.state);
        let Ok(text) = crate::walletfile::text(w) else { return Err(crate::out::Fail::new(1, format!("could not write {path}"))) };
        crate::util::write_whole(path, text.as_bytes()).map_err(|e| crate::out::Fail::new(1, format!("could not write {path} ({e})")))
    }

    /// A phrase someone typed is checked before it is used (word count, word list, checksum): a slip would otherwise open
    /// an empty wallet at another address and look like lost funds. `--any-phrase` takes it as it is.
    fn checked_phrase(&self, phrase: String) -> Result<String, Fail> {
        if self.args.iter().any(|a| a == "--any-phrase") { return Ok(phrase) }
        if let Some(why) = phrase_problem(&phrase) { return Err(crate::out::Fail::new(2, format!("seed phrase not accepted: {why}"))) }
        Ok(phrase.split_whitespace().collect::<Vec<_>>().join(if phrase.contains('\u{3000}') { "\u{3000}" } else { " " }))
    }

    /// Puts the phrase in the wallet file: sealed if there is a password, in plain text if not.
    fn keep(&self, w: &mut WalletFile, phrase: &str) -> Ended {
        (w.mnemonic, w.mnemonic_file, w.secret, w.warning) = (None, None, None, None);
        match self.pass.as_deref() {
            Some(pw) => {
                let sealed = seal_phrase(phrase, pw).filter(|s| open_phrase(s, pw).as_deref() == Ok(phrase));
                let Some(sealed) = sealed else { return Err(crate::out::Fail::new(1, "could not seal the seed phrase; nothing written".to_string())) };
                w.secret = Some(sealed);
            }
            None => { w.mnemonic = Some(phrase.to_string()); w.warning = Some("seed phrase stored in plain text: no password was given".into()); }
        }
        Ok(())
    }
}

/// The wallet's keys: m/44'/429'/0'/0/i, the same path every Ergo wallet uses, so a phrase restores anywhere.
fn key_at(phrase: &str, i: u64) -> Option<ExtSecretKey> {
    let master = ExtSecretKey::derive_master(Mnemonic::to_seed(phrase, "")).ok()?;
    master.derive(DerivationPath::from_str(&format!("m/44'/429'/0'/0/{i}")).ok()?).ok()
}
/// The first address of a phrase.
fn derive(phrase: &str) -> Option<Address> { Some(Address::P2Pk(key_at(phrase, 0)?.public_image())) }
/// The signing keys of the first `n_addr` addresses of a phrase.
fn secrets(phrase: &str, n_addr: u64) -> Vec<SecretKey> { (0..n_addr).filter_map(|i| key_at(phrase, i).map(|k| k.secret_key())).collect() }
fn read_phrase(file: &str) -> Result<String, Fail> {
    if file == "-" { return Ok(stdin_line()) }
    Ok(std::fs::read_to_string(file).map_err(|e| crate::out::Fail::new(2, format!("--mnemonic-file {file}: {e}")))?.trim().to_string())
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
    let reads_only = matches!(c.sub, "derive" | "mnemonic" | "pubkey" | "babel-list") || (c.sub == "address" && !args.iter().any(|a| a == "--add"));
    if makes { private_dir(&c.state); }
    let _turn = if !reads_only && std::path::Path::new(&c.state).is_dir() { Some(crate::util::turn(&c.state).map_err(Fail::from)?) } else { None };
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
    let Some(addr) = derive(&phrase) else { return Err(crate::out::Fail::new(1, "key derivation failed".to_string())) };
    crate::out::set("address", enc.address_to_str(&addr));
    say!("address {}", enc.address_to_str(&addr));
    Ok(())
}

/// wallet new: a new phrase, its first address, and a wallet file.
fn cmd_new(c: &Ctx) -> Ended {
    let Ctx { args, path, chain, enc, pass, .. } = c;
    if c.load()?.is_some() { return Err(crate::out::Fail::new(2, format!("{path} already exists; not overwriting"))) }
    let phrase: Secret = match MnemonicGenerator::new(Language::English, 128) { Ok(g) => Secret::new(g.generate()), Err(e) => { return Err(crate::out::Fail::new(1, format!("mnemonic: {e:?}"))) } };
    let Some(addr) = derive(&phrase) else { return Err(crate::out::Fail::new(1, "key derivation failed".to_string())) };
    let birthday = chain.first().map(|h| h.height).unwrap_or(0);
    let a = enc.address_to_str(&addr);
    let mut w = WalletFile { address_list: Some(vec![a.clone()]), birthday: birthday as u64, ..WalletFile::new(arg(args, "--name", "Wallet".to_string())?, a.clone()) };
    c.keep(&mut w, &phrase)?;
    c.save(&w)?;
    crate::out::set("address", &a); crate::out::set("birthday", birthday); crate::out::set("sealed", pass.is_some());
    say!("new wallet: address {a}\nbirthday height {birthday} (nothing before it is ever looked for)\nsaved to {path}{}", if pass.is_some() { "\nseed phrase sealed with the password given" } else { "\nseed phrase in plain text (no password given)" });
    Ok(())
}

/// wallet restore: a wallet file from a phrase. The boxes are looked for afterwards (`cmd_sync`).
fn cmd_restore(c: &Ctx) -> Ended {
    let Ctx { args, path, enc, .. } = c;
    // Restore = seed phrase -> addresses -> current boxes. No history is fetched or kept.
    let file: String = arg(args, "--mnemonic-file", String::new())?;
    let phrase = Secret::new(c.checked_phrase(read_phrase(&file)?)?);
    let Some(addr) = derive(&phrase) else { return Err(crate::out::Fail::new(1, "key derivation failed".to_string())) };
    let a = enc.address_to_str(&addr);
    // the phrase is referenced, not copied, unless told to keep it (--embed: a phone has nowhere else to hold it)
    let embed = args.iter().any(|x| x == "--embed");
    if file == "-" && !embed { return Err(crate::out::Fail::new(2, "a phrase given on stdin has to be kept in the wallet file: add --embed".to_string())) }
    // A restore does not know how many addresses were in use. All twenty are written down and the wallet is marked;
    // the first sync that asks a node what each holds keeps those up to the last one with coins (see `scan_addresses` below).
    let look_at_all = |w: &mut WalletFile| {
        let all: Vec<String> = (0..20).filter_map(|i| key_at(&phrase, i).map(|k| enc.address_to_str(&Address::P2Pk(k.public_image())))).collect();
        if all.first() == Some(&a) { w.address_list = Some(all); w.scan_addresses = true; }
    };
    // --reseal: the wallet is already here but its phrase can no longer be opened (its password is gone); the same phrase puts it back
    let reseal = args.iter().any(|x| x == "--reseal");
    match c.load()? {
        // restoring over a read-only wallet of the same address gives it its key and keeps what it already tracks
        Some(mut w) if w.address == a && (w.watch || reseal) => {
            let was_watch = std::mem::take(&mut w.watch);
            if embed { c.keep(&mut w, &phrase)?; } else {
                (w.mnemonic, w.secret, w.warning) = (None, None, None);
                w.mnemonic_file = Some(file.clone());
            }
            if w.address_list.is_none() { w.address_list = Some(vec![a.clone()]); }
            if was_watch { look_at_all(&mut w); }
            w.restored = true;
            c.save(&w)?;
            crate::out::set("address", &a); crate::out::set("restored", if was_watch { "upgraded" } else { "resealed" });
            say!("{}: address {a}", if was_watch { "upgraded read-only wallet" } else { "resealed wallet" });
        }
        Some(_) => { return Err(crate::out::Fail::new(2, format!("{path} already exists and is not a read-only wallet of {a}; not overwriting"))) }
        None => {
            let mut w = WalletFile { address_list: Some(vec![a.clone()]), restored: true, ..WalletFile::new(arg(args, "--name", "Wallet".to_string())?, a.clone()) };
            if embed { c.keep(&mut w, &phrase)?; } else { w.mnemonic_file = Some(file.clone()); }
            look_at_all(&mut w);
            c.save(&w)?;
            crate::out::set("address", &a); crate::out::set("restored", "new");
            say!("restored wallet: address {a}");
        }
    }
    Ok(())
}

/// wallet watch: a read-only wallet.
fn cmd_watch(c: &Ctx) -> Ended {
    let Ctx { args, path, enc, .. } = c;
    // a read-only wallet: an address and nothing else; no key ever touches this device
    if c.load()?.is_some() { return Err(crate::out::Fail::new(2, format!("{path} already exists; not overwriting"))) }
    let a: String = arg(args, "--address", String::new())?;
    if enc.parse_address_from_str(&a).is_err() { return Err(crate::out::Fail::new(2, format!("--address is not a {} address", net().name))) }
    let name: String = arg(args, "--name", "Wallet".to_string())?;
    c.save(&WalletFile { watch: true, ..WalletFile::new(name, a.clone()) })?;
    crate::out::set("address", &a); crate::out::set("watch", true);
    say!("watching {a}");
    Ok(())
}

/// Reads the wallet file and opens its phrase if it can: every subcommand below starts from this.
fn open(c: &Ctx) -> Result<Open, Fail> {
    let Ctx { state, path, enc, pass, .. } = c;
    let Some(w) = c.load()? else { return Err(crate::out::Fail::new(2, format!("no wallet at {path}; run `tachys wallet new --state {state}`"))) };
    // A sealed phrase is opened only when the password comes with the command. Without it the wallet still knows its
    // addresses (they are written beside the sealed phrase), so it can watch, prove and receive; it cannot sign or show the words.
    // A wrong password leaves it as closed as no password does (`bad_password` says so): a command that needs the phrase
    // is refused, and one that does not goes on. A sealed phrase that cannot be read at all is an error for every command.
    let sealed = w.secret.as_ref().is_some_and(|s| s.is_object());
    let mut bad_password = false;
    let phrase: Option<Secret> = if sealed {
        match pass.as_deref().map(|pw| open_phrase(w.secret.as_ref().unwrap_or(&serde_json::Value::Null), pw)) {
            None => None,
            Some(Ok(p)) => Some(Secret::new(p)),
            Some(Err(e)) if e == "bad wallet password" => { crate::out::set("bad_password", true); bad_password = true; None }
            Some(Err(e)) => return Err(crate::out::Fail::new(1, format!("wallet: the seed phrase could not be opened: {e}"))),
        }
    } else {
        w.mnemonic.clone()
                .or_else(|| w.mnemonic_file.as_deref().and_then(|f| std::fs::read_to_string(f).ok()).map(|p| p.trim().to_string())).map(Secret::new)
    };
    let locked = sealed && phrase.is_none();
    let no_phrase = if bad_password { "wallet: the seed phrase could not be opened: bad wallet password" } else if locked { "the seed phrase is sealed: give the wallet's password (--pass-stdin, or TACHYS_PASS)" } else { "this wallet holds no phrase" };
    let Some(addr) = phrase.as_deref().and_then(|p| derive(p)).or_else(|| enc.parse_address_from_str(&w.address).ok()) else { return Err(crate::out::Fail::new(1, "wallet file has no usable seed phrase or address".to_string())) };
    if sealed && w.address != enc.address_to_str(&addr) { return Err(crate::out::Fail::new(1, "wallet: the sealed phrase does not lead to this wallet's address; refusing to use it".to_string())) }
    let Ok(my_tree) = addr.script() else { return Err(crate::out::Fail::new(1, "address has no script".to_string())) };
    // Further addresses of the same phrase (indices 1..). Everything at any of them is this wallet's; change goes to the first.
    let listed: Vec<Address> = w.address_list.iter().flatten().filter_map(|a| enc.parse_address_from_str(a).ok()).take(20).collect();
    // A restored wallet not yet looked over has all twenty listed, with or without the phrase to hand.
    // A restored wallet that found all twenty empty kept the first only, so a payment made later to another of them went
    // unseen. Asked to look again (`status --reconcile`, or a sync after a sign that another wallet of the phrase is at
    // work), it looks over all twenty again and keeps any up to the last that holds a box; never fewer than before.
    let rescan = c.sub == "status" && w.restored && !w.scan_addresses && phrase.is_some() && (c.args.iter().any(|a| a == "--reconcile") || w.refresh);
    let scanning = w.scan_addresses || rescan;
    let n_addr = if scanning && phrase.is_some() { 20 } else if phrase.is_some() { w.addresses.unwrap_or(1).clamp(1, 20) } else if locked { (listed.len() as u64).max(1) } else { 1 };
    let my_addrs: Vec<Address> = match phrase.as_deref() {
        Some(p) => (0..n_addr).filter_map(|i| key_at(p, i).map(|k| Address::P2Pk(k.public_image()))).collect(),
        None if locked && listed.first() == Some(&addr) => listed,
        None => vec![addr.clone()],
    };
    let my_trees: Vec<_> = my_addrs.iter().filter_map(|a| a.script().ok()).collect();
    let addr_strs: Vec<String> = my_addrs.iter().map(|a| enc.address_to_str(a)).collect();
    Ok(Open { w, sealed, phrase, locked, bad_password, no_phrase, addr, my_tree, scanning, n_addr, my_addrs, my_trees, addr_strs })
}

/// wallet encrypt: seal a phrase that is in plain text.
fn cmd_encrypt(c: &Ctx, o: Open) -> Ended {
    let Open { mut w, sealed, addr_strs, .. } = o;
    let pass = &c.pass;
    // a phrase kept in plain text becomes a sealed one; `keep` opens what it sealed before anything is written
    if sealed { crate::out::set("sealed", true); say!("already sealed: {}", addr_strs[0]); return Ok(()) }
    if pass.is_none() { return Err(crate::out::Fail::new(2, "encrypt needs the password to seal with (--pass-stdin, or TACHYS_PASS)".to_string())) }
    let Some(p) = w.mnemonic.clone().map(Secret::new) else { return Err(crate::out::Fail::new(2, "this wallet file holds no phrase of its own to seal".to_string())) };
    c.keep(&mut w, &p)?;
    w.address_list = Some(addr_strs.clone());
    c.save(&w)?;
    crate::out::set("sealed", true);
    say!("sealed: the seed phrase of {} is no longer in plain text", addr_strs[0]);
    Ok(())
}

/// wallet name: give the wallet a name.
fn cmd_name(c: &Ctx, o: Open) -> Ended {
    let (args, mut w) = (c.args, o.w);
    w.name = arg(args, "--name", "Wallet".to_string())?;
    c.save(&w)?; crate::out::set("name", &w.name); say!("named {}", w.name);
    Ok(())
}

/// wallet address: list the wallet's addresses, or add the next one (`--add`).
fn cmd_address(c: &Ctx, o: Open) -> Ended {
    let Ctx { args, enc, .. } = c;
    let Open { mut w, phrase, locked, bad_password, no_phrase, n_addr, addr_strs, .. } = o;
    if args.iter().any(|a| a == "--add") {
        let Some(p) = phrase.as_deref() else { return Err(crate::out::Fail::new(if bad_password { 1 } else { 2 }, (if locked { no_phrase } else { "a read-only wallet has one address" }).to_string())) };
        if n_addr >= 20 { return Err(crate::out::Fail::new(2, "twenty addresses is this build's limit".to_string())) }
        let Some(k) = key_at(p, n_addr) else { return Err(crate::out::Fail::new(1, "key derivation failed".to_string())) };
        let mut list = addr_strs.clone(); list.push(enc.address_to_str(&Address::P2Pk(k.public_image())));
        w.addresses = Some(n_addr + 1); w.address_list = Some(list); c.save(&w)?;
        crate::out::set("added", enc.address_to_str(&Address::P2Pk(k.public_image()))); crate::out::set("addresses", &w.address_list);
        say!("address {}: {}", n_addr, enc.address_to_str(&Address::P2Pk(k.public_image())));
    } else { crate::out::set("addresses", &addr_strs); for (i, a) in addr_strs.iter().enumerate() { say!("address {i}: {a}"); } }
    Ok(())
}

/// wallet mnemonic: show the seed phrase.
fn cmd_mnemonic(c: &Ctx, o: Open) -> Ended {
    let state = &c.state;
    let Open { phrase, no_phrase, bad_password, .. } = o;
    // for the owner to write down; the caller decides who may ask
    if policy::present(state) { return Err(crate::out::Fail::new(2, format!("this wallet is under a spending policy ({}); it does not show its seed phrase", policy::path(state)))) }
    match phrase.as_deref() { Some(p) => { crate::out::set("mnemonic", p); if !crate::out::json() { say!("{p}") } } None => { return Err(crate::out::Fail::new(if bad_password { 1 } else { 2 }, no_phrase.to_string())) } }
    Ok(())
}

/// wallet pubkey: the extended public key, for a watching wallet.
fn cmd_pubkey(o: Open) -> Ended {
    let Open { phrase, no_phrase, bad_password, addr, .. } = o;
    // The extended public key at m/44'/429'/0', laid out as ergo-appkit's Bip32Serialization does: it lets
    // another wallet watch every address of this one without being able to spend.
    use sha2::Digest;
    let Some(p) = phrase.as_deref() else { return Err(crate::out::Fail::new(if bad_password { 1 } else { 2 }, no_phrase.to_string())) };
    let at = |path: &str| ExtSecretKey::derive_master(Mnemonic::to_seed(p, "")).ok().and_then(|m| m.derive(DerivationPath::from_str(path).ok()?).ok()).and_then(|k| k.public_key().ok());
    let (Some(parent), Some(grand)) = (at("m/44'/429'/0'"), at("m/44'/429'")) else { return Err(crate::out::Fail::new(1, "key derivation failed".to_string())) };
    let finger = ripemd::Ripemd160::digest(sha2::Sha256::digest(grand.pub_key_bytes()));
    let mut out: Vec<u8> = if net().testnet { vec![0x04, 0x35, 0x87, 0xCF] } else { vec![0x04, 0x88, 0xB2, 0x1E] };
    out.push(3); out.extend(&finger[..4]); out.extend([0u8; 4]); out.extend(parent.chain_code()); out.extend(parent.pub_key_bytes());
    // check what is about to be shown: read the key back out of those bytes and derive the first address from it
    let back = <[u8; 33]>::try_from(&out[45..78]).ok().zip(<[u8; 32]>::try_from(&out[13..45]).ok())
        .and_then(|(k, c)| ergo_lib::wallet::ext_pub_key::ExtPubKey::new(k, c, DerivationPath::from_str("m/44'/429'/0'").ok()?).ok())
        .and_then(|x| x.derive(DerivationPath::from_str("m/44'/429'/0'/0/0").ok()?).ok()).map(Address::from);
    if back.as_ref() != Some(&addr) { return Err(crate::out::Fail::new(1, "public key does not lead back to this wallet's address; not shown".to_string())) }
    crate::out::set("hex", out.iter().map(|b| format!("{b:02x}")).collect::<String>()); crate::out::set("base58", bs58::encode(&out).with_check().into_string());
    say!("hex {}", out.iter().map(|b| format!("{b:02x}")).collect::<String>());
    say!("base58 {}", bs58::encode(&out).with_check().into_string());
    Ok(())
}

/// wallet promise: keep a payer's signed transaction to submit later.
fn cmd_promise(c: &Ctx, o: Open) -> Ended {
    let Ctx { args, state, .. } = c;
    let Open { mut w, my_trees, .. } = o;
    // The payee's side of an offline payment: keep the payer's signed transaction, so this wallet can
    // submit it itself once it has network, and expect the boxes that pay it. Nothing is accepted yet:
    // the payer can still spend the same coins elsewhere until this is in a block.
    let hexs: String = arg(args, "--tx", String::new())?;
    match hold(state, &mut w, &my_trees, &hexs, &[], 0) {
        Ok((ids, value, tokens)) => {
            c.save(&w)?;
            crate::out::set("promised", &ids); crate::out::set("nanoerg", value); crate::out::set("tokens", tokens);
            say!("promised {} {} nanoERG {} tokens", ids[0], value, tokens);
            Ok(())
        }
        Err(why) => { say!("{why}"); Err(crate::out::Fail::said(1)) }
    }
}

/// wallet request: say what a signing request does, and sign it if told to. A refusal is handed back, not printed:
/// nothing here stops the process.
fn cmd_request(c: &Ctx, o: Open) -> Ended {
    let Ctx { args, state, node, chain, enc, .. } = c;
    let Open { mut w, phrase, no_phrase, n_addr, my_addrs, my_trees, .. } = o;
    // A signing request (ErgoPay, EIP-20, static form): someone else built the transaction; this wallet
    // says what it does, in its own words from the transaction itself, and signs only when told to.
    use base64::Engine;
    use ergo_lib::chain::transaction::reduced::ReducedTransaction;
    use ergo_lib::wallet::Wallet;
    let code: String = arg(args, "--code", String::new())?;
    let Some(b64) = code.trim().strip_prefix("ergopay:") else { return Err("not an ergopay: request".into()) };
    if b64.starts_with("//") { return Err("this is a request to fetch from a server; only requests that carry the transaction are supported".into()) }
    let Some(reduced) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(b64.trim_end_matches('=')).ok().and_then(|b| ReducedTransaction::sigma_parse_bytes(&b).ok()) else { return Err("the request does not hold a transaction".into()) };
    let tx = reduced.unsigned_tx.clone();
    // the wallet's own record of a box is used only if its bytes hash to the id they are filed under
    let tracked: std::collections::BTreeMap<String, ErgoBox> = w.boxes.iter().filter_map(|(k, b)| unhex(&b.bytes).and_then(|x| ErgoBox::sigma_parse_bytes(&x).ok()).filter(|x| String::from(x.box_id()) == *k).map(|x| (k.clone(), x))).collect();
    let in_ids: Vec<String> = tx.inputs.iter().map(|i| String::from(i.box_id)).collect();
    // What each input needs in order to be spent, as the request states it. An input whose condition names one of
    // this wallet's keys is this wallet's money wherever it sits: an offer box it made, say, not only a box at its address.
    use ergotree_ir::sigma_protocol::sigma_boolean::{ProveDlog, SigmaBoolean};
    let my_keys: Vec<ProveDlog> = my_addrs.iter().filter_map(|a| if let Address::P2Pk(k) = a { Some(k.clone()) } else { None }).collect();
    let conditions: Vec<SigmaBoolean> = reduced.reduced_inputs().iter().map(|r| r.sigma_prop.clone()).collect();
    let needs_my_key = |id: &String| in_ids.iter().position(|x| x == id).and_then(|i| conditions.get(i)).is_some_and(|c| names(c, &my_keys));
    let mine: Vec<&ErgoBox> = in_ids.iter().filter_map(|i| tracked.get(i)).collect();
    let others: Vec<String> = in_ids.iter().filter(|i| !tracked.contains_key(*i)).cloned().collect();
    let mut mine: Vec<ErgoBox> = mine.into_iter().cloned().collect();
    // inputs that are not ours (a pool, a bank, another party) must be real: proven unspent now
    let mut other_boxes: Vec<ErgoBox> = vec![];
    if !others.is_empty() {
        if node.is_empty() || chain.is_empty() { return Err("it spends boxes that are not ours; a node and an agreed tip are needed to check them".into()) }
        let mut cost = RestCost::default();
        match prove_any(node, chain, &others, &mut cost) {
            Err(e) => return Err(e.into()),
            Ok((_, found)) => for (id, f) in others.iter().zip(found.iter()) {
                // whose money an input is depends on who can spend it, not on whether this wallet had seen it:
                // a box at our own address that we were not tracking yet is still ours
                match f.as_ref().and_then(|b| ErgoBox::sigma_parse_bytes(b).ok()) {
                    Some(b) if my_trees.contains(&b.ergo_tree) || needs_my_key(id) => mine.push(b),
                    Some(b) => other_boxes.push(b),
                    None => return Err(format!("input {}… is not an unspent box", clip(id, 16)).into()),
                }
            },
        }
    }
    if mine.is_empty() { return Err("it spends nothing from this wallet".into()) }
    // totals, from the transaction itself
    type Sums = (u64, std::collections::BTreeMap<String, u64>);
    fn add(s: &mut Sums, value: u64, tokens: Option<&ergotree_ir::chain::ergo_box::BoxTokens>) {
        // (whoever wrote the request chose these amounts: sums that would wrap are held at the top, as `leaves_wallet` holds them)
        s.0 = s.0.saturating_add(value);
        for t in tokens.into_iter().flat_map(|ts| ts.iter()) { let n = s.1.entry(String::from(t.token_id)).or_default(); *n = n.saturating_add(*t.amount.as_u64()); }
    }
    let fee_tree = fee_tree_hex();
    let (mut give, mut get, mut to_others, mut from_others): (Sums, Sums, Sums, Sums) = Default::default();
    let mut paid_to: Vec<String> = vec![];
    let mut fee = 0u64;
    for b in &mine { add(&mut give, *b.value.as_u64(), b.tokens.as_ref()); }
    for b in &other_boxes { add(&mut from_others, *b.value.as_u64(), b.tokens.as_ref()); }
    for o in tx.output_candidates.iter() {
        let tree_hex = o.ergo_tree.sigma_serialize_bytes().map(|b| hex(&b)).unwrap_or_default();
        if my_trees.contains(&o.ergo_tree) { add(&mut get, *o.value.as_u64(), o.tokens.as_ref()); }
        else if tree_hex == fee_tree { fee += *o.value.as_u64(); }
        else {
            add(&mut to_others, *o.value.as_u64(), o.tokens.as_ref());
            let a = Address::recreate_from_ergo_tree(&o.ergo_tree).map(|a| enc.address_to_str(&a)).unwrap_or_else(|_| format!("script {}", clip(&tree_hex, 40)));
            if !paid_to.contains(&a) { paid_to.push(a); }
        }
    }
    // a token that leaves the wallet and comes straight back is not part of what the request does to you
    let through: std::collections::BTreeMap<String, u64> = give.1.iter().filter_map(|(id, a)| get.1.get(id).map(|b| (id.clone(), *a.min(b)))).collect();
    for (id, t) in &through { for side in [&mut give.1, &mut get.1] { if let Some(v) = side.get_mut(id) { *v -= t; } } }
    give.1.retain(|_, v| *v > 0); get.1.retain(|_, v| *v > 0);
    let show = |s: &Sums| -> String { format!("{} nanoERG{}", s.0, s.1.iter().map(|(id, a)| format!(", {a} of token {id}")).collect::<String>()) };
    let sums = |s: &Sums| serde_json::json!({ "nanoerg": s.0, "tokens": s.1 });
    crate::out::set("give", sums(&give)); crate::out::set("get", sums(&get)); crate::out::set("others_put_in", sums(&from_others)); crate::out::set("others_receive", sums(&to_others));
    crate::out::set("fee_nanoerg", fee); crate::out::set("paid_to", &paid_to); crate::out::set("inputs", serde_json::json!({ "all": in_ids.len(), "yours": mine.len(), "others": other_boxes.len() }));
    crate::out::set("net_nanoerg", get.0 as i128 - give.0 as i128); crate::out::set("signed", false);
    say!("request: {} inputs ({} yours, {} proven others), {} outputs", in_ids.len(), mine.len(), other_boxes.len(), tx.output_candidates.len());
    say!("you give: {}", show(&give));
    say!("you get back: {}", show(&get));
    say!("others put in: {}", show(&from_others));
    say!("others receive: {}", show(&to_others));
    say!("network fee: {fee} nanoERG");
    let net_erg = get.0 as i128 - give.0 as i128;
    let mut ids: std::collections::BTreeSet<&String> = give.1.keys().collect(); ids.extend(get.1.keys());
    let tokens_net: String = ids.iter().map(|id| (id, *get.1.get(*id).unwrap_or(&0) as i128 - *give.1.get(*id).unwrap_or(&0) as i128)).filter(|x| x.1 != 0).map(|(id, d)| format!(", {d:+} of token {id}")).collect();
    say!("net for you: {net_erg:+} nanoERG{tokens_net}");
    if !args.iter().any(|a| a == "--sign") { say!("not signed (add --sign to sign and send)"); return Ok(()) }
    if node.is_empty() { return Err("--sign needs --node to send the result".into()) }
    let Some(p) = phrase.as_deref() else { return Err(no_phrase.into()) };
    let secrets = secrets(p, n_addr);
    if secrets.is_empty() { return Err("derivation".into()) }
    // a wallet under a spending policy holds a request to it like any payment: what it gives, less what it gets back
    let ask = policy::Ask { nano: give.0.saturating_sub(get.0), tokens: give.1.clone(), payees: paid_to.clone(), purpose: arg(args, "--purpose", String::new())?, what: "request".into() };
    if let Err(e) = policy::reserve(state, &ask, false) { return Err(format!("the spending policy does not allow it: {e}").into()) }
    let signed = match Wallet::from_secrets(secrets).sign_reduced_transaction(reduced, None) { Ok(t) => t, Err(e) => return Err(format!("sign: {e:?}").into()) };
    let json = serde_json::to_string(&signed).unwrap_or_default();
    crate::out::set("signed", true); crate::out::set("tx", String::from(signed.id()));
    // The payment is written down as waiting before any node is sent it: if this stops between the node taking it and
    // the file being written, the wallet must not be left thinking those coins are free to spend again.
    let before = w.clone();
    let mut expect: Vec<String> = w.expect.clone();
    expect.extend(signed.outputs.iter().filter(|o| my_trees.contains(&o.ergo_tree)).map(|o| String::from(o.box_id())));
    for b in &mine { w.boxes.remove(&String::from(b.box_id())); }
    w.expect = expect;
    note_pending(&mut w, &signed, &json);
    c.save(&w)?;
    let unsure = match send_tx(node, &json) {
        Ok((n, said)) => { crate::out::set("sent_via", &n); say!("signed and sent via {n}: accepted, {said}"); None }
        // a refusal, or no node reached at all: nothing went out, and the wallet is put back as it was
        Err(NotSent::Refused(c2, body)) => { c.save(&before)?; return Err(format!("the node refused it ({c2}): {}", one_line(body.lines().find(|l| l.contains("detail")).unwrap_or(""))).into()) }
        Err(NotSent::Unreachable(e)) => { c.save(&before)?; return Err(format!("could not send: {e}").into()) }
        Err(NotSent::Unknown(e)) => Some(format!("the node did not answer ({e}). The payment may have gone out: it is kept as waiting, its coins are not counted, and the next sync settles it (sent again if it did not arrive; back in the wallet if it is not in a block after ten). Do not pay again until then")),
    };
    if unsure.is_some() { crate::out::set("outcome_unknown", true); }
    policy::note(state, &String::from(signed.id()), "");
    match unsure { Some(e) => Err(e.into()), None => Ok(()) }
}

/// What a payment is to do, read from the command line, and what the wallet has to do it with: all that the builders
/// below work from. Reading it asks no node anything.
struct Wanted<'a> {
    c: &'a Ctx<'a>,
    /// the wallet's boxes confirmed under a tip the peers agreed on: the only ones a payment spends
    inputs: Vec<ErgoBox>,
    addr: Address,
    my_tree: ErgoTree,
    /// to the first payee (or, for an offer, into it); `more` are the further payees of `--pay`
    amount: u64,
    fee: u64,
    /// the payees and the fee together
    need_total: u64,
    v_out: BoxValue,
    v_fee: BoxValue,
    v_need: BoxValue,
    height: u32,
    token_hex: String,
    token_amount: u64,
    mint_name: String,
    mint_decimals: usize,
    payee_tree: ErgoTree,
    more: Vec<(ErgoTree, u64)>,
    pay_token: Option<Token>,
    /// every token the first payee is paid: `--token` and each `--also-token`
    want_tokens: Vec<Token>,
    /// the token the fee is paid in (`--babel`), and the most of it the fee may cost
    babel_token: String,
    babel_max: u64,
}

/// What a builder made of it: the boxes spent and the boxes made, and what the rest of `cmd_send` must know of them.
struct Built {
    selection: BoxSelection<ErgoBox>,
    outs: Vec<ErgoBoxCandidate>,
    /// the scripts paid as payees: boxes under them are the receipt
    payee_trees: Vec<ErgoTree>,
    /// scripts paid that are neither a payee's, ours nor the fee's (a Babel offer)
    other_trees: Vec<ErgoTree>,
    burn: Vec<Token>,
    /// what a Babel offer spent is told: where its recreated box sits among the outputs
    extension: Option<(BoxId, ContextExtension)>,
}

/// Reads what the command line asks for and gathers the boxes that may be spent. What cannot be a payment is refused
/// here, before anything is built.
fn wanted<'a>(c: &'a Ctx<'a>, w: &WalletFile, addr: &Address, my_tree: &ErgoTree) -> Result<Wanted<'a>, Fail> {
    let Ctx { args, chain, enc, .. } = c;
    let sub = c.sub;
    let token_hex: String = arg(args, "--token", String::new())?;
    let token_amount: u64 = arg(args, "--token-amount", 0)?;
    let mint_name: String = arg(args, "--mint", String::new())?;
    let mint_decimals: usize = arg(args, "--decimals", 0)?;
    // minting pays the new token to ourselves; a token payment carries the minimum ERG a box needs
    let to_self = !mint_name.is_empty() || sub != "send";
    let to: String = if to_self { enc.address_to_str(addr) } else { arg(args, "--to", String::new())? };
    // `--pay` and `--also-token` are read below as the word after each; one given last has none, and is misuse, as an
    // option without its value is everywhere (`util::try_arg`), not a payee or token quietly left out
    if let Some(last) = args.last().filter(|a| *a == "--pay" || *a == "--also-token") { return Err(crate::out::Fail::new(2, format!("{last} needs a value"))) }
    // further payees in the same payment, one fee for all: --pay ADDRESS:NANOERG (repeatable)
    let more: Vec<(ergotree_ir::ergo_tree::ErgoTree, u64)> = args.iter().enumerate().filter(|(_, a)| *a == "--pay").filter_map(|(i, _)| args.get(i + 1)).map(|spec| {
        let (a, n) = spec.split_once(':').unwrap_or((spec.as_str(), ""));
        match (enc.parse_address_from_str(a).ok().and_then(|x| x.script().ok()), n.parse::<u64>()) { (Some(t), Ok(v)) if v >= 1_000_000 => Ok((t, v)), _ => Err(format!("--pay wants ADDRESS:NANOERG with at least 1000000 nanoERG, got {spec}")) }
    }).collect::<Result<_, String>>()?;
    let amount: u64 = { let a: u64 = arg(args, "--nanoerg", 0)?; if a == 0 && (!token_hex.is_empty() || !mint_name.is_empty() || sub != "send") { 1_000_000 } else { a } };
    let fee: u64 = arg(args, "--fee", 1_100_000)?;
    if chain.len() < 10 { return Err("needs the ten agreed headers; run `tachys tips` first".into()) }
    let Ok(payee) = enc.parse_address_from_str(&to) else { return Err(format!("--to is not a {} address", net().name).into()) };
    let Ok(payee_tree) = payee.script() else { return Err("payee address has no script".into()) };
    // Only a box confirmed under a tip the peers agreed on is spent. One proven only in a block a single node has
    // shown is tracked and shown as seen; a payment built on it would be void if that block is not the chain's.
    // A box's stored bytes are used only if they hash to the id they are filed under (as `cmd_request` holds them): the
    // bytes of another box would be signed for in its place, and the balance and the payment would disagree.
    let mut inputs: Vec<ErgoBox> = vec![];
    for (id, b) in w.boxes.iter().filter(|(_, b)| b.confirmed_at.is_some()) {
        let Some(x) = unhex(&b.bytes).and_then(|x| ErgoBox::sigma_parse_bytes(&x).ok()) else { return Err("some tracked boxes have no stored bytes; run `tachys wallet status` first".into()) };
        if String::from(x.box_id()) != *id { return Err(format!("the bytes stored for box {id} are another box's ({}); nothing was signed. Run `tachys wallet status`, which reads each box again from its proof", String::from(x.box_id())).into()) }
        inputs.push(x);
    }
    if inputs.is_empty() && !w.boxes.is_empty() { return Err("none of this wallet's boxes is confirmed under a tip the peers agree on yet; run `tachys wallet status` again after a block".into()) }
    let more_total: u64 = more.iter().try_fold(0u64, |a, m| a.checked_add(m.1)).ok_or("the amounts add up to more than can exist")?;
    let need_total = amount.checked_add(more_total).and_then(|x| x.checked_add(fee)).ok_or("the amounts add up to more than can exist")?;
    let (Ok(v_out), Ok(v_fee), Ok(v_need)) = (BoxValue::try_from(amount), BoxValue::try_from(fee), BoxValue::try_from(need_total)) else { return Err("amount or fee out of range".into()) };
    let height = chain[0].height;
    let pay_token: Option<Token> = if token_hex.is_empty() || sub.starts_with("babel-") { None } else {
        let (Some(id), Ok(amt)) = (ergo_chain_types::Digest32::try_from(token_hex.clone()).ok().map(TokenId::from), TokenAmount::try_from(token_amount)) else { return Err("--token needs a 64-hex id and --token-amount above zero".into()) };
        Some(Token { token_id: id, amount: amt })
    };
    // further tokens to the same payee in the same payment: --also-token ID:RAW (repeatable)
    let also: Vec<Token> = if sub != "send" { vec![] } else { args.iter().enumerate().filter(|(_, a)| *a == "--also-token").filter_map(|(i, _)| args.get(i + 1)).map(|spec| {
        let (id, n) = spec.split_once(':').unwrap_or((spec.as_str(), ""));
        match (ergo_chain_types::Digest32::try_from(id.to_string()).ok().map(TokenId::from), n.parse::<u64>().ok().and_then(|v| TokenAmount::try_from(v).ok())) { (Some(id), Some(amount)) => Ok(Token { token_id: id, amount }), _ => Err(format!("--also-token wants ID:RAW with an amount above zero, got {spec}")) }
    }).collect::<Result<_, String>>()? };
    let want_tokens: Vec<Token> = pay_token.iter().cloned().chain(also).collect();
    { let mut seen = std::collections::BTreeSet::new(); if !want_tokens.iter().all(|t| seen.insert(String::from(t.token_id))) { return Err("the same token is named twice".into()) } }
    let babel_token: String = arg(args, "--babel", String::new())?;
    // the most token units the fee may cost: what the person was shown, so that is the most that is signed
    let babel_max: u64 = arg(args, "--babel-max", 0)?;
    Ok(Wanted { c, inputs, addr: addr.clone(), my_tree: my_tree.clone(), amount, fee, need_total, v_out, v_fee, v_need, height, token_hex, token_amount, mint_name, mint_decimals,
        payee_tree, more, pay_token, want_tokens, babel_token, babel_max })
}

    // Fees in a token (EIP-31, "Babel fees"). Someone locks ERG in a box under a public contract with a price;
    // anyone may take ERG from it by putting at least (ERG taken / price) of the token back into the same box.
fn babel_tree(token: &str) -> Option<ErgoTree> { unhex(&babel_hex(token)).and_then(|b| ErgoTree::sigma_parse_bytes(&b).ok()) }
/// The price an offer asks: nanoERG for one unit of the token.
fn babel_price(b: &ErgoBox) -> i64 { b.additional_registers.get_constant(R5).ok().flatten().and_then(|c| c.try_extract_into::<i64>().ok()).unwrap_or(0) }
    // The node lists the offers; each one is then proven unspent under the agreed tip (a proof only above it, against
    // a block one node has shown, is not used) and checked to sit under the Babel contract for this token, so a node
    // can hide an offer but cannot invent one. An error is a lookup that could not be made or could not be checked (the
    // node did not answer, or its proof is against a block the agreed chain does not have yet): that is not "no offers",
    // and a caller must be able to tell the two apart to know that asking again may find one.
fn proven_babel_offers(c: &Ctx, token: &str) -> Result<Vec<ErgoBox>, String> {
    let Ctx { node, chain, state, .. } = c;
    let price_of = babel_price;
    let Some(tree) = babel_tree(token) else { return Ok(vec![]) };
    let n = crate::nodes::each(node).into_iter().find(|n| !n.starts_with("p2p://")).unwrap_or_default();
    let Ok(r) = http().post(&format!("{n}/blockchain/box/unspent/byErgoTree?offset=0&limit=50")).set("Content-Type", "application/json").send_string(&format!("\"{}\"", babel_hex(token))) else { return Err("no node answered when asked for the offers".into()) };
    let ids: Vec<String> = serde_json::from_str::<serde_json::Value>(&r.into_string().unwrap_or_default()).ok().and_then(|v| v.as_array().map(|x| x.iter().filter_map(|b| b["boxId"].as_str().map(String::from)).collect())).unwrap_or_default();
    if ids.is_empty() { return Ok(vec![]) }
    let mut cost = RestCost::default();
    match prove_any_agreed(node, chain, agreed_height(state), &ids, &mut cost) {
        Ok((_, found)) => Ok(found.into_iter().flatten().filter_map(|b| ErgoBox::sigma_parse_bytes(&b).ok()).filter(|b| b.ergo_tree == tree && price_of(b) > 0).collect()),
        Err(e) => Err(e),
    }
}

/// wallet babel-list: the offers a node lists for a token, each proven. `proven` says the lookup was checked against
/// the agreed tip; when it could not be (exit 1), the empty list means "not known", not "none".
fn babel_list(s: &Wanted) -> Ended {
    let (token_hex, node, price_of) = (&s.token_hex, &s.c.node, babel_price);
    let babel_offers = |token: &str| proven_babel_offers(s.c, token);
    if token_hex.len() != 64 || node.is_empty() { return Err("babel-list needs --token ID and --node".into()) }
    crate::out::set("offers", serde_json::json!([]));
    let offers = match babel_offers(token_hex) {
        Ok(found) => { crate::out::set("proven", serde_json::json!(true)); found }
        Err(e) => { crate::out::set("proven", serde_json::json!(false)); return Err(format!("the offers could not be checked: {e}").into()) }
    };
    if offers.is_empty() { say!("no proven offers for that token"); }
    for b in &offers { crate::out::push("offers", serde_json::json!({ "box": String::from(b.box_id()), "available_nanoerg": b.value.as_u64().saturating_sub(1_000_000), "nanoerg_per_unit": price_of(b) })); say!("offer {}  {} nanoERG available at {} nanoERG per token unit", String::from(b.box_id()), b.value.as_u64().saturating_sub(1_000_000), price_of(b)); }
    Ok(())
}

/// wallet babel-offer: ERG put up for others to take in exchange for a token.
fn offer_built(s: &Wanted) -> Result<Built, Fail> {
    let (args, token_hex, addr, inputs) = (s.c.args, &s.token_hex, &s.addr, &s.inputs);
    let (amount, v_out, v_need, height) = (s.amount, s.v_out, s.v_need, s.height);
    let mut other_trees = vec![];
    // put ERG up for others to take in exchange for a token, at our price (nanoERG per smallest token unit)
    let price: i64 = arg(args, "--price", 0)?;
    let Some(tree) = babel_tree(token_hex) else { return Err("babel-offer needs --token ID".into()) };
    let Address::P2Pk(pk) = addr.clone() else { return Err("wallet address is not a plain key".into()) };
    if price <= 0 || amount < 2_000_000 { return Err("babel-offer needs --price above zero and --nanoerg of at least 2000000".into()) }
    let selection = match SimpleBoxSelector::new().select(inputs.clone(), v_need, &[]) { Ok(s) => s, Err(e) => return Err(format!("box selection: {e:?}").into()) };
    let mut ob = ErgoBoxCandidateBuilder::new(v_out, tree.clone(), height);
    ob.set_register_value(R4, Constant::from(pk)); ob.set_register_value(R5, Constant::from(price));
    other_trees.push(tree);
    say!("offering {} nanoERG for token {} at {} nanoERG per token unit", amount, token_hex, price);
    let (selection, outs) = (selection, vec![match ob.build() { Ok(o) => o, Err(e) => return Err(format!("output: {e:?}").into()) }]);
    Ok(Built { selection, outs, payee_trees: vec![s.payee_tree.clone()], other_trees, burn: vec![], extension: None })
}

/// wallet send --babel: a payment whose fee is paid in a token, with ERG taken from someone's offer.
fn babel_paid_built(s: &Wanted) -> Result<Built, Fail> {
    let (node, inputs, want_tokens, more, babel_token, payee_tree) = (&s.c.node, &s.inputs, s.want_tokens.clone(), s.more.clone(), s.babel_token.clone(), &s.payee_tree);
    let (need_total, babel_max, v_out, height) = (s.need_total, s.babel_max, s.v_out, s.height);
    let (mut payee_trees, mut other_trees) = (vec![payee_tree.clone()], vec![]);
    let price_of = babel_price;
    let babel_offers = |token: &str| proven_babel_offers(s.c, token);
    let extension;
    let Some(tree) = babel_tree(&babel_token) else { return Err("--babel wants the 64-hex id of the token to pay the fee in".into()) };
    if node.is_empty() { return Err("--babel needs --node to find offers".into()) }
    if babel_max == 0 { return Err("--babel needs --babel-max RAW: the most token units the fee may cost. `wallet babel-list` shows the offers and their prices".into()) }
    // every box of ours goes in; the change box holds whatever is left
    let mine: Vec<ErgoBox> = inputs.iter().take(40).cloned().collect();
    let my_erg: u64 = mine.iter().map(|b| *b.value.as_u64()).sum();
    let mut left: std::collections::BTreeMap<String, u64> = Default::default();
    for t in mine.iter().flat_map(|b| b.tokens.iter().flat_map(|ts| ts.iter())) { *left.entry(String::from(t.token_id)).or_default() += *t.amount.as_u64(); }
    // what the payment needs in ERG: the payees, the fee, and the least a change box may hold
    let need = need_total.checked_add(1_000_000).ok_or("the amounts add up to more than can exist")?;
    if my_erg >= need { return Err("this wallet has enough ERG for the fee; send without --babel".into()) }
    let short = need - my_erg;
    for t in &want_tokens { let have = left.entry(String::from(t.token_id)).or_default(); if *have < *t.amount.as_u64() { return Err("not enough of a token being sent".into()) } *have -= *t.amount.as_u64(); }
    // the best offer: the one that asks the fewest tokens and can still hold a box's minimum afterwards
    let offers = babel_offers(&babel_token).map_err(|e| format!("the offers for the fee could not be checked: {e}"))?;
    let have_fee_token = *left.get(&babel_token).unwrap_or(&0);
    let cost_in = |b: &ErgoBox| -> u64 { let p = price_of(b) as u64; short.div_ceil(p) };
    let Some(offer) = offers.iter().filter(|b| *b.value.as_u64() >= short + 1_000_000 && cost_in(b) <= have_fee_token).min_by_key(|b| cost_in(b)) else {
        // which of the two it is: no offer holds enough ERG, or the wallet has too few of the token left after what it is sending
        let cheapest = offers.iter().filter(|b| *b.value.as_u64() >= short + 1_000_000).map(&cost_in).min();
        return Err(match cheapest {
            Some(cost) => format!("the fee would cost {cost} units of the token and {have_fee_token} are left after what is being sent: send {} fewer, or pay the fee in ERG", cost - have_fee_token),
            None => format!("no proven offer holds the {short} nanoERG this payment is short of ({} offers seen)", offers.len()),
        }.into())
    };
    let paid = cost_in(offer);
    // The node lists the offers, so it can leave the good ones out. Nothing is signed above what was agreed to.
    if paid > babel_max { return Err(format!("the fee would cost {paid} token units at the best offer the node lists now, more than the {babel_max} allowed; nothing was signed. Look the price up again").into()) }
    *left.entry(babel_token.clone()).or_default() -= paid;
    crate::out::set("fee_token", &babel_token); crate::out::set("fee_token_units", paid);
    say!("fee in tokens: taking {short} nanoERG from offer {} at {} nanoERG per unit, paying {paid} token units", String::from(offer.box_id()), price_of(offer));
    // the offer box, recreated with less ERG and more tokens; the contract checks this output
    let Ok(v_back) = BoxValue::try_from(*offer.value.as_u64() - short) else { return Err("offer value".into()) };
    let mut bb = ErgoBoxCandidateBuilder::new(v_back, tree.clone(), height);
    let held: Vec<Token> = offer.tokens.iter().flat_map(|ts| ts.iter().cloned()).collect();
    let before = held.first().map(|t| *t.amount.as_u64()).unwrap_or(0);
    let (Some(fee_id), Ok(amt)) = (ergo_chain_types::Digest32::try_from(babel_token.clone()).ok().map(TokenId::from), TokenAmount::try_from(before + paid)) else { return Err("fee token".into()) };
    bb.add_token(Token { token_id: fee_id, amount: amt });
    for t in held.iter().skip(1) { bb.add_token(*t); }
    let (Ok(Some(r4)), Ok(Some(r5))) = (offer.additional_registers.get_constant(R4), offer.additional_registers.get_constant(R5)) else { return Err("offer registers".into()) };
    let id_bytes: Vec<u8> = ergo_chain_types::Digest32::from(offer.box_id()).0.to_vec();
    bb.set_register_value(R4, r4); bb.set_register_value(R5, r5); bb.set_register_value(R6, Constant::from(id_bytes));
    let mut ob = ErgoBoxCandidateBuilder::new(v_out, payee_tree.clone(), height);
    for t in &want_tokens { ob.add_token(*t); }
    let mut outs = vec![match ob.build() { Ok(o) => o, Err(e) => return Err(format!("output: {e:?}").into()) }];
    for (t, value) in &more {
        let Ok(v) = BoxValue::try_from(*value) else { return Err("--pay amount out of range".into()) };
        outs.push(match ErgoBoxCandidateBuilder::new(v, t.clone(), height).build() { Ok(o) => o, Err(e) => return Err(format!("output: {e:?}").into()) });
        payee_trees.push(t.clone());
    }
    outs.push(match bb.build() { Ok(o) => o, Err(e) => return Err(format!("offer output: {e:?}").into()) });
    // the contract is told where its recreated box sits among the outputs
    let mut ext = ergotree_ir::chain::context_extension::ContextExtension::empty();
    ext.values.insert(0, Constant::from((outs.len() - 1) as i32));
    extension = Some((offer.box_id(), ext));
    other_trees.push(tree);
    let change_tokens: Vec<Token> = left.iter().filter(|(_, a)| **a > 0).filter_map(|(id, a)| Some(Token { token_id: TokenId::from(ergo_chain_types::Digest32::try_from(id.clone()).ok()?), amount: TokenAmount::try_from(*a).ok()? })).collect();
    let Ok(v_change) = BoxValue::try_from(1_000_000u64) else { return Err("value".into()) };
    let change = ErgoBoxAssetsData { value: v_change, tokens: BoxTokens::from_vec(change_tokens).ok() };
    let mut all = mine.clone(); all.push(offer.clone());
    let Ok(bx) = all.try_into() else { return Err("box list".into()) };
    let (selection, outs) = (BoxSelection { boxes: bx, change_boxes: vec![change] }, outs);
    Ok(Built { selection, outs, payee_trees, other_trees, burn: vec![], extension })
}

/// wallet consolidate: every tracked box into one, back to ourselves.
fn consolidation_built(s: &Wanted) -> Result<Built, Fail> {
    let (inputs, my_tree, fee, height) = (&s.inputs, &s.my_tree, s.fee, s.height);
    // every tracked box (up to 60) into one box back to ourselves: fewer boxes means smaller proofs later
    let boxes: Vec<ErgoBox> = inputs.iter().take(60).cloned().collect();
    if boxes.len() < 2 { return Err("nothing to consolidate: fewer than two boxes".into()) }
    let total: u64 = boxes.iter().map(|b| *b.value.as_u64()).sum();
    let mut sums: Vec<(TokenId, u64)> = vec![];
    for t in boxes.iter().flat_map(|b| b.tokens.iter().flat_map(|ts| ts.iter())) {
        match sums.iter_mut().find(|x| x.0 == t.token_id) { Some(x) => x.1 += *t.amount.as_u64(), None => sums.push((t.token_id, *t.amount.as_u64())) }
    }
    let Ok(v) = BoxValue::try_from(total.saturating_sub(fee)) else { return Err("not enough ERG to pay the fee".into()) };
    let mut ob = ErgoBoxCandidateBuilder::new(v, my_tree.clone(), height);
    for (id, amt) in &sums { if let Ok(a) = TokenAmount::try_from(*amt) { ob.add_token(Token { token_id: *id, amount: a }); } }
    let out = match ob.build() { Ok(o) => o, Err(e) => return Err(format!("output: {e:?}").into()) };
    let Ok(bx) = boxes.clone().try_into() else { return Err("box list".into()) };
    say!("consolidating {} boxes ({} token kinds) into one", boxes.len(), sums.len());
    let (selection, outs) = (BoxSelection { boxes: bx, change_boxes: vec![] }, vec![out]);
    Ok(Built { selection, outs, payee_trees: vec![s.payee_tree.clone()], other_trees: vec![], burn: vec![], extension: None })
}

/// wallet burn: some of a token destroyed.
fn burn_built(s: &Wanted) -> Result<Built, Fail> {
    let (inputs, my_tree, token_hex, pay_token) = (&s.inputs, &s.my_tree, &s.token_hex, s.pay_token);
    let (fee, height, token_amount) = (s.fee, s.height, s.token_amount);
    let mut burn = vec![];
    // destroy some of a token: it is selected as an input and appears in no output, which needs an explicit permit
    let Some(t) = pay_token else { return Err("burn needs --token ID and --token-amount RAW".into()) };
    let Ok(need) = BoxValue::try_from(1_000_000 + fee) else { return Err("fee out of range".into()) };
    let sel = match SimpleBoxSelector::new().select(inputs.clone(), need, &[t]) { Ok(x) => x, Err(e) => return Err(format!("box selection: {e:?}").into()) };
    let Ok(v) = BoxValue::try_from(1_000_000u64) else { return Err("value".into()) };
    let out = match ErgoBoxCandidateBuilder::new(v, my_tree.clone(), height).build() { Ok(o) => o, Err(e) => return Err(format!("output: {e:?}").into()) };
    say!("burning {} units of token {}", token_amount, token_hex);
    burn.push(t);
    let (selection, outs) = (sel, vec![out]);
    Ok(Built { selection, outs, payee_trees: vec![s.payee_tree.clone()], other_trees: vec![], burn, extension: None })
}

/// wallet send: a payment to one payee or several, of ERG and tokens; or a new token minted to ourselves.
fn payment_built(s: &Wanted) -> Result<Built, Fail> {
    let (inputs, want_tokens, more, payee_tree, mint_name) = (&s.inputs, s.want_tokens.clone(), s.more.clone(), &s.payee_tree, &s.mint_name);
    let (v_need, v_out, height, token_amount, mint_decimals) = (s.v_need, s.v_out, s.height, s.token_amount, s.mint_decimals);
    let mut payee_trees = vec![payee_tree.clone()];
    let selection = match SimpleBoxSelector::new().select(inputs.clone(), v_need, &want_tokens) { Ok(s) => s, Err(e) => return Err(format!("box selection: {e:?}").into()) };
    let mut ob = ErgoBoxCandidateBuilder::new(v_out, payee_tree.clone(), height);
    for t in &want_tokens { ob.add_token(*t); }
    if !mint_name.is_empty() {
        // a new token's id is the id of the first box the transaction spends; its name and decimals go in the box's registers
        let Ok(amt) = TokenAmount::try_from(token_amount) else { return Err("--mint needs --token-amount above zero".into()) };
        let id = TokenId::from(selection.boxes.first().box_id());
        ob.mint_token(Token { token_id: id, amount: amt }, mint_name.clone(), String::new(), mint_decimals);
        say!("minting {} units of '{}' ({} decimals), token id {}", token_amount, mint_name, mint_decimals, String::from(id));
    }
    let mut outs = vec![match ob.build() { Ok(o) => o, Err(e) => return Err(format!("output: {e:?}").into()) }];
    for (tree, value) in &more {
        let Ok(v) = BoxValue::try_from(*value) else { return Err("--pay amount out of range".into()) };
        outs.push(match ErgoBoxCandidateBuilder::new(v, tree.clone(), height).build() { Ok(o) => o, Err(e) => return Err(format!("output: {e:?}").into()) });
        payee_trees.push(tree.clone());
    }
    let (selection, outs) = (selection, outs);
    Ok(Built { selection, outs, payee_trees, other_trees: vec![], burn: vec![], extension: None })
}

/// A payment given up as "not included" is still a valid transaction: a node may yet hold it, and so may a payee
/// handed it offline. The coins it spends count as this wallet's again, so the next payment could be made from
/// other boxes and both could confirm. So the next payment always spends one of those coins: then only one of the
/// two can ever be in the chain. (Its value comes straight back as change.)
fn with_a_released_coin(selection: BoxSelection<ErgoBox>, inputs: &[ErgoBox], done: &[Done]) -> Result<BoxSelection<ErgoBox>, Fail> {
    let released: Vec<String> = done.iter().filter(|d| d.result == "not included").flat_map(|d| d.inputs.iter().cloned()).collect();
    let is_released = |b: &ErgoBox| released.contains(&String::from(b.box_id()));
    Ok(match inputs.iter().find(|b| is_released(b)) {
        Some(extra) if !selection.boxes.iter().any(is_released) => {
            let mut boxes = selection.boxes.as_vec().clone(); boxes.push(extra.clone());
            let mut change = selection.change_boxes.clone();
            change.push(ErgoBoxAssetsData { value: extra.value, tokens: extra.tokens.clone() });
            BoxSelection { boxes: boxes.try_into().map_err(|_| "box list".to_string())?, change_boxes: change }
        }
        _ => selection,
    })
}

/// The library pays the fee to mainnet's fee script. A network with another reward delay has
/// another script; swap it in on the one output that is neither the payment nor our change.
fn with_fee_script(unsigned: UnsignedTransaction, payee_trees: &[ErgoTree], my_trees: &[ErgoTree], other_trees: &[ErgoTree]) -> Result<UnsignedTransaction, Fail> {
    if net().fee_tree.is_empty() { return Ok(unsigned) }
    let Some(ft) = unhex(net().fee_tree).and_then(|b| ErgoTree::sigma_parse_bytes(&b).ok()) else { return Err("net file fee_tree is not a script".into()) };
    let outs: Vec<_> = unsigned.output_candidates.iter().cloned().map(|mut c| { if !payee_trees.contains(&c.ergo_tree) && !my_trees.contains(&c.ergo_tree) && !other_trees.contains(&c.ergo_tree) { c.ergo_tree = ft.clone(); } c }).collect();
    match UnsignedTransaction::new_from_vec(unsigned.inputs.as_vec().clone(), unsigned.data_inputs.as_ref().map(|d| d.as_vec().clone()).unwrap_or_default(), outs) { Ok(t) => Ok(t), Err(e) => Err(format!("fee script swap: {e:?}").into()) }
}

/// What this transaction takes out of the wallet, read off the transaction itself: what its own boxes put in,
/// less what comes back to its own addresses. A spending policy, if the wallet has one, is applied to this.
/// Handed back: the nanoERG that leaves (the network fee is in it), the token units that leave by token id, and the
/// address of every output that is neither the wallet's own nor a fee. Nothing is read but the transaction.
fn leaves_wallet(spent: &[ErgoBox], outputs: &[ErgoBoxCandidate], my_trees: &[ErgoTree], extension: Option<&(BoxId, ContextExtension)>, enc: &AddressEncoder) -> (u64, std::collections::BTreeMap<String, u64>, Vec<String>) {
    let fee_tree = fee_tree_hex();
    let (mut nano_in, mut nano_back) = (0u64, 0u64);
    let (mut tok_in, mut tok_back): (std::collections::BTreeMap<String, u64>, std::collections::BTreeMap<String, u64>) = Default::default();
    for b in spent.iter().filter(|b| my_trees.contains(&b.ergo_tree)) {
        nano_in = nano_in.saturating_add(*b.value.as_u64());
        for t in b.tokens.iter().flat_map(|ts| ts.iter()) { let n = tok_in.entry(String::from(t.token_id)).or_default(); *n = n.saturating_add(*t.amount.as_u64()); }
    }
    // A Babel fee recreates the offer box it takes ERG from, under the offer's own script: the fee's counterpart, as
    // the miner's fee output is, not a payee the owner chose. Only that box: the output the contract is told it
    // is, under the script of the offer spent, with no more ERG than the offer had (it takes none of ours). Any
    // other output to that script is a payee. The tokens it is paid still count as leaving the wallet.
    let offer_back: Option<(usize, &ErgoBox)> = extension.and_then(|(id, ext)| {
        let at = ext.values.get(&0)?.clone().try_extract_into::<i32>().ok()?;
        Some((usize::try_from(at).ok()?, spent.iter().find(|b| b.box_id() == *id)?))
    });
    let mut payees: Vec<String> = vec![];
    for (i, o) in outputs.iter().enumerate() {
        if offer_back.is_some_and(|(at, offer)| at == i && o.ergo_tree == offer.ergo_tree && o.value.as_u64() <= offer.value.as_u64()) { continue }
        if my_trees.contains(&o.ergo_tree) {
            nano_back = nano_back.saturating_add(*o.value.as_u64());
            for t in o.tokens.iter().flat_map(|ts| ts.iter()) { let n = tok_back.entry(String::from(t.token_id)).or_default(); *n = n.saturating_add(*t.amount.as_u64()); }
        } else {
            let tree_hex = o.ergo_tree.sigma_serialize_bytes().map(|b| hex(&b)).unwrap_or_default();
            if tree_hex == fee_tree { continue }
            let a = Address::recreate_from_ergo_tree(&o.ergo_tree).map(|a| enc.address_to_str(&a)).unwrap_or_else(|_| format!("script {}", clip(&tree_hex, 40)));
            if !payees.contains(&a) { payees.push(a); }
        }
    }
    let tokens = tok_in.into_iter().filter_map(|(id, n)| { let left = n.saturating_sub(*tok_back.get(&id).unwrap_or(&0)); (left > 0).then_some((id, left)) }).collect();
    (nano_in.saturating_sub(nano_back), tokens, payees)
}

/// A payment signed: say what it is, keep it, and hand it on as asked: written as a parcel for someone to carry
/// (`--parcel-out`), sent to a node (`--broadcast`), or left in the state directory.
fn hand_over(c: &Ctx, mut w: WalletFile, signed: Transaction, spent: Vec<ErgoBox>, payee_trees: &[ErgoTree], my_trees: &[ErgoTree]) -> Ended {
    let Ctx { args, state, node, .. } = c;
    let json = serde_json::to_string(&signed).unwrap_or_default();
    let bx = |b: &ErgoBox| String::from(b.box_id());
    let payee_ids: Vec<String> = signed.outputs.iter().filter(|o| payee_trees.contains(&o.ergo_tree)).map(bx).collect();
    let change_ids: Vec<String> = signed.outputs.iter().filter(|o| my_trees.contains(&o.ergo_tree)).map(bx).collect();
    crate::out::set("signed", true); crate::out::set("tx", String::from(signed.id())); crate::out::set("receipt", &payee_ids); crate::out::set("change", &change_ids);
    say!("signed: tx {} ({} B as JSON), {} inputs, {} outputs", signed.id(), json.len(), spent.len(), signed.outputs.len());
    say!("receipt for the payee (box ids to hand over): {}", payee_ids.join(","));
    say!("change we will look for ourselves: {}", if change_ids.is_empty() { "none".into() } else { change_ids.join(",") });
    policy::note(state, &String::from(signed.id()), payee_ids.first().map(|s| s.as_str()).unwrap_or(""));
    let _ = write_private(&format!("{state}/last-tx.json"), json.as_bytes());
    let parcel_out: String = arg(args, "--parcel-out", String::new())?;
    if !parcel_out.is_empty() {
        // offline: write the payment as a parcel for someone else to carry, and do our own
        // bookkeeping now (inputs are spent as far as we are concerned; change is expected)
        use ergotree_ir::serialization::SigmaSerializable;
        let bytes = match signed.sigma_serialize_bytes() { Ok(b) => b, Err(e) => return Err(format!("serialize: {e:?}").into()) };
        let parcel = serde_json::json!({ "tx_id": String::from(signed.id()), "tx_bytes": hex(&bytes), "tx_json": json });
        if std::fs::write(&parcel_out, parcel.to_string()).is_err() { return Err(format!("cannot write {parcel_out}").into()) }
        let mut expect: Vec<String> = w.expect.clone();
        expect.extend(change_ids.clone());
        for b in &spent { w.boxes.remove(&bx(b)); }
        w.expect = expect;
        note_pending(&mut w, &signed, &json);
        c.save(&w)?;
        crate::out::set("parcel", &parcel_out); crate::out::set("tx_bytes", hex(&bytes));
        say!("parcel written to {parcel_out} ({} B of transaction); hand it to any wallet that can reach the network", bytes.len());
        return Ok(());
    }
    if args.iter().any(|a| a == "--broadcast") {
        if node.is_empty() { return Err("--broadcast needs --node".into()) }
        // written down as waiting before any node is sent it (see `cmd_request`): spent inputs go, change is expected
        let before = w.clone();
        let mut expect: Vec<String> = w.expect.clone();
        expect.extend(change_ids);
        for b in &spent { w.boxes.remove(&bx(b)); }
        w.expect = expect;
        note_pending(&mut w, &signed, &json);
        c.save(&w)?;
        let unsure = match send_tx(node, &json) {
            Ok((n, said)) => { crate::out::set("broadcast", serde_json::json!({ "node": n, "accepted": true })); say!("broadcast via {n}: accepted, {said}"); None }
            // a refusal, or no node reached at all: nothing went out, and the wallet is put back as it was
            Err(NotSent::Refused(code, body)) => { c.save(&before)?; return Err(format!("node refused ({code}): {}", one_line(&body)).into()) }
            Err(NotSent::Unreachable(e)) => { c.save(&before)?; return Err(format!("broadcast: {e}").into()) }
            Err(NotSent::Unknown(e)) => Some(format!("the node did not answer ({e}). The payment may have gone out: it is kept as waiting, its coins are not counted, and the next sync settles it (sent again if it did not arrive; back in the wallet if it is not in a block after ten). Do not pay again until then")),
        };
        if unsure.is_some() { crate::out::set("outcome_unknown", true); }
        if let Some(e) = unsure { return Err(e.into()) }
    } else {
        say!("not broadcast (add --broadcast); signed transaction saved to {state}/last-tx.json");
    }
    Ok(())
}

/// wallet send | consolidate | burn | babel-offer | babel-list: what is asked for is read (`wanted`), the transaction
/// that does it is built (one builder for each), what it takes out of the wallet is read off it and held to the
/// spending policy (`leaves_wallet`), and only then is it signed and handed on (`hand_over`).
fn cmd_send(c: &Ctx, o: Open) -> Ended {
    let Ctx { args, state, chain, enc, .. } = c;
    let sub = c.sub;
    let Open { w, phrase, no_phrase, locked, bad_password, addr, my_tree, n_addr, my_trees, .. } = o;
    let s = wanted(c, &w, &addr, &my_tree)?;
    if sub == "babel-list" { return babel_list(&s) }
    let Built { selection, outs, payee_trees, other_trees, burn, extension } = match sub {
        "babel-offer" => offer_built(&s)?,
        "send" if !s.babel_token.is_empty() && s.mint_name.is_empty() => babel_paid_built(&s)?,
        "consolidate" => consolidation_built(&s)?,
        "burn" => burn_built(&s)?,
        _ => payment_built(&s)?,
    };
    let selection = with_a_released_coin(selection, &s.inputs, &w.done)?;
    let (height, v_fee, fee) = (s.height, s.v_fee, s.fee);
    let mut tb = TxBuilder::new(selection.clone(), outs, height, v_fee, addr.clone());
    if !burn.is_empty() { tb.set_token_burn_permit(burn.clone()); }
    if let Some((id, ext)) = extension.clone() { tb.set_context_extension(id, ext); }
    let unsigned = match tb.build() { Ok(t) => t, Err(e) => return Err(format!("build: {e:?}").into()) };
    let unsigned = with_fee_script(unsigned, &payee_trees, &my_trees, &other_trees)?;
    let headers: [Header; 10] = match chain[..10].to_vec().try_into() { Ok(h) => h, Err(_) => return Err("header context".into()) };
    let ctx = ErgoStateContext::new(PreHeader::from(headers[0].clone()), headers, Parameters::default());
    let spent: Vec<ErgoBox> = selection.boxes.as_vec().clone();
    // a spending policy, if the wallet has one, is applied to this
    let (nano, tokens, payees) = leaves_wallet(&spent, unsigned.output_candidates.as_vec(), &my_trees, extension.as_ref(), enc);
    let ask = policy::Ask { nano, tokens, payees, purpose: arg(args, "--purpose", String::new())?, what: sub.to_string() };
    let tc = match TransactionContext::new(unsigned, spent.clone(), vec![]) { Ok(t) => t, Err(e) => return Err(format!("context: {e:?}").into()) };
    if args.iter().any(|a| a == "--request-out") {
        // not signed: written as a signing request (ErgoPay, static form) for a wallet to show, sign and send
        use base64::Engine;
        let reduced = match ergo_lib::chain::transaction::reduced::reduce_tx(tc.clone(), &ctx) { Ok(r) => r, Err(e) => return Err(format!("reduce: {e:?}").into()) };
        let bytes = match reduced.sigma_serialize_bytes() { Ok(b) => b, Err(e) => return Err(format!("serialize: {e:?}").into()) };
        crate::out::set("request", format!("ergopay:{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes))); crate::out::set("signed", false);
        say!("ergopay:{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes));
        return Ok(());
    }
    // what the payment does, read off the transaction as built: the same numbers the spending policy is held to
    crate::out::set("leaves_wallet_nanoerg", ask.nano); crate::out::set("tokens_out", &ask.tokens); crate::out::set("payees", &ask.payees); crate::out::set("fee_nanoerg", fee); crate::out::set("signed", false);
    if args.iter().any(|a| a == "--dry-run") {
        // a wallet with no phrase at all (read-only) could never sign this, nor one given a wrong password: a dry run that
        // says yes would mislead
        if phrase.is_none() && (!locked || bad_password) { return Err(no_phrase.into()) }
        // built and checked against the policy, as a real payment is; nothing is signed, written or counted
        if let Err(e) = policy::reserve(state, &ask, true) { return Err(format!("the spending policy does not allow it: {e}").into()) }
        say!("dry run: {} nanoERG would leave this wallet (fee {fee} included), to {}; nothing signed", ask.nano, if ask.payees.is_empty() { "itself".to_string() } else { ask.payees.join(", ") });
        return Ok(());
    }
    let Some(p) = phrase.as_deref() else { return Err(no_phrase.into()) };
    let secrets = secrets(p, n_addr);
    if secrets.is_empty() { return Err("derivation".into()) }
    // written to the payments log before the signature is made, so it counts whether or not the payment goes through
    if let Err(e) = policy::reserve(state, &ask, false) { return Err(format!("the spending policy does not allow it: {e}").into()) }
    let signed = match Wallet::from_secrets(secrets).sign_transaction(tc, &ctx, None) { Ok(t) => t, Err(e) => return Err(format!("sign: {e:?}").into()) };
    hand_over(c, w, signed, spent, &payee_trees, &my_trees)
}

/// One proof, and what it was asked: the boxes asked about and, for each, its bytes if it is unspent at `height`.
/// `settled`: the proof is at or below the tip the peers agreed on (`agreed`), so it can take away as well as add.
#[derive(Clone, Copy)]
struct Shown<'a> { ids: &'a [String], found: &'a [Option<Vec<u8>>], height: u32, agreed: u32, settled: bool, chain: &'a [Header] }

/// What a sync changes as it reads a proof: the boxes expected, the boxes that can never be, and the boxes undone.
struct Books { expect: Vec<String>, void: Vec<String>, undone: Vec<String> }

/// What settling the wallet's own payments leaves for the boxes to be read by.
struct Sent {
    /// boxes that payments of this wallet, waiting or settled, spend
    spent_by_us: Vec<String>,
    /// boxes a payment still waiting spends: not this wallet's to count or spend again
    held: Vec<String>,
    /// boxes a payment of this wallet spent or made: nothing to say of them on a sync
    quiet: Vec<String>,
    /// a payment's outcome was unclear: the nodes are to be asked again what the addresses hold
    refresh: bool,
}

/// `wallet receive --pools`: a box asked about that this wallet neither has nor expects may be paid by a payment not yet in a block.
/// If a node holds that payment in its pool it is fetched (`pending_payment`: it is the payment that makes that box,
/// as far as its body goes; a payment's id does not cover its signatures). It is held as if the payer had handed it
/// over (`hold`) only if every coin it spends is proven unspent against the agreed chain and its spending proofs
/// verify against them: then it is a payment that can be made, and from then on its coins are proven with the box.
fn take_from_pools(c: &Ctx, w: &mut WalletFile, my_trees: &[ErgoTree], nodes: &[String], ids: &[String], cost: &mut RestCost) {
    let Ctx { state, node, chain, .. } = c;
    // a payment of this wallet's own, still waiting, is not someone else's to hold
    let own: Vec<String> = w.pending.iter().flat_map(|p| outputs_of(p, my_trees)).map(|x| x.0).collect();
    let own_tx: Vec<String> = w.pending.iter().map(|p| p.tx.clone()).collect();
    for id in ids {
        if w.boxes.contains_key(id) || w.expect.contains(id) || w.void.contains(id) || own.contains(id) { continue }
        let (found, answered) = pending_payment(nodes, id);
        // a node showed such a payment, held or not: then nobody can say there is none
        let offered = found.is_some();
        let mut held = false;
        if let Some(tx) = found.filter(|t| !own_tx.contains(&String::from(t.id()))) {
            let coins: Vec<String> = tx.inputs.iter().map(|i| String::from(i.box_id)).collect();
            let reads: Vec<String> = tx.data_inputs.as_ref().map(|d| d.iter().map(|i| String::from(i.box_id)).collect()).unwrap_or_default();
            let all: Vec<String> = coins.iter().chain(reads.iter()).cloned().collect();
            let mut seen_at = 0u64;
            let checked = prove_any_agreed(node, chain, agreed_height(state), &all, cost).map_err(|e| format!("its coins could not be proven ({e})")).and_then(|(h, f)| {
                seen_at = h as u64;
                let parse = |v: &[Option<Vec<u8>>]| v.iter().map(|b| b.as_ref().ok_or("a coin it spends is not an unspent box (spent, or made by a payment not yet in a block)".to_string()).and_then(|b| ErgoBox::sigma_parse_bytes(b).map_err(|e| format!("box decode: {e:?}")))).collect::<Result<Vec<_>, String>>();
                let (fi, fd) = f.split_at(coins.len());
                crate::proofs::verifies(&tx, parse(fi)?, parse(fd)?, chain)
            });
            match checked.and_then(|_| tx.sigma_serialize_bytes().map(|b| hex(&b)).map_err(|e| format!("{e:?}"))).and_then(|hexs| hold(state, w, my_trees, &hexs, &coins, seen_at)) {
                Ok(_) => { held = true; say!("  {id}  a node holds the payment that makes it, not yet in a block; its coins are unspent and its signatures verify: taken and held") }
                Err(why) => say!("  {id}  a node holds a payment that makes it, but it is not held: {why}"),
            }
        }
        crate::out::push("pools", serde_json::json!({ "id": id, "answered": answered, "offered": offered, "held": held }));
    }
}

/// Asks every node what it lists for each address, and adds what is not yet asked about: a node can hide a box
/// but cannot invent one, because everything listed is proven like a receipt. Handed back: whether every node
/// answered in full for every address, and whether any answered at all.
fn listed_at(nodes: &[String], addr_strs: &[String], ids: &mut Vec<String>, cost: &mut RestCost) -> (bool, bool) {
    let (mut all_listed, mut any_listed) = (true, false);
    for a in addr_strs { for n in nodes {
        let (mut listed, mut offset) = (vec![], 0usize);
        loop {
            cost.requests += 1; cost.sent += a.len();
            let Ok(r) = http().post(&format!("{n}/blockchain/box/unspent/byAddress?offset={offset}&limit=200")).set("Content-Type", "application/json").send_string(a) else { say!("reconcile: {n} did not answer"); all_listed = false; break };
            let text = r.into_string().unwrap_or_default();
            cost.received += text.len();
            any_listed = true;
            let page: Vec<String> = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v.as_array().map(|x| x.iter().filter_map(|b| b["boxId"].as_str().map(String::from)).collect())).unwrap_or_default();
            let n_page = page.len();
            listed.extend(page);
            if n_page < 200 { break }
            // a node that lists more than this is not followed further, and what it listed is not taken for everything
            if offset >= 1800 { say!("reconcile: {n} lists more than {} boxes for one address; only those are looked at", offset + 200); all_listed = false; break }
            offset += 200;
        }
        let fresh: Vec<String> = listed.iter().filter(|i| !ids.contains(i)).cloned().collect();
        say!("reconcile: {n} lists {} unspent boxes, {} not yet tracked", listed.len(), fresh.len());
        ids.extend(fresh);
    } }
    (all_listed, any_listed)
}

/// The first proof a node gives of the boxes asked about, against the stored chain or a few work-checked headers
/// the node adds to it. Those headers are kept for the next proof; what the peers agreed on is not touched.
fn proof_from(nodes: &[String], state: &str, chain: &mut Vec<Header>, agreed: u32, ids: &[String], cost: &mut RestCost) -> Option<Proven> {
    let mut proven = None;
    for n in nodes {
        if let Some(peer) = n.strip_prefix("p2p://") {
            match prove_p2p(peer, chain, ids, cost) { Ok(x) => { proven = Some(x); break } Err(e) => { say!("proof from {n}: {e}"); continue } }
        }
        match prove_reading(n, chain, Some(agreed), ids, cost) {
            Ok(x) => { proven = Some(x); break }
            Err(e) => {
                let added = extend_chain(n, chain, 3, cost);
                if added == 0 { say!("proof from {n}: {e}"); continue }
                say!("node is {added} block(s) past the agreed tip; extended with work-checked, linked headers from {n}");
                match prove_reading(n, chain, Some(agreed), ids, cost) { Ok(x) => { proven = Some(x); break } Err(e) => say!("proof from {n}: {e}") }
            }
        }
    }
    if proven.is_some() {
        // headers this node added are kept for the next proof, but the file stays short and `agreed-tip.bin` is not
        // touched: what the peers agreed on is still what it was. However many a node has added, the agreed tip and the
        // few below it stay in the file, so a proof from an honest node at the agreed tip can still be read (`prove_reading`)
        let keep = chain.iter().position(|h| h.height <= agreed).map_or(16, |i| 16.max(i + crate::chain::FRESH));
        if let Err(e) = write_whole(&format!("{state}/agreed-chain.bin"), &chain_bytes(chain.iter().take(keep))) { say!("note: the chain could not be saved in {state} ({e}); the one from before is kept"); }
    }
    proven
}

/// What became of the payments this wallet sent, read off the same proof as the balance (`payment_outcome`): each
/// one waiting is confirmed, given up, found to have its coins spent, or left waiting and sent again.
fn settle_sent(c: &Ctx, w: &mut WalletFile, shown: &Shown, nodes: &[String], my_trees: &[ErgoTree], books: &mut Books, cost: &mut RestCost) -> Sent {
    let Shown { ids, found, height, agreed, settled, chain } = *shown;
    let (node, expect) = (&c.node, &mut books.expect);
    const GIVE_UP: u64 = 10;
    // boxes that payments of this wallet (waiting or settled) spend, and every box a waiting payment creates
    let spent_by_us: Vec<String> = w.pending.iter().flat_map(|p| p.inputs.iter()).chain(w.done.iter().flat_map(|d| d.inputs.iter())).cloned().collect();
    let mut made: Vec<String> = vec![];
    let mut refresh = false;
    let present = |id: &str| ids.iter().position(|x| x == id).map(|i| found[i].is_some()).unwrap_or(false);
    let mut still: Vec<Pending> = vec![];
    let mut done: Vec<Done> = std::mem::take(&mut w.done);
    // A payment given up on is still watched. If a box it creates is there, it was included after all. If a box it
    // spends is gone and none it creates is there, it may have been included and spent on: that is settled by a
    // proof that it is in the chain, or left as "coins spent".
    let late: Vec<(String, Option<u32>)> = done.iter().filter(|d| settled && d.result == "not included" && !present(&d.watch) && d.inputs.iter().any(|i| !present(i)))
        .map(|d| (d.tx.clone(), prove_included(node, chain, &d.tx, cost).ok().filter(|h| *h <= agreed))).collect();
    for (tx, included) in late {
        let Some(d) = done.iter_mut().find(|d| d.tx == tx) else { continue };
        match included {
            Some(h) => { d.result = "confirmed".into(); d.seen_at = h as u64; d.other.insert("proof".into(), serde_json::json!("inclusion")); say!("payment {tx}: included after all: proven to be in the block at height {h}"); }
            None => { d.result = "coins spent".into(); d.seen_at = height as u64; say!("payment {tx}: given up on earlier; its coins have since been spent, by it or by something else"); }
        }
        crate::out::push("payments", serde_json::json!({ "tx": tx, "result": d.result, "height": included, "proof": included.map(|_| "inclusion") }));
        refresh = true;
    }
    for d in done.iter_mut().filter(|d| settled && d.result == "not included") {
        if present(&d.watch) { d.result = "confirmed".into(); d.seen_at = height as u64; crate::out::push("payments", serde_json::json!({ "tx": d.tx, "result": "confirmed" })); say!("payment {}: included after all; confirmed at height {height} or before", d.tx); }
    }
    // A payment settled earlier as "coins spent" (or, by an older build, "spent elsewhere") is given one try at a proof
    // that it is in the chain. If it is, it confirmed: the earlier word is put right.
    for d in done.iter_mut().filter(|d| settled && (d.result == "coins spent" || d.result == "spent elsewhere") && !d.other.contains_key("inclusion_tried")) {
        match prove_included(node, chain, &d.tx, cost) {
            Ok(h) if h <= agreed => {
                d.result = "confirmed".into(); d.seen_at = h as u64; d.other.insert("proof".into(), serde_json::json!("inclusion")); d.other.insert("inclusion_tried".into(), true.into());
                crate::out::push("payments", serde_json::json!({ "tx": d.tx, "result": "confirmed", "height": h, "proof": "inclusion" }));
                say!("payment {}: it did confirm: proven to be in the block at height {h}", d.tx);
            }
            Ok(_) => {}
            // a node that knows no such transaction has answered; one that could not be reached has not, and is asked again next time
            Err(e) => if e.contains("knows no such transaction") { d.other.insert("inclusion_tried".into(), true.into()); },
        }
    }
    for mut p in std::mem::take(&mut w.pending) {
        let tx = p.tx.clone();
        let inputs: Vec<String> = p.inputs.clone();
        let first = p.first_look.unwrap_or(height as u64);
        p.first_look = Some(first);
        let outs = outputs_of(&p, my_trees);
        let created = present(&p.watch) || outs.iter().any(|(id, _)| present(id));
        made.extend(outs.into_iter().map(|x| x.0));
        let verdict = payment_outcome(settled, created, inputs.iter().any(|i| !present(i)), (height as u64).saturating_sub(first) >= GIVE_UP);
        // What a node says of a payment the proof cannot settle: in a block, or unknown to it. A node's word, kept
        // apart from the verdict and shown as such; it is asked only then.
        // First, whether it can be proven to be in the chain all the same: the block a node names, linked by hash to
        // the agreed chain, and a Merkle proof from the transaction to that block (`prove_included`). Then it confirmed,
        // whatever became of its boxes since.
        let included: Option<u32> = (verdict == Some("coins spent")).then(|| prove_included(node, chain, &tx, cost).ok()).flatten().filter(|h| *h <= agreed);
        let verdict = if included.is_some() { Some("confirmed") } else { verdict };
        let reported: Option<String> = (verdict == Some("coins spent")).then(|| node_says(nodes, &tx)).flatten();
        match verdict {
            Some(v) => {
                crate::out::push("payments", serde_json::json!({ "tx": tx, "result": v, "node_says": reported, "height": included, "proof": included.map(|_| "inclusion") }));
                if v == "coins spent" || included.is_some() { refresh = true; }
                say!("payment {tx}: {}", match v {
                    "confirmed" => match included { Some(h) => format!("confirmed: proven to be in the block at height {h} (what it created has been spent since)"), None => format!("confirmed at height {height} or before") },
                    "coins spent" => format!("its coins are spent and nothing it created is unspent now. Either it confirmed and what it paid has been spent since (by the payee, or by another wallet of this phrase), or another transaction spent its coins: the proof cannot tell which. {} Check before paying again", match reported.as_deref() {
                        Some(r) => format!("A node says: {r} (its word, not proven)."), None => "No node could say more.".to_string() }),
                    _ => format!("NOT INCLUDED after {GIVE_UP} blocks; its coins count as this wallet's again") });
                if v != "confirmed" || included.is_some() {
                    // nothing it would have created is expected any more
                    let outs: Vec<String> = outputs_of(&p, my_trees).into_iter().map(|x| x.0).collect();
                    expect.retain(|e| !outs.contains(e));
                }
                let mut d = Done { tx, watch: p.watch.clone(), inputs: p.inputs.clone(), result: v.to_string(), seen_at: included.unwrap_or(height) as u64, ..Default::default() };
                if included.is_some() { d.other.insert("proof".into(), serde_json::json!("inclusion")); }
                if let Some(r) = &reported { d.other.insert("node_says".into(), serde_json::json!(r)); }
                done.insert(0, d);
            }
            None => {
                // (TACHYS_NO_RESEND is for tests of the not-included path, on a private network only)
                if height as u64 > first && !(private_net() && std::env::var_os("TACHYS_NO_RESEND").is_some()) { if let (Some(n), Some(j)) = (nodes.iter().find(|n| !n.starts_with("p2p://")), Some(p.json.as_str()).filter(|j| !j.is_empty())) { let _ = http().post(&format!("{n}/transactions")).set("Content-Type", "application/json").send_string(j); } }
                crate::out::push("payments", serde_json::json!({ "tx": tx, "result": "waiting" }));
                say!("payment {tx}: waiting, {} block(s) since first checked", (height as u64).saturating_sub(first));
                still.push(p);
            }
        }
    }
    // The last twenty settled payments are kept, and beyond them any payment given up as "not included" whose coins this
    // wallet still tracks: the next payment spends one of those coins (`cmd_send`), which is what keeps that payment and
    // the next from both confirming. Once its coins are spent or gone it ages out like the rest, so this stays bounded.
    let mut n = 0;
    done.retain(|d| { n += 1; n <= 20 || (d.result == "not included" && d.inputs.iter().any(|i| w.boxes.contains_key(i))) });
    // boxes of a payment still waiting are not this wallet's to count or spend again; boxes of a failed one are
    let held: Vec<String> = still.iter().flat_map(|p| p.inputs.iter().cloned()).collect();
    let quiet: Vec<String> = still.iter().flat_map(|p| p.inputs.iter().chain(std::iter::once(&p.watch))).chain(done.iter().flat_map(|d| d.inputs.iter().chain(std::iter::once(&d.watch)))).cloned().chain(made).collect();
    w.pending = still;
    w.done = done;
    Sent { spent_by_us, held, quiet, refresh }
}

/// A held payment none of whose boxes exist, one of whose coins this wallet had seen unspent and is now gone, at or
/// below the agreed tip, can never confirm: what it promised is void, and the payment is put aside (`given-up/`;
/// nothing sends it again). A coin never seen is not taken for gone: it may be made by a payment not yet in a block
/// (a payer who chains payments). Coins seen unspent now are written down with the payment.
fn settle_held(c: &Ctx, shown: &Shown, promises: &[Held], books: &mut Books, cost: &mut RestCost) {
    let Shown { ids, found, height, agreed, settled, chain } = *shown;
    let (node, state) = (&c.node, &c.state);
    let Books { expect, void, .. } = books;
    let present = |id: &str| ids.iter().position(|x| x == id).map(|i| found[i].is_some()).unwrap_or(false);
    for (file, v, inputs, outs) in promises {
        if !settled || outs.iter().any(|o| present(o)) { continue }
        // each coin with the height it was seen unspent at: a proof from below that height (a node behind) says nothing
        // about whether it is gone since
        let mut seen: serde_json::Map<String, serde_json::Value> = v["coins_seen"].as_object().cloned().unwrap_or_default();
        let fresh: Vec<String> = inputs.iter().filter(|i| present(i) && !seen.contains_key(*i)).cloned().collect();
        if !fresh.is_empty() { for f in fresh { seen.insert(f, (height as u64).into()); } let mut v = v.clone(); v["coins_seen"] = serde_json::Value::Object(seen.clone()); let _ = write_whole(&file.to_string_lossy(), v.to_string().as_bytes()); }
        let seen_below = |c: &String| seen.get(c).and_then(|h| h.as_u64()).is_some_and(|h| h <= height as u64);
        let aside = format!("{state}/given-up");
        // put aside as void, and its coins are all back (the payment that spent one was itself undone): held again
        if file.starts_with(&aside) && outs.iter().all(|o| void.contains(o)) && inputs.iter().all(|i| present(i)) {
            void.retain(|o| !outs.contains(o)); for o in outs { if !expect.contains(o) { expect.push(o.clone()); } }
            if let Some(name) = file.file_name() { let _ = std::fs::create_dir_all(format!("{state}/outbox")); let _ = std::fs::rename(file, std::path::Path::new(&format!("{state}/outbox")).join(name)); }
            say!("  a payment held for this wallet that was void has its coins back (what spent one was undone): it is held again");
            continue
        }
        let Some(gone) = inputs.iter().find(|i| !present(i) && seen_below(i)) else { continue };
        if void.iter().any(|o| outs.contains(o)) { continue }
        // a coin gone and no box of it there is also what a payment that confirmed, and whose boxes were spent since,
        // looks like: if it can be proven to be in the chain, it is not void
        // the id read from the payment's own bytes, never from the file's word
        if let Some(tx) = v["tx_bytes"].as_str().and_then(unhex).and_then(|b| ergo_lib::chain::transaction::Transaction::sigma_parse_bytes(&b).ok()).map(|t| String::from(t.id())) {
            if let Ok(h) = prove_included(node, chain, &tx, cost) { if h <= agreed { say!("  a payment held for this wallet is in the chain (proven, block {h}); what it made has been spent since"); continue } }
        }
        expect.retain(|e| !outs.contains(e));
        for o in outs { if !void.contains(o) { void.push(o.clone()); } }
        let _ = std::fs::create_dir_all(&aside);
        if let Some(name) = file.file_name() { if !file.starts_with(&aside) { let _ = std::fs::rename(file, std::path::Path::new(&aside).join(name)); } }
        crate::out::push("void", serde_json::json!({ "boxes": outs, "coin_gone": gone, "reason": "a coin it spends, seen unspent before, is gone" }));
        say!("  a payment held for this wallet can never confirm: its coin {}, seen unspent before, is not an unspent box at height {height}; what it promised ({}) is void", clip(gone, 16), outs.iter().map(|o| clip(o, 16)).collect::<Vec<_>>().join(", "));
    }
}

/// Each box asked about, read off the proof: kept, added, dropped as spent, or said to be undone, void or absent.
/// Handed back: whether a box went that no payment of this wallet spent (another wallet of the phrase may be at work).
fn settle_boxes(c: &Ctx, w: &mut WalletFile, shown: &Shown, sent: &Sent, books: &mut Books, my_trees: &[ErgoTree], proven_held: &[String]) -> bool {
    let Shown { ids, found, height, agreed, settled, chain } = *shown;
    let Sent { spent_by_us, held, quiet, .. } = sent;
    let Books { expect, void, undone } = books;
    let sub = c.sub;
    let mut refresh = false;
    // each box asked about, for a program: its id, what the proof showed ("proven": unspent and ours, at or below the agreed
    // tip; "seen": the same above it; "absent"; "spent"; "undone"; "void"; "expected"; "other_address"; "undecodable"; "absent_above_agreed_tip";
    // "absent_below_confirmed")
    let checked = |id: &str, state: &str| crate::out::push("checked", serde_json::json!({ "id": id, "state": state, "height": height }));
    // the boxes paying this wallet that its own payments still waiting make: built and signed here from coins proven
    // unspent under an agreed tip, which this sync has asked about again (a coin of one gone has settled it, above)
    let own_waiting: Vec<String> = w.pending.iter().flat_map(|p| outputs_of(p, my_trees)).filter(|x| x.1).map(|x| x.0).collect();
    // a box confirmed under a header the agreed chain no longer has there left with its block: undone, not spent.
    // Only the stored chain can say, so a reorganisation further back than it reaches still reads as a spend.
    let header_at = |h: u64| chain.iter().find(|x| x.height as u64 == h).map(|x| hex(&crate::chain::id32(x)));
    let boxes = &mut w.boxes;
    for (id, f) in ids.iter().zip(found.iter()) {
        if held.contains(id) { continue }
        match f.as_ref().map(|b| ErgoBox::sigma_parse_bytes(b)) {
            // a box one of this wallet's own payments spent or made: nothing to say on a sync, but a receipt asked about by name gets its answer
            None if quiet.contains(id) && !boxes.contains_key(id) && !expect.contains(id) => { if sub == "receive" { checked(id, "spent"); say!("  {id}  not unspent at height {height}: a payment of this wallet spent it"); } }
            Some(Ok(b)) if !my_trees.contains(&b.ergo_tree) && quiet.contains(id) => {}
            None if !settled && boxes.contains_key(id) => { checked(id, "absent_above_agreed_tip"); say!("  {id}  absent at height {height}, above the tip the peers agree on ({agreed}): kept until they reach it") }
            // A proof can be against a stored header below the one this box was confirmed at (a node a block or two behind):
            // there the box did not exist yet, and its absence says nothing of its being spent, nor of its block having left
            // the chain (`undone`, below): that is read from a proof at or above the height it was confirmed at.
            None if boxes.get(id).and_then(|b| b.confirmed_at).is_some_and(|c| c > height as u64) => {
                let c = boxes.get(id).and_then(|b| b.confirmed_at).unwrap_or(0);
                checked(id, "absent_below_confirmed"); say!("  {id}  absent at height {height}, below where it was confirmed ({c}): kept")
            }
            None => match boxes.remove(id) {
                Some(old) if settled && !spent_by_us.contains(id) && !old.confirmed_in.is_empty() && old.confirmed_at.and_then(header_at).is_some_and(|now| now != old.confirmed_in) => {
                    checked(id, "undone"); refresh = true;
                    if !undone.contains(id) { undone.push(id.clone()); }
                    say!("  {id}  gone at height {height}, and the block it was confirmed under (height {}) is no longer in the chain the peers agree on: UNDONE by a reorganisation. It may be included again, or its coins may have been spent elsewhere; until it is proven again it is not a payment", old.confirmed_at.unwrap_or(0));
                }
                Some(_) => { checked(id, "spent"); if !spent_by_us.contains(id) { refresh = true; } say!("  {id}  gone at height {height}: spent{}; removed", if spent_by_us.contains(id) { "" } else { " (not by a payment of this wallet)" }); }
                None if expect.contains(id) => { crate::out::push("checked", serde_json::json!({ "id": id, "state": "expected", "height": height, "held_proven": proven_held.contains(id), "own": own_waiting.contains(id) })); say!("  {id}  expected from our own payment, not on chain yet at {height}"); }
                // an undone box stays undone, asked about again, until it is back or its coins are spent
                None if undone.contains(id) => { checked(id, "undone"); say!("  {id}  undone by a reorganisation and not back in the chain at height {height}: not a payment now"); }
                None if void.contains(id) => { checked(id, "void"); say!("  {id}  VOID: the payment that was to make it can never confirm (a coin it spends is gone, or it was refused for good): not accepted"); }
                None => { checked(id, "absent"); say!("  {id}  ABSENT at height {height}: not accepted"); }
            },
            Some(Err(_)) => { checked(id, "undecodable"); say!("  {id}  present but undecodable: not accepted") }
            Some(Ok(b)) if !my_trees.contains(&b.ergo_tree) => { checked(id, "other_address"); say!("  {id}  exists but pays another address: not accepted") }
            Some(Ok(b)) => {
                // a box that was undone and is back is the same payment, not a new one
                let back = undone.contains(id);
                let new = !boxes.contains_key(id) && !back;
                undone.retain(|u| u != id); void.retain(|v| v != id);
                // the first height at or below an agreed tip at which this box was proven; confirmations are counted from it (a lower bound)
                let confirmed_at: Option<u64> = boxes.get(id).and_then(|x| x.confirmed_at).or((height <= agreed).then_some(height as u64));
                let confirmed_in: String = boxes.get(id).filter(|x| x.confirmed_at.is_some()).map(|x| x.confirmed_in.clone()).filter(|c| !c.is_empty()).or_else(|| confirmed_at.and_then(header_at)).unwrap_or_default();
                let confirmations = confirmed_at.map_or(0, |c| (agreed as u64 + 1).saturating_sub(c));
                let first_proven = boxes.get(id).map(|x| x.first_proven).filter(|h| *h > 0).unwrap_or(height as u64);
                boxes.insert(id.clone(), TrackedBox { value: *b.value.as_u64(), tokens: b.tokens.as_ref().map_or(0, |t| t.len()), assets: b.tokens.iter().flat_map(|t| t.iter()).map(|x| Asset { id: String::from(x.token_id), amount: *x.amount.as_u64() }).collect(),
                    proven_at: height as u64, first_proven, confirmed_at, confirmed_in, bytes: hex(f.as_ref().expect("present")), ..Default::default() });
                if back { say!("  {id}  back in the chain after it was undone: the same payment, not a new one"); }
                expect.retain(|e| e != id);
                crate::out::push("checked", serde_json::json!({ "id": id, "state": if confirmed_at.is_some() { "proven" } else { "seen" }, "height": height, "nanoerg": b.value.as_u64(), "confirmations": confirmations, "added": new }));
                // a box confirmed on an earlier sync stays confirmed when this proof happens to be against a block above
                // the agreed tip: it was proven under one then and is still unspent now
                if confirmed_at.is_some() { say!("  {id}  {} nanoERG, proven unspent at {height}, confirmations {confirmations}{}", b.value.as_u64(), if new { " (added)" } else { "" }); }
                else { say!("  {id}  {} nanoERG, seen unspent at {height}, above the tip the peers agree on ({agreed}): one source, confirmations {confirmations}{}", b.value.as_u64(), if new { " (added)" } else { "" }); }
            }
        }
    }
    refresh
}

/// wallet receive | status, and the second half of restore: prove the wallet's boxes against the agreed chain and
/// say what became of the payments it sent. The steps, in order: what to ask about (payments held from a node's pool
/// with `--pools`, what the nodes list with `--reconcile`); one proof of all of it (`proof_from`); then the
/// wallet's own payments (`settle_sent`), payments held for it (`settle_held`) and its boxes (`settle_boxes`) are
/// each read off that proof.
fn cmd_sync(c: &Ctx, o: Open) -> Ended {
    let Ctx { args, state, node, chain, enc, .. } = c;
    let sub = c.sub;
    let Open { mut w, addr, scanning, my_trees, addr_strs, .. } = o;
    if node.is_empty() || chain.is_empty() { return Err(crate::out::Fail::new(2, "usage: tachys wallet receive|status|restore --state DIR --node URL[,URL...] [--ids ID,...] [--reconcile] [--mnemonic-file FILE]  (run `tachys tips --state DIR` first)".to_string())) }
    let nodes: Vec<String> = crate::nodes::each(node);
    let mut cost = RestCost::default();
    let mut tracked: Vec<String> = w.boxes.keys().cloned().collect();
    let mut ids: Vec<String> = match sub {
        "receive" => arg(args, "--ids", String::new())?.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        "status" | "restore" | "watch" => {
            // boxes we created ourselves (change from our own payments) are looked for without asking anyone
            let mut v = tracked.clone();
            v.extend(w.expect.iter().cloned());
            // boxes undone by a reorganisation are asked about until they are back (or seen no more for long: the last 64)
            v.extend(w.undone.iter().filter(|u| !v.contains(u)).cloned().collect::<Vec<_>>());
            v
        }
        _ => { return Err(crate::out::Fail::new(2, "usage: tachys wallet new|restore|receive|status|send ...".to_string())) }
    };
    // every box a waiting payment creates is asked about too: any one of them existing shows it is in the chain
    for p in &w.pending { for (id, _) in outputs_of(p, &my_trees) { if !ids.contains(&id) { ids.push(id); } } }
    if sub == "receive" && args.iter().any(|a| a == "--pools") { take_from_pools(c, &mut w, &my_trees, &nodes, &ids, &mut cost); }
    // payments still waiting, and ones given up on (they may yet be included): the boxes they spend and one they create.
    // On every sync, `receive` too: each such payment is settled by this proof, and a coin of it not asked about would
    // read as gone
    let watched = w.pending.iter().map(|p| (&p.inputs, &p.watch)).chain(w.done.iter().filter(|d| d.result == "not included").map(|d| (&d.inputs, &d.watch)));
    for id in watched.flat_map(|(inputs, watch)| inputs.iter().chain(std::iter::once(watch))) { if !id.is_empty() && !ids.contains(id) { ids.push(id.clone()); } }
    // payments this wallet holds that someone else made to it (`wallet promise`, or fetched from a node's pool with
    // `--pools`): the boxes each promises and the coins it spends are asked about too, so that one whose coin is gone
    // is known to be dead (void) and not waited for
    let promises = held_promises(state, &w);
    // the boxes promised by a payment held with its coins proven unspent and its signatures verified when it was taken
    // (`--pools`); one handed over (`wallet promise`) was checked for nothing, and a file from before says nothing
    let proven_held: Vec<String> = promises.iter().filter(|p| p.1["proven"] == true).flat_map(|p| p.3.iter().cloned()).collect();
    for (_, _, inputs, outs) in &promises { for id in inputs.iter().chain(outs.iter()) { if !ids.contains(id) { ids.push(id.clone()); } } }
    // `refresh`: an earlier sync saw the sign of another wallet of this phrase at work (a box gone that no payment of
    // ours spent, or a payment whose outcome the proof could not tell), so the nodes are asked again what the addresses hold
    let reconcile = sub == "restore" || (sub == "status" && (args.iter().any(|a| a == "--reconcile") || w.refresh));
    if reconcile && w.refresh && !args.iter().any(|a| a == "--reconcile") { say!("looking again for this wallet's boxes: something else has spent or moved its coins since the last sync"); }
    // every node given is asked what it lists for each address, and the union is taken
    let (all_listed, any_listed) = if reconcile { listed_at(&nodes, &addr_strs, &mut ids, &mut cost) } else { (false, false) };
    crate::out::set("listed_by_all", reconcile.then_some(all_listed));
    // nothing known and nobody to ask is not "a balance of nothing"
    if ids.is_empty() && reconcile && !any_listed { return Err("no node answered what this wallet's addresses hold".into()) }
    if ids.is_empty() {
        // a restored wallet that every node says is empty at all twenty addresses keeps the first
        if scanning && all_listed {
            w.scan_addresses = false;
            let keep = (w.addresses.unwrap_or(1) as usize).clamp(1, addr_strs.len());
            w.addresses = Some(keep as u64); w.address_list = Some(addr_strs.iter().take(keep).cloned().collect());
            c.save(&w)?;
        }
        crate::out::set("address", enc.address_to_str(&addr)); crate::out::set("addresses", &addr_strs); crate::out::set("nanoerg", 0); crate::out::set("boxes", 0); crate::out::set("tokens", serde_json::json!([])); say!("address {}\nno boxes tracked; balance 0 ERG", enc.address_to_str(&addr)); return Ok(())
    }
    let mut chain = chain.clone();
    // the tip the peers agreed on: a proof at or below it settles (below); one above it is only seen
    let agreed = agreed_height(state);
    let proven = proof_from(&nodes, state, &mut chain, agreed, &ids, &mut cost);
    let Some((height, found)) = proven else { say!("NOT PROVEN by any node given"); return Err(crate::out::Fail::said(1)) };
    // A box counts as confirmed once it has been proven at or below the tip the peers agreed on. A proof against a
    // newer header rests on that header's one source and its own work: seen, not confirmed.
    // The same goes for what is taken away. A box is dropped, and a payment called confirmed or failed, only on a proof
    // at or below the agreed tip; a proof above it can add what it shows (marked as seen) and settles nothing.
    let settled = height <= agreed;
    let shown = Shown { ids: &ids, found: &found, height, agreed, settled, chain: &chain };
    let mut books = Books { expect: w.expect.clone(), void: std::mem::take(&mut w.void), undone: std::mem::take(&mut w.undone) };
    let sent = settle_sent(c, &mut w, &shown, &nodes, &my_trees, &mut books, &mut cost);
    settle_held(c, &shown, &promises, &mut books, &mut cost);
    let refresh = settle_boxes(c, &mut w, &shown, &sent, &mut books, &my_trees, &proven_held) | sent.refresh;
    let boxes = &w.boxes;
    tracked = boxes.keys().cloned().collect();
    let total: u64 = boxes.values().fold(0u64, |a, b| a.saturating_add(b.value));
    // a restored wallet has now been looked over at all twenty addresses: keep those up to the last one that holds a box
    let in_use: Option<usize> = (scanning && all_listed).then(|| {
        let trees: Vec<_> = boxes.values().filter_map(|b| unhex(&b.bytes)).filter_map(|b| ErgoBox::sigma_parse_bytes(&b).ok()).map(|b| b.ergo_tree).collect();
        my_trees.iter().rposition(|t| trees.contains(t)).map_or(1, |i| i + 1)
    });
    let Books { expect, mut undone, mut void } = books;
    w.expect = expect;
    let keep_last = |v: &mut Vec<String>| { let n = v.len(); if n > 64 { v.drain(..n - 64); } };
    keep_last(&mut undone); keep_last(&mut void);
    (w.undone, w.void) = (undone, void);
    // asked every node and got an answer from each: the wallet's boxes are up to date, whoever moved them
    if reconcile && all_listed { w.refresh = false; }
    // (a sync that has just asked every node already holds what they list: nothing more to look for)
    if refresh && !(reconcile && all_listed) { w.refresh = true; say!("another wallet of this phrase may be in use, or a payment's outcome is unclear: the next sync asks the nodes again what this wallet's addresses hold"); }
    crate::out::set("refresh_due", w.refresh);
    if let Some(n) = in_use.map(|n| n.max(w.addresses.unwrap_or(1) as usize).min(addr_strs.len())) {
        w.scan_addresses = false;
        w.addresses = Some(n as u64);
        w.address_list = Some(addr_strs.iter().take(n).cloned().collect());
        if n > 1 { say!("addresses: {n} of this phrase hold coins or come before one that does; all {n} are kept"); }
    }
    c.save(&w)?;
    crate::out::set("address", enc.address_to_str(&addr)); crate::out::set("addresses", &addr_strs); crate::out::set("nanoerg", total); crate::out::set("boxes", tracked.len());
    // of that total, what is in boxes confirmed under an agreed tip (the rest is seen in a block one node has shown, and is not spent)
    crate::out::set("confirmed_nanoerg", w.boxes.values().filter(|b| b.confirmed_at.is_some()).fold(0u64, |a, b| a.saturating_add(b.value)));
    crate::out::set("tokens", w.tokens().iter().map(|(id, amount)| serde_json::json!({ "id": id, "amount": amount })).collect::<Vec<_>>());
    crate::out::set("proven_at", height); crate::out::set("agreed_height", agreed); crate::out::set("settled", settled); crate::out::set("expecting", &w.expect);
    crate::out::set("cost", serde_json::json!({ "requests": cost.requests, "sent": cost.sent, "received": cost.received }));
    say!("address {}\nbalance {} ERG in {} boxes, proven at height {height}\ncost: {} requests, {} B sent, {} B received", enc.address_to_str(&addr), crate::util::erg(total), tracked.len(), cost.requests, cost.sent, cost.received);
    if height > agreed { say!("note: height {height} is above the tip the peers agree on ({agreed}); the headers between came from this node alone"); }
    Ok(())
}

/// What became of a payment this wallet sent, from one proof at or below the agreed tip (`settled`; a proof above it
/// settles nothing):
///  - a box it creates exists (`created`): it is in the chain;
///  - a box it spends is gone (`input_gone`) and none it creates is there: "coins spent". The boxes it created may
///    have been spent already (a payee who moves the coins on, a miner who collects the fee, another wallet of the
///    same phrase spending the change), or another transaction may have spent its coins. A proof of what is unspent
///    now cannot tell these apart, and this wallet does not assume it is the only one holding its keys; saying
///    "failed" would invite paying twice. A node may be asked what it knows (`node_says`), as its word;
///  - neither, and `gave_up` (enough blocks since first looked): not included; its coins count as ours again (if it
///    is included later after all, that is noticed and it becomes confirmed);
///  - otherwise `None`: still waiting, and it is sent again in case the node dropped it.
pub(crate) fn payment_outcome(settled: bool, created: bool, input_gone: bool, gave_up: bool) -> Option<&'static str> {
    if !settled { None }
    else if created { Some("confirmed") }
    else if input_gone { Some("coins spent") }
    else if gave_up { Some("not included") } else { None }
}

/// What the first indexed node that answers says of a transaction: "in a block at height H", or that it does not
/// know it. `None` if no node answered. Nothing is decided on this.
fn node_says(nodes: &[String], tx: &str) -> Option<String> {
    let mut unknown = false;
    for n in nodes.iter().filter(|n| !n.starts_with("p2p://")) {
        match http().get(&format!("{n}/blockchain/transaction/byId/{tx}")).call() {
            Ok(r) => {
                let v: serde_json::Value = serde_json::from_str(&r.into_string().unwrap_or_default()).unwrap_or(serde_json::Value::Null);
                if v["id"].as_str() == Some(tx) { if let Some(h) = v["inclusionHeight"].as_u64() { return Some(format!("it is in a block, at height {h}")) } }
            }
            // one node not knowing it is not every node not knowing it: the next is asked, and this is said only if none does
            Err(ureq::Error::Status(404, _)) => unknown = true,
            Err(_) => {}
        }
    }
    unknown.then(|| "none asked knows such a transaction in a block".to_string())
}

/// The boxes a remembered payment creates, from its signed transaction: each id, and whether it pays this wallet.
fn outputs_of(p: &Pending, my_trees: &[ErgoTree]) -> Vec<(String, bool)> {
    let mine: Vec<String> = my_trees.iter().filter_map(|t| t.sigma_serialize_bytes().ok()).map(|b| hex(&b)).collect();
    serde_json::from_str::<serde_json::Value>(&p.json).ok().and_then(|j| j["outputs"].as_array().cloned()).unwrap_or_default().iter()
        .filter_map(|o| Some((o["boxId"].as_str()?.to_string(), o["ergoTree"].as_str().is_some_and(|t| mine.iter().any(|m| m == t))))).collect()
}

/// Remember a payment this wallet sent, so later syncs can say what became of it: every box it spends, one box it
/// creates (if that box exists, the payment is in the chain), and the signed transaction to send again if needed.
pub(crate) fn note_pending(w: &mut WalletFile, tx: &ergo_lib::chain::transaction::Transaction, json: &str) {
    w.pending.push(Pending { tx: String::from(tx.id()), inputs: tx.inputs.iter().map(|i| String::from(i.box_id)).collect(),
        watch: tx.outputs.iter().next().map(|o| String::from(o.box_id())).unwrap_or_default(), json: json.to_string(), ..Default::default() });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ergo_lib::chain::ergo_box::box_builder::ErgoBoxCandidateBuilder;
    use ergo_lib::chain::transaction::Transaction;
    use ergotree_ir::chain::ergo_box::box_value::BoxValue;
    use ergotree_ir::chain::tx_id::TxId;
    use sigma_ser::ScorexSerializable;

    // the BIP-39 test vector for sixteen zero bytes of entropy
    const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    const ERG: u64 = 1_000_000_000;
    const FEE: u64 = 1_100_000;

    fn address(i: u64) -> Address { Address::P2Pk(key_at(PHRASE, i).unwrap().public_image()) }
    fn text(a: &Address) -> String { AddressEncoder::new(NetworkPrefix::Mainnet).address_to_str(a) }

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
        for _ in 0..10 { chain.extend_from_slice(&(bytes.len() as u16).to_be_bytes()); chain.extend_from_slice(&bytes); }
        std::fs::write(format!("{dir}/agreed-chain.bin"), chain).unwrap();
        let me = address(0);
        let candidate = ErgoBoxCandidateBuilder::new(BoxValue::try_from(10 * ERG).unwrap(), me.script().unwrap(), header.height - 100).build().unwrap();
        let b = ErgoBox::from_box_candidate(&candidate, TxId::zero(), 0).unwrap();
        let mut boxes = serde_json::Map::new();
        boxes.insert(String::from(b.box_id()), serde_json::json!({ "value": 10 * ERG, "confirmed_at": 1, "bytes": hex(&b.sigma_serialize_bytes().unwrap()) }));
        let w = serde_json::json!({ "name": "test", "address": text(&me), "address_list": [text(&me)], "birthday": 0, "mnemonic": PHRASE, "boxes": boxes });
        std::fs::write(format!("{dir}/wallet.json"), w.to_string()).unwrap();
        dir
    }

    /// `tachys wallet REST --state STATE`, as the command line would run it, short of printing a refusal and stopping.
    fn run(state: &str, rest: &[&str]) -> Result<(), String> {
        let args: Vec<String> = ["tachys", "wallet"].iter().chain(rest).chain(&["--state", state]).map(|s| s.to_string()).collect();
        let c = Ctx::new(&args).map_err(|f| f.text().to_string())?;
        let o = open(&c).map_err(|f| f.text().to_string())?;
        (if c.sub == "request" { cmd_request(&c, o) } else { cmd_send(&c, o) }).map_err(|f| f.text().to_string())
    }
    fn signed(state: &str) -> Option<Transaction> { serde_json::from_str(&std::fs::read_to_string(format!("{state}/last-tx.json")).ok()?).ok() }

    fn token(n: u8, amount: u64) -> Token { Token { token_id: TokenId::from(ergo_chain_types::Digest32::from([n; 32])), amount: TokenAmount::try_from(amount).unwrap() } }
    fn candidate(value: u64, tree: &ErgoTree, tokens: &[Token]) -> ErgoBoxCandidate {
        let mut b = ErgoBoxCandidateBuilder::new(BoxValue::try_from(value).unwrap(), tree.clone(), 1);
        for t in tokens { b.add_token(*t); }
        b.build().unwrap()
    }
    fn boxed(value: u64, tree: &ErgoTree, tokens: &[Token], index: u16) -> ErgoBox { ErgoBox::from_box_candidate(&candidate(value, tree, tokens), TxId::zero(), index).unwrap() }
    fn fee_script() -> ErgoTree { ErgoTree::sigma_parse_bytes(&unhex(&fee_tree_hex()).unwrap()).unwrap() }
    fn id_of(t: Token) -> String { String::from(t.token_id) }

    #[test]
    fn what_leaves_the_wallet_is_what_its_boxes_put_in_less_what_comes_back() {
        let enc = AddressEncoder::new(NetworkPrefix::Mainnet);
        let (mine, payee, other) = ([address(0).script().unwrap(), address(1).script().unwrap()], address(5), address(6));
        // two boxes of ours at two of our addresses go in, with 100 of one token and 7 of another
        let spent = [boxed(10 * ERG, &mine[0], &[token(1, 100)], 0), boxed(3 * ERG, &mine[1], &[token(2, 7)], 1)];
        // 2 ERG and 40 of the first token to a payee, the fee, and the rest back to our first address, less 2 of the second token (burned)
        let outs = [candidate(2 * ERG, &payee.script().unwrap(), &[token(1, 40)]), candidate(FEE, &fee_script(), &[]), candidate(11 * ERG - FEE, &mine[0], &[token(1, 60), token(2, 5)])];
        let (nano, tokens, payees) = leaves_wallet(&spent, &outs, &mine, None, &enc);
        assert_eq!(nano, 2 * ERG + FEE, "what the payee gets and the fee");
        assert_eq!(tokens, [(id_of(token(1, 1)), 40), (id_of(token(2, 1)), 2)].into_iter().collect(), "tokens paid and tokens burned both leave");
        assert_eq!(payees, vec![text(&payee)], "the fee is not a payee, nor is our own address");
        // nothing but change: only the fee leaves, no token and no payee
        let (nano, tokens, payees) = leaves_wallet(&spent, &[candidate(FEE, &fee_script(), &[]), candidate(13 * ERG - FEE, &mine[1], &[token(1, 100), token(2, 7)])], &mine, None, &enc);
        assert_eq!((nano, tokens.len(), payees.len()), (FEE, 0, 0));
        // a box of someone else's among those spent puts in nothing of ours, and what goes back to them is a payee's
        let theirs = boxed(4 * ERG, &other.script().unwrap(), &[token(3, 9)], 2);
        let with_theirs = [spent[0].clone(), theirs];
        let outs = [candidate(4 * ERG, &other.script().unwrap(), &[token(3, 9)]), candidate(FEE, &fee_script(), &[]), candidate(10 * ERG - FEE, &mine[0], &[token(1, 100)])];
        let (nano, tokens, payees) = leaves_wallet(&with_theirs, &outs, &mine, None, &enc);
        assert_eq!((nano, tokens.len(), payees), (FEE, 0, vec![text(&other)]));
        // each payee is named once, however many boxes it is paid in
        let outs = [candidate(ERG, &payee.script().unwrap(), &[]), candidate(ERG, &payee.script().unwrap(), &[]), candidate(ERG, &other.script().unwrap(), &[]), candidate(10 * ERG, &mine[0], &[token(1, 100), token(2, 7)])];
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
        let told = |at: i32| { let mut e = ContextExtension::empty(); e.values.insert(0, Constant::from(at)); (offer.box_id(), e) };
        // the payee, the offer put back with less ERG and 3 more of the token, the fee, our change
        let outs = |back: u64, also: Option<u64>| {
            let mut o = vec![candidate(ERG, &payee.script().unwrap(), &[]), candidate(back, &offer_script, &[token(4, 13)]), candidate(FEE, &fee_script(), &[]), candidate(1_000_000, &mine[0], &[token(4, 47)])];
            if let Some(v) = also { o.push(candidate(v, &offer_script, &[])); }
            o
        };
        let (nano, tokens, payees) = leaves_wallet(&spent, &outs(4 * ERG, None), &mine, Some(&told(1)), &enc);
        assert_eq!(nano, ERG / 2 - 1_000_000, "all the ERG we put in but the change");
        assert_eq!(tokens, [(id_of(token(4, 1)), 3)].into_iter().collect(), "the token units the fee cost still leave");
        assert_eq!(payees, vec![text(&payee)], "the offer put back is the fee's other half, not a payee");
        let under_offer = text(&Address::recreate_from_ergo_tree(&offer_script).unwrap());
        // a second box under the offer's script is a payee
        assert_eq!(leaves_wallet(&spent, &outs(4 * ERG, Some(ERG)), &mine, Some(&told(1)), &enc).2, vec![text(&payee), under_offer.clone()]);
        // so is the box put back if it holds more ERG than the offer had: that ERG would be ours
        assert_eq!(leaves_wallet(&spent, &outs(5 * ERG + 1, None), &mine, Some(&told(1)), &enc).2, vec![text(&payee), under_offer.clone()]);
        // and a box under that script the contract is not told is its own, or with no offer spent at all
        assert_eq!(leaves_wallet(&spent, &outs(4 * ERG, None), &mine, Some(&told(0)), &enc).2, vec![text(&payee), under_offer.clone()]);
        assert_eq!(leaves_wallet(&spent[..1], &outs(4 * ERG, None), &mine, Some(&told(1)), &enc).2, vec![text(&payee), under_offer.clone()]);
        assert_eq!(leaves_wallet(&spent, &outs(4 * ERG, None), &mine, None, &enc).2, vec![text(&payee), under_offer]);
    }

    #[test]
    fn a_payment_pays_the_payee_and_the_fee_and_returns_the_rest() {
        let st = state("send");
        let (me, payee) = (address(0).script().unwrap(), address(1));
        run(&st, &["send", "--to", &text(&payee), "--nanoerg", "2000000000"]).unwrap();
        let tx = signed(&st).expect("a signed transaction is written");
        assert_eq!(tx.inputs.len(), 1);
        let to = |tree: &ErgoTree| -> Vec<u64> { tx.outputs.iter().filter(|o| &o.ergo_tree == tree).map(|o| *o.value.as_u64()).collect() };
        assert_eq!(to(&payee.script().unwrap()), vec![2 * ERG], "the payee gets what was asked");
        assert_eq!(to(&me), vec![8 * ERG - FEE], "the rest comes back");
        let fee: Vec<u64> = tx.outputs.iter().filter(|o| o.ergo_tree.sigma_serialize_bytes().map(|b| hex(&b)).unwrap_or_default() == fee_tree_hex()).map(|o| *o.value.as_u64()).collect();
        assert_eq!(fee, vec![FEE]);
        assert_eq!(tx.outputs.len(), 3, "and nothing else is paid");
        // signed, not sent: the wallet still counts its box
        let w = crate::walletfile::parse(&std::fs::read_to_string(format!("{st}/wallet.json")).unwrap()).unwrap();
        assert_eq!(tracked_count(&w), 1);
        let _ = std::fs::remove_dir_all(&st);
    }

    #[test]
    fn a_refusal_is_handed_back_and_nothing_is_signed() {
        let st = state("refusals");
        let payee = text(&address(1));
        let refused = |rest: &[&str]| run(&st, rest).expect_err("refused");
        assert!(refused(&["send", "--to", "nonsense", "--nanoerg", "1000000000"]).contains("--to is not a"));
        assert!(refused(&["send", "--to", &payee, "--nanoerg", "99000000000"]).contains("box selection"));
        assert!(refused(&["send", "--to", &payee, "--nanoerg", "1000000000", "--pay", "nonsense:5"]).contains("--pay wants"));
        assert!(refused(&["send", "--to", &payee, "--nanoerg", "1000000000", "--also-token", "nonsense:0"]).contains("--also-token wants"));
        assert!(refused(&["send", "--to", &payee, "--nanoerg", "1000000000", "--babel", "nonsense"]).contains("--babel wants"));
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
            let args: Vec<String> = ["tachys", "wallet", "send", "--state", &st, "--to", &payee, "--nanoerg", "1000000000", last].iter().map(|s| s.to_string()).collect();
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
            assert!(c(&st).read().unwrap_err().contains("was not touched"), "{damaged:?}");
        }
        std::fs::remove_file(format!("{st}/wallet.json")).unwrap();
        assert_eq!(c(&st).read(), Ok(None), "only a missing file is no wallet");
        let _ = std::fs::remove_dir_all(&st);
    }

    #[test]
    fn the_boxes_a_payment_creates_are_read_from_what_was_signed() {
        let st = state("outputs");
        let (me, payee) = (address(0).script().unwrap(), address(1));
        run(&st, &["send", "--to", &text(&payee), "--nanoerg", "2000000000"]).unwrap();
        let tx = signed(&st).unwrap();
        let mut w = WalletFile::default();
        note_pending(&mut w, &tx, &std::fs::read_to_string(format!("{st}/last-tx.json")).unwrap());
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
        let big = (1u16..).map(|i| ErgoBox::from_box_candidate(&ErgoBoxCandidateBuilder::new(BoxValue::try_from(50 * ERG).unwrap(), me.script().unwrap(), 1).build().unwrap(), TxId::zero(), i).unwrap()).find(|b| String::from(b.box_id()) > small).unwrap();
        w.boxes.insert(String::from(big.box_id()), TrackedBox { value: 50 * ERG, confirmed_at: Some(1), bytes: hex(&big.sigma_serialize_bytes().unwrap()), ..Default::default() });
        std::fs::write(&path, crate::walletfile::text(&w).unwrap()).unwrap();
        let spends = |st: &str| -> Vec<String> { signed(st).unwrap().inputs.iter().map(|i| String::from(i.box_id)).collect() };
        // with nothing given up, the large box alone pays
        run(&st, &["send", "--to", &payee, "--nanoerg", "1000000000"]).unwrap();
        assert_eq!(spends(&st), vec![String::from(big.box_id())]);
        // an earlier payment spent the small box, was never included, and was given up: its signed transaction is still
        // out there and still valid. The next payment must spend that box too, so that only one of the two can confirm
        w.done.push(Done { tx: "aa".repeat(32), inputs: vec![small.clone()], result: "not included".into(), ..Default::default() });
        std::fs::write(&path, crate::walletfile::text(&w).unwrap()).unwrap();
        run(&st, &["send", "--to", &payee, "--nanoerg", "1000000000"]).unwrap();
        let ins = spends(&st);
        assert!(ins.contains(&small), "the coin of the payment given up is spent: {ins:?}");
        // and nothing is lost by it: what went in comes back, less the payment and the fee
        let tx = signed(&st).unwrap();
        let back: u64 = tx.outputs.iter().filter(|o| o.ergo_tree == me.script().unwrap()).map(|o| *o.value.as_u64()).sum();
        assert_eq!(back, 60 * ERG - ERG - FEE);
        let _ = std::fs::remove_dir_all(&st);
    }

    #[test]
    fn a_box_only_one_node_has_shown_is_not_spent() {
        let st = state("seen");
        let path = format!("{st}/wallet.json");
        let mut w = crate::walletfile::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // the wallet's one box, as a proof above the agreed tip leaves it: tracked, and not confirmed
        for b in w.boxes.values_mut() { b.confirmed_at = None; }
        std::fs::write(&path, crate::walletfile::text(&w).unwrap()).unwrap();
        let e = run(&st, &["send", "--to", &text(&address(1)), "--nanoerg", "1000000000"]).unwrap_err();
        assert!(e.contains("confirmed under a tip the peers agree on"), "{e}");
        assert!(signed(&st).is_none());
        let _ = std::fs::remove_dir_all(&st);
    }

    #[test]
    fn a_spending_policy_holds_what_send_signs() {
        let st = state("policy");
        let (payee, stranger) = (text(&address(1)), text(&address(2)));
        std::fs::write(policy::path(&st), serde_json::json!({ "max_per_payment_nanoerg": ERG / 2, "max_per_day_nanoerg": ERG, "allowed_payees": [payee] }).to_string()).unwrap();
        let send = |to: &str, nano: u64| run(&st, &["send", "--to", to, "--nanoerg", &nano.to_string()]);
        assert!(send(&payee, ERG / 10).unwrap_err().contains("no payments log"), "a policy with no log to count in allows nothing");
        std::fs::write(policy::log_path(&st), "").unwrap();
        assert!(send(&payee, ERG).unwrap_err().contains("over the per-payment limit"));
        assert!(send(&payee, ERG / 2).unwrap_err().contains("over the per-payment limit"), "the fee counts");
        assert!(send(&stranger, ERG / 10).unwrap_err().contains("not on the policy's list of payees"));
        assert!(signed(&st).is_none(), "nothing refused was signed");
        assert_eq!(std::fs::read_to_string(policy::log_path(&st)).unwrap(), "", "and nothing refused was counted");
        send(&payee, ERG / 10).unwrap();
        assert!(signed(&st).is_some());
        let log = std::fs::read_to_string(policy::log_path(&st)).unwrap();
        let first: serde_json::Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
        assert_eq!(first["nanoerg"].as_u64(), Some(ERG / 10 + FEE), "what left the wallet, fee included, is on the log");
        // the day's limit counts what was signed, sent or not
        send(&payee, ERG / 2 - FEE).unwrap();
        assert!(send(&payee, ERG / 2 - FEE).unwrap_err().contains("over the daily limit"));
        let _ = std::fs::remove_dir_all(&st);
    }
}


//! Commands for programs: `recipes`, `setup`, `address`, `balance`, `pay`, `accept`, `wait`, `history`, each answering
//! with one JSON object.

use crate::network::{net, private_net};
use crate::policy;
use crate::util::arg;
use crate::walletfile::WalletFile;

/// What another command answered: its `--json` object (and nothing else is read as an answer), whether it ended
/// with success, and its sentences, which are for people.
pub(crate) struct Ran { pub(crate) json: serde_json::Value, pub(crate) ok: bool, pub(crate) said: String }
impl Ran {
    /// Why it failed, in its own words.
    fn error(&self) -> String { self.json["error"].as_str().map(String::from).unwrap_or_else(|| self.said.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim().to_string()) }
}
/// Runs another command of this wallet, in this process, as a call of its own inside this one: its JSON answer,
/// its sentences kept apart. `ok` is true only if it ended with code 0 and its object says so too. (It used to run
/// the program again; a library has no program to run.)
pub(crate) fn self_run(args: &[&str]) -> Ran {
    let full: Vec<String> = std::iter::once("tachys").chain(args.iter().copied()).chain(["--json"]).map(String::from).collect();
    crate::out::begin_inner(&full);
    let ended = tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(crate::dispatch(&full)));
    let code = match ended { Ok(()) => 0, Err(f) => { if let Some(m) = f.msg { complain!("{m}"); } f.code } };
    let (json, said) = crate::out::end_inner(code);
    Ran { ok: code == 0 && json["ok"] == true && json["v"] == crate::out::V, json, said }
}

/// A refusal: the object to print and the exit code (1 refused or failed, 2 misuse).
type No = (serde_json::Value, i32);
/// A refusal with a word a program can branch on (`code`) and the reason in words (`refused`):
///   no_wallet, no_policy, policy (a limit; will not pass by retrying), no_tip, no_node, not_proven (these three may
///   pass later), node_refused, not_yet (ask again after a block), timeout, misuse, failed.
fn no(code: &str, why: impl Into<String>) -> No { (serde_json::json!({ "ok": false, "code": code, "refused": why.into() }), if code == "misuse" { 2 } else { 1 }) }
fn refused(why: impl Into<String>) -> No { no("failed", why) }
fn misuse(why: impl Into<String>) -> No { no("misuse", why) }
fn opt<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> Result<T, No> { arg(args, name, default).map_err(|f| misuse(f.msg.unwrap_or_default())) }
/// An amount of ERG as typed (`0.02`, `12`, `1.5`) in nanoERG, read exactly as a decimal: never through a float, which
/// would round some amounts to a nanoERG either side. At most nine digits after the point; no sign, exponent or
/// spaces inside; not more than fits.
pub(crate) fn erg_to_nano(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    if whole.is_empty() && frac.is_empty() { return Err(format!("is not an amount: {s:?}")) }
    if !whole.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) { return Err(format!("is not an amount of ERG in digits (no sign or exponent): {s:?}")) }
    if frac.len() > 9 { return Err(format!("has more than nine digits after the point (a nanoERG is the smallest unit): {s:?}")) }
    let w: u64 = if whole.is_empty() { 0 } else { whole.parse().map_err(|_| format!("is more than can exist: {s:?}"))? };
    let f: u64 = format!("{frac:0<9}").parse().unwrap_or(0);
    w.checked_mul(1_000_000_000).and_then(|x| x.checked_add(f)).ok_or_else(|| format!("is more than can exist: {s:?}"))
}

/// The commands for programs and agents, by name.
pub(crate) const COMMANDS: &[&str] = &["recipes", "setup", "address", "balance", "pay", "accept", "wait", "history"];

/// Commands for programs and agents. Each prints exactly one JSON object and exits 0 (done), 1 (refused or
/// failed) or 2 (misuse). Nothing here ever prints a key. `tachys recipes` lists them with what each is for.
/// `pay` needs `<state>/policy.json` (see `policy.rs`); with no policy file, paying is off. The limits are applied
/// by `wallet send` to the transaction itself, as they are to every command that signs while the file is there,
/// and each payment is written to `<state>/payments.log` before it is signed.
pub(crate) fn agent(args: &[String]) -> crate::out::Ended {
    let (mut v, code) = match answer(args) { Ok(v) => (v, 0), Err(no) => no };
    // the version of these answers, as on every other command's (`docs/json.md`); `ok` is true exactly when the exit code is 0
    v["v"] = crate::out::V.into();
    v["ok"] = (code == 0).into();
    crate::out::own_answer();
    crate::out::write_out(&v.to_string());
    if code == 0 { Ok(()) } else { Err(crate::out::Fail::said(code)) }
}

/// What a receipt turned out to be.
struct Receipt { accepted: bool, code: &'static str, why: &'static str, confirmations: u64, first_seen: bool, nanoerg: Option<u64>, tokens: Vec<serde_json::Value>, pending: Option<bool> }
impl Receipt {
    /// Into an answer: `accepted`, `reason`, `confirmations`, and what the receipt's own box holds.
    fn write_into(&self, v: &mut serde_json::Value, receipt: &str) {
        v["accepted"] = self.accepted.into(); v["reason"] = self.why.into(); v["confirmations"] = self.confirmations.into();
        v["receipt"] = serde_json::json!({ "box": receipt, "nanoerg": self.nanoerg, "tokens": self.tokens, "first_seen": self.first_seen, "pending": self.pending });
        if !self.accepted { v["code"] = self.code.into(); v["refused"] = self.why.into(); }
    }
}

/// What every agent command works from.
struct Agent { state: String, node: String }
impl Agent {
    /// The wallet file, read as the wallet commands read it: a file this build cannot read is said to be so, not taken for no wallet.
    fn wallet(&self) -> Result<WalletFile, No> {
        let (state, path) = (&self.state, format!("{}/wallet.json", self.state));
        let Ok(text) = std::fs::read_to_string(&path) else { return Err(no("no_wallet", format!("no wallet in {state}; make one with `tachys setup --state {state}`"))) };
        let w = crate::walletfile::parse(&text).map_err(|why| refused(format!("{path} is not a wallet file this build can read: {why}")))?;
        // an address handed to a payer has to be one: of this network, and readable
        let enc = ergotree_ir::chain::address::AddressEncoder::new(if net().testnet { ergotree_ir::chain::address::NetworkPrefix::Testnet } else { ergotree_ir::chain::address::NetworkPrefix::Mainnet });
        if enc.parse_address_from_str(&w.address).is_err() { return Err(refused(format!("the wallet in {state} is not a {} wallet (its address is not a {} address); say which network with --network", net().name, net().name))) }
        Ok(w)
    }
    /// Every command that touches the chain starts from a tip the peers agree on. Returns its height.
    fn tip(&self) -> Result<u64, No> {
        let (peers, tries, quorum) = if net().name == "testnet" { ("2", "8", "2") } else { ("4", "24", "3") };
        // agreed means the command's answer says so and it ended with success: `tips` exits 1 when no tip is agreed on
        // or the chain could not be saved
        let t = self_run(&["tips", "--peers", peers, "--tries", tries, "--quorum", quorum, "--state", &self.state]);
        if !t.ok || t.json["verdict"] != "agreed" { return Err(no("no_tip", "no agreed chain tip: no network, or the peers disagree")) }
        // with no node named, usable ones are found once (`nodes.rs`); the commands run below use the list it leaves in the state directory
        if self.node == crate::nodes::AUTO { if let Err(e) = crate::nodes::find(&self.state, false) { return Err(no("no_node", format!("the tip is agreed but there is no node to ask for proofs: {e}"))) } }
        Ok(t.json["tip"]["height"].as_u64().unwrap_or(0))
    }
    /// Asks the nodes what the wallet's addresses hold and proves every box against the agreed tip: the answer of
    /// `wallet status --reconcile`. (Without the asking, a payment made to the address with no receipt handed over
    /// would never be seen.)
    fn status(&self) -> Result<Ran, No> {
        let r = self_run(&["wallet", "status", "--reconcile", "--state", &self.state, "--node", &self.node]);
        if r.ok { Ok(r) } else { Err(no("not_proven", format!("the tip is agreed but the boxes could not be proven: {}", r.error()))) }
    }
    /// What the wallet holds, as the last proof left it.
    fn summary(&self, tip_height: u64) -> Result<serde_json::Value, No> {
        let w = self.wallet()?;
        // what is confirmed under a tip the peers agree on, which is what can be spent; and beside it, what sits in boxes
        // proven only in a block one node has shown
        let seen: u64 = w.boxes.values().filter(|b| b.confirmed_at.is_none()).fold(0u64, |a, b| a.saturating_add(b.value));
        let nano = w.nanoerg().saturating_sub(seen);
        Ok(serde_json::json!({ "address": w.address, "nanoerg": nano, "unconfirmed_nanoerg": seen, "erg": format!("{:.9}", nano as f64 / 1e9), "boxes": w.boxes.values().filter(|b| b.confirmed_at.is_some()).count(),
            "tokens": w.confirmed_tokens().iter().map(|(id, amount)| serde_json::json!({ "id": id, "amount": amount })).collect::<Vec<_>>(), "tip_height": tip_height, "network": net().name }))
    }
    /// Is this receipt real and ours? Accepted only once the box is proven under the tip the peers agree on. The answer
    /// says what the box holds and whether this is the first time this wallet has seen it: a receipt shown twice, or
    /// a small box shown for a large invoice, is told apart by those and not by the wording of a reason.
    fn receipt(&self, receipt: &str) -> Receipt {
        // `--pools`: if a node holds, in its pool, the payment that makes this box, the wallet takes and holds it
        // (`wallet::pending_payment`, taken only if it is that payment). From then on its coins are proven with the box
        // each time: unspent, it is on its way (`pending: true`); one gone, it is void. Without it a box id alone could
        // not tell a late payment from a double-spent one.
        let r = self_run(&["wallet", "receive", "--state", &self.state, "--node", &self.node, "--ids", receipt, "--pools"]);
        let found = if r.ok { r.json["checked"].as_array().and_then(|a| a.iter().find(|c| c["id"] == receipt)).cloned().unwrap_or(serde_json::Value::Null) } else { serde_json::Value::Null };
        // pending: held, its coins unspent and its signatures checked (true); no node holding one, among nodes that answered
        // (false); nobody answered, or nothing to say (null)
        let answered = r.json["pools"].as_array().is_some_and(|a| a.iter().any(|p| p["id"] == receipt && p["answered"] == true));
        let pending = match found["state"].as_str() { Some("expected") => Some(true), Some("absent") if answered => Some(false), _ => None };
        let confirmations: u64 = found["confirmations"].as_u64().unwrap_or(0);
        let (accepted, code, why) = match found["state"].as_str() {
            Some("proven") if confirmations >= 1 => (true, "", if found["added"] == true { "proven unspent and pays this wallet" } else { "already in this wallet, proven unspent" }),
            Some("proven") | Some("seen") => (false, "not_yet", "seen in a block that only the node has shown; not yet under a tip the peers agree on: ask again after a block"),
            Some("other_address") => (false, "failed", "that box pays someone else"),
            Some("spent") => (false, "failed", "this wallet was paid that box and has since spent it"),
            // a payment this wallet held and could never submit: a coin it spends is gone. Nothing will make it now
            Some("void") => (false, "failed", "the payment that was to make this box can never confirm: a coin it spends is gone (spent first by another payment), or it was refused for good"),
            // the block that confirmed it left the chain: not a payment now, and perhaps one again later
            Some("undone") => (false, "not_yet", "the block that confirmed this box is no longer in the chain the peers agree on (a reorganisation): it is not a payment now. It may be included again (then it is accepted with first_seen false: the same payment); or its coins were spent elsewhere, and it never will be"),
            // the payment is held and none of its coins is gone: it is on its way
            Some("expected") => (false, "not_yet", "the payment that makes this box is held by this wallet and its coins are unspent: it is not in a block yet"),
            // nothing to go on but the box id: no node asked holds a payment that makes it
            Some("absent") => (false, "not_yet", "no such unspent box, and no node asked holds a payment that makes it: not sent, not yet seen, already dropped, or not a real receipt"),
            _ => (false, "not_proven", "could not be checked"),
        };
        let held = self.wallet().ok().and_then(|w| w.boxes.get(receipt).cloned());
        Receipt { accepted, code, why, confirmations, first_seen: found["added"] == true, pending,
            nanoerg: held.as_ref().map(|b| b.value), tokens: held.map(|b| b.assets.iter().map(|a| serde_json::json!({ "id": a.id, "amount": a.amount })).collect()).unwrap_or_default() }
    }
    /// What became of a payment this wallet sent: "waiting", "confirmed", "coins spent", "not included", or `None` if it sent no such payment.
    fn payment(&self, tx: &str) -> Result<Option<String>, No> {
        let w = self.wallet()?;
        Ok(if w.pending.iter().any(|p| p.tx == tx) { Some("waiting".into()) } else { w.done.iter().find(|d| d.tx == tx).map(|d| d.result.clone()) })
    }
}

fn answer(args: &[String]) -> Result<serde_json::Value, No> {
    let cmd = args[1].as_str();
    if cmd == "recipes" { return Ok(recipes()) }
    let a = Agent { state: opt(args, "--state", "state".to_string())?, node: opt(args, "--node", crate::nodes::AUTO.to_string())? };
    let state = &a.state;
    match cmd {
        // no network: make the wallet if there is none, and say where to pay it
        "setup" => {
            let created = !std::path::Path::new(&format!("{state}/wallet.json")).exists();
            if created { let r = self_run(&["wallet", "new", "--state", state]); if !r.ok { return Err(refused(r.error())) } }
            let w = a.wallet()?;
            Ok(serde_json::json!({ "ok": true, "created": created, "address": w.address, "network": net().name, "sealed": w.secret.is_some(), "can_pay": policy::present(state) && std::path::Path::new(&policy::log_path(state)).exists(),
                "note": if policy::present(state) { "ready: `balance` to see funds, `pay` to pay within the policy" } else { "can receive now. Paying is off until its owner writes a spending policy (docs/agents.md)" } }))
        }
        "address" => { let w = a.wallet()?; Ok(serde_json::json!({ "ok": true, "address": w.address, "addresses": w.address_list.clone().unwrap_or_else(|| vec![w.address.clone()]), "network": net().name })) }
        // no network: what this wallet sent and what became of it, as the last sync left it, and what the policy has counted
        "history" => {
            let w = a.wallet()?;
            let mut sent: Vec<serde_json::Value> = w.pending.iter().map(|p| serde_json::json!({ "tx": p.tx, "result": "waiting" })).collect();
            sent.extend(w.done.iter().map(|d| serde_json::json!({ "tx": d.tx, "result": d.result, "height": d.seen_at })));
            let log: Vec<serde_json::Value> = std::fs::read_to_string(policy::log_path(state)).unwrap_or_default().lines().rev().take(20).filter_map(|l| serde_json::from_str(l).ok()).collect();
            Ok(serde_json::json!({ "ok": true, "address": w.address, "sent": sent, "expecting": w.expect, "payments_log": log, "note": "as of the last sync; run `balance` to bring it up to date" }))
        }
        _ => {
            if private_net() && a.node == crate::nodes::AUTO { return Err(misuse("a private network has no nodes to find: give --node URL")) }
            a.wallet()?;
            online(&a, cmd, args)
        }
    }
}

/// The commands that need the network.
fn online(a: &Agent, cmd: &str, args: &[String]) -> Result<serde_json::Value, No> {
    let (state, node) = (&a.state, &a.node);
    match cmd {
        "balance" => {
            let tip = a.tip()?;
            let r = a.status()?;
            // proven: every box counted was proven unspent, and every node asked answered for every address (`complete`)
            let mut v = a.summary(tip)?; v["proven"] = serde_json::json!(true); v["complete"] = r.json["listed_by_all"].clone();
            // payments still waiting hold their coins back: say so, or the balance looks short for no reason
            v["waiting"] = serde_json::json!(a.wallet()?.pending.iter().map(|p| p.tx.clone()).collect::<Vec<_>>());
            v["settled"] = r.json["payments"].as_array().map(|p| p.iter().filter(|x| x["result"] != "waiting").cloned().collect::<Vec<_>>()).unwrap_or_default().into();
            Ok(v)
        }
        "accept" => {
            let receipt: String = opt(args, "--receipt", String::new())?;
            if receipt.len() != 64 { return Err(misuse("--receipt wants a 64-hex box id")) }
            let tip = a.tip()?;
            let r = a.receipt(&receipt);
            let mut v = a.summary(tip)?;
            r.write_into(&mut v, &receipt);
            if r.accepted { Ok(v) } else { Err((v, 1)) }
        }
        // One call instead of a loop of them: come back when a receipt is accepted with enough confirmations, or a
        // payment this wallet sent is settled, or the time is up.
        "wait" => {
            let receipt: String = opt(args, "--receipt", String::new())?;
            let tx: String = opt(args, "--tx", String::new())?;
            let want: u64 = opt(args, "--confirmations", 1)?;
            let timeout: u64 = opt(args, "--timeout", 900)?;
            if (receipt.len() == 64) == (tx.len() == 64) { return Err(misuse("usage: tachys wait --state DIR (--receipt BOXID [--confirmations N] | --tx TXID) [--timeout SECONDS]")) }
            if !tx.is_empty() && a.payment(&tx)?.is_none() { return Err(refused("this wallet has no record of sending that transaction")) }
            if want == 0 { return Err(misuse("--confirmations is at least 1")) }
            let t0 = std::time::Instant::now();
            // a block is the least that can change anything; half a block between looks, and never less than five seconds
            let pause = std::time::Duration::from_millis((net().block_interval_ms / 2).clamp(5_000, 60_000));
            let mut last = serde_json::json!({ "reason": "no agreed chain tip yet" });
            loop {
                if let Ok(tip) = a.tip() {
                    if !receipt.is_empty() {
                        let r = a.receipt(&receipt);
                        let enough = r.accepted && r.confirmations >= want;
                        last = serde_json::json!({ "tip_height": tip });
                        r.write_into(&mut last, &receipt);
                        last["accepted"] = enough.into();
                        if enough { let mut v = a.summary(tip)?; for (k, x) in last.as_object().into_iter().flatten() { v[k] = x.clone(); } v["waited_s"] = t0.elapsed().as_secs().into(); return Ok(v) }
                        if r.accepted { last["code"] = "not_yet".into(); last["refused"] = format!("proven, with {} of the {want} confirmations asked for", r.confirmations).into(); }
                        // a box that pays someone else, or that this wallet has spent, will never be a payment: no use waiting
                        if r.code == "failed" { break }
                    } else if a.status().is_ok() {
                        let result = a.payment(&tx)?.unwrap_or_default();
                        // confirmations: the agreed tip's height less the height the payment was proven at, plus one (a lower bound)
                        let at = a.wallet()?.done.iter().find(|d| d.tx == tx).map_or(0, |d| d.seen_at);
                        let confirmations = if result == "confirmed" && at > 0 { (tip + 1).saturating_sub(at) } else { 0 };
                        last = serde_json::json!({ "tx": tx, "result": result, "confirmations": confirmations, "tip_height": tip });
                        if result == "confirmed" && confirmations >= want { return Ok(serde_json::json!({ "ok": true, "tx": tx, "result": result, "confirmations": confirmations, "tip_height": tip, "waited_s": t0.elapsed().as_secs() })) }
                        if result == "confirmed" { last["code"] = "not_yet".into(); last["refused"] = format!("in a block, with {confirmations} of the {want} confirmations asked for").into(); }
                        else if result != "waiting" { last["code"] = "failed".into(); last["refused"] = format!("the payment ended as: {result}").into(); break }
                    }
                }
                if t0.elapsed().as_secs() + pause.as_secs() > timeout { last["timed_out"] = true.into(); last["code"] = "timeout".into(); if last.get("refused").is_none() { last["refused"] = "not so within the time given".into(); } break }
                std::thread::sleep(pause);
            }
            last["waited_s"] = t0.elapsed().as_secs().into();
            Err((last, 1))
        }
        "pay" => {
            let to: String = opt(args, "--to", String::new())?;
            let nano: u64 = { let n: u64 = opt(args, "--nanoerg", 0)?; if n > 0 { n } else { erg_to_nano(&opt(args, "--erg", "0".to_string())?).map_err(|e| misuse(format!("--erg {e}")))? } };
            let token: String = opt(args, "--token", String::new())?;
            let token_amount: u64 = opt(args, "--token-amount", 0)?;
            let purpose: String = opt(args, "--purpose", String::new())?;
            let dry = args.iter().any(|a| a == "--dry-run");
            if to.is_empty() || (nano == 0 && token.is_empty()) { return Err(misuse("usage: tachys pay --state DIR --to ADDRESS (--erg X | --nanoerg N | --token ID --token-amount RAW) [--purpose TEXT] [--dry-run]")) }
            // The policy. `wallet send` holds every signature to it and writes the payment to the log before signing;
            // it is checked here first as well, so that a refusal costs no network and a dry run can answer.
            if !policy::present(state) { return Err(no("no_policy", format!("paying is off: there is no policy file at {}", policy::path(state)))) }
            if !token.is_empty() && token_amount == 0 { return Err(misuse("--token needs --token-amount RAW above zero")) }
            let fee = 1_100_000u64;
            let spend = if token.is_empty() { nano } else { 1_000_000 };
            let mut ask = policy::Ask { nano: spend.saturating_add(fee), payees: vec![to.clone()], purpose: purpose.clone(), what: "pay".into(), ..Default::default() };
            if !token.is_empty() { ask.tokens.insert(token.clone(), token_amount); }
            policy::reserve(state, &ask, true).map_err(|e| no("policy", e))?;
            a.tip()?;
            a.status()?;
            let amount_s = nano.to_string(); let ta = token_amount.to_string();
            let mut cmd: Vec<&str> = vec!["wallet", "send", "--state", state, "--node", node, "--to", &to, "--purpose", &purpose];
            if token.is_empty() { cmd.extend(["--nanoerg", amount_s.as_str()]); } else { cmd.extend(["--token", token.as_str(), "--token-amount", ta.as_str()]); }
            // A dry run builds the same transaction and stops before signing: what it reports leaving the wallet is
            // what a real payment would put on the payments log, not an estimate.
            cmd.push(if dry { "--dry-run" } else { "--broadcast" });
            let r = self_run(&cmd);
            let leaves = r.json["leaves_wallet_nanoerg"].as_u64().unwrap_or(0);
            let fee = r.json["fee_nanoerg"].as_u64().unwrap_or(fee);
            // A node was sent the payment and did not answer. It may be on its way: it is kept as waiting and must not be paid again on a guess.
            if r.json["outcome_unknown"] == true {
                return Err((serde_json::json!({ "ok": false, "code": "outcome_unknown", "outcome_unknown": true, "tx": r.json["tx"], "receipt": r.json["receipt"][0], "to": to, "refused": r.error(),
                    "next": format!("do not pay again. Run `tachys wait --state {state} --tx {}` to learn what became of it", r.json["tx"].as_str().unwrap_or("TX")) }), 1))
            }
            if !r.ok { return Err(no(if r.error().contains("spending policy") { "policy" } else if r.error().contains("node refused") { "node_refused" } else { "failed" }, r.error())) }
            if dry { return Ok(serde_json::json!({ "ok": true, "dry_run": true, "would_pay_nanoerg": leaves.saturating_sub(fee), "would_leave_wallet_nanoerg": leaves, "token": token, "token_amount": token_amount, "to": to, "fee_nanoerg": fee })) }
            // sent means the command succeeded and says a node took it
            if r.json["signed"] != true || r.json["broadcast"]["accepted"] != true { return Err(refused(r.error())) }
            Ok(serde_json::json!({ "ok": true, "tx": r.json["tx"], "receipt": r.json["receipt"][0], "to": to, "nanoerg": leaves.saturating_sub(fee), "fee_nanoerg": fee, "purpose": purpose,
                "note": "broadcast, not yet in a block; the payee can check the receipt once it confirms" }))
        }
        other => Err(misuse(format!("no such command: {other}"))),
    }
}

/// `tachys recipes`: what a program can do with this wallet, each as the one command to run. Read once; it costs
/// no network and needs no wallet. docs/agents.md says the same at more length.
fn recipes() -> serde_json::Value {
    serde_json::json!({
        "about": "tachys: a light wallet for Ergo. Each command below prints one JSON object. `ok` is true exactly when the exit code is 0. Exit 1: refused, failed, or not yet so; the object says why in `refused` and gives a `code` to branch on. Exit 2: misuse. Add `--network testnet` for testnet (the default is mainnet: real money). DIR is a directory this wallet owns; use the same one every time.",
        "codes": "no_wallet | no_policy | policy (a limit: retrying will not help) | no_tip, no_node, not_proven (the network: try again later) | not_yet (ask again after a block) | timeout | node_refused | outcome_unknown | misuse | failed",
        "units": "1 ERG = 1000000000 nanoERG. Amounts in answers are nanoERG unless the field says erg. The network fee is 1100000 nanoERG a payment, on top of what is paid.",
        "recipes": [
            { "to": "get a wallet, or find the one already there", "run": "tachys setup --state DIR", "gives": "address (give it to whoever pays you), created, can_pay, sealed", "network": false,
              "mind": "sealed:false means the seed phrase is in plain text in DIR; it is sealed only if TACHYS_PASS was set when the wallet was made" },
            { "to": "know where to be paid", "run": "tachys address --state DIR", "gives": "address", "network": false },
            { "to": "know what you hold", "run": "tachys balance --state DIR", "gives": "nanoerg, erg, tokens, unconfirmed_nanoerg (beside nanoerg, not part of it: what is in a block only one node has shown yet), waiting (payments of yours not yet in a block; their coins are held back)" },
            { "to": "check a payment would be allowed, and what it would cost, without making it", "run": "tachys pay --state DIR --to ADDRESS --erg AMOUNT --dry-run", "gives": "would_leave_wallet_nanoerg, fee_nanoerg" },
            { "to": "pay someone", "run": "tachys pay --state DIR --to ADDRESS --erg AMOUNT --purpose WHY", "gives": "tx, receipt (a box id: hand it to the payee, it is how they check they were paid)",
              "mind": "ok:true means a node took it, not that it is in a block. If the answer has outcome_unknown:true, do NOT pay again: run the next recipe with its tx." },
            { "to": "pay in a token", "run": "tachys pay --state DIR --to ADDRESS --token TOKENID --token-amount RAW_UNITS", "gives": "tx, receipt" },
            { "to": "know a payment you made is in the chain", "run": "tachys wait --state DIR --tx TX --confirmations 3 --timeout 1800", "gives": "result, confirmations. confirmed: done. not included: it was not in a block after ten; its coins count as yours again, and your next payment will spend one of them so that the two cannot both confirm, so paying again is safe. coins spent: unclear, do not pay again without checking with the payee.",
              "mind": "returns as soon as it is so; exit 1 with code timeout if not in time (15 minutes unless --timeout says otherwise)" },
            { "to": "check you were paid, given a receipt (a box id) from the payer", "run": "tachys accept --state DIR --receipt BOXID", "gives": "accepted, confirmations, receipt: { nanoerg, tokens, first_seen }",
              "mind": "accepted says the box is real, unspent and yours. YOU must check receipt.nanoerg and receipt.tokens are what you were owed, and receipt.first_seen is true: a box you have been shown before is not a new payment. Exit 1 with code not_yet is the normal answer until the payment is in a block." },
            { "to": "wait until you have been paid", "run": "tachys wait --state DIR --receipt BOXID --confirmations 3 --timeout 1800", "gives": "accepted, confirmations, receipt (as above; returns as soon as it is so)" },
            { "to": "see what you sent and what became of it", "run": "tachys history --state DIR", "gives": "sent (tx, result), payments_log", "network": false }
        ],
        "limits": "`pay` works only if the wallet's owner wrote DIR/policy.json and DIR/payments.log (per-payment and daily limits, allowed payees); a refusal names the limit. The limits bind you if the owner set the wallet up under another account than yours: then you cannot change them, read the seed phrase, or pay outside them. They are not a wall against a program running as the wallet's own user.",
        "more": "docs/agents.md says the same at more length. Every other command takes --json (docs/json.md); those are the owner's tools, and `wallet send` among them signs with no limit when there is no policy file."
    })
}

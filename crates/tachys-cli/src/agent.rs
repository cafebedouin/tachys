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
    // under `wait --timeout`, a command run from here (`tips`, over the peer protocol) is stopped at the deadline;
    // the requests to nodes the others make end by it already (`util::request_time`)
    let ended = tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(async {
        match crate::util::Deadline::left() {
            Some(left) => tokio::time::timeout(left, crate::dispatch(&full)).await.unwrap_or_else(|_| Err(crate::out::Fail::new(1, "the time given ran out".into()))),
            None => crate::dispatch(&full).await,
        }
    }));
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
        // pending: held with its coins proven unspent and its signatures verified when it was taken, or made by a payment
        // of this wallet's own, still waiting (true); no node holding one, among nodes that answered (false); nobody
        // answered, or nothing to say (null), which is also what a payment held by `wallet promise` gets: it was handed
        // over and nothing about it was checked
        // false only if a node answered and none showed such a payment: one shown but not held (its coins not yet proven
        // under the agreed tip, or its proof did not check) is not "none", and a payee told so might ask to be paid twice
        let answered = r.json["pools"].as_array().is_some_and(|a| a.iter().any(|p| p["id"] == receipt && p["answered"] == true))
            && !r.json["pools"].as_array().is_some_and(|a| a.iter().any(|p| p["id"] == receipt && p["offered"] == true));
        let (proven, own) = (found["held_proven"] == true, found["own"] == true);
        let pending = match found["state"].as_str() { Some("expected") if proven || own => Some(true), Some("absent") if answered => Some(false), _ => None };
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
            // the wallet's own payment, waiting: each sync proves its coins again, and one gone settles it
            Some("expected") if own => (false, "not_yet", "the payment that makes this box is this wallet's own, built and signed by it from coins it had proven unspent: it is not in a block yet"),
            // the payment is held and none of its coins is gone: it is on its way
            Some("expected") if proven => (false, "not_yet", "the payment that makes this box is held by this wallet, its coins proven unspent and its signatures verified: it is not in a block yet"),
            Some("expected") => (false, "not_yet", "the payment that makes this box is held by this wallet as it was handed over, nothing about it proven: it is not in a block yet"),
            // nothing to go on but the box id: no node asked holds a payment that makes it
            Some("absent") => (false, "not_yet", "no such unspent box, and no node asked holds a payment that makes it: not sent, not yet seen, already dropped, or not a real receipt"),
            _ => (false, "not_proven", "could not be checked"),
        };
        let w = self.wallet().ok();
        let held = w.as_ref().and_then(|w| w.boxes.get(receipt).cloned());
        // a box not in a block yet, whose payment this wallet holds: what that payment would make it hold
        let expected = if held.is_none() && found["state"] == "expected" { w.as_ref().and_then(|w| crate::wallet::expected_box(&self.state, w, receipt)) } else { None };
        let (nanoerg, tokens) = match (held, expected) {
            (Some(b), _) => (Some(b.value), b.assets.iter().map(|a| serde_json::json!({ "id": a.id, "amount": a.amount })).collect()),
            (None, Some(b)) => (Some(*b.value.as_u64()), b.tokens.iter().flat_map(|t| t.iter()).map(|t| serde_json::json!({ "id": String::from(t.token_id), "amount": t.amount.as_u64() })).collect()),
            (None, None) => (None, vec![]),
        };
        Receipt { accepted, code, why, confirmations, first_seen: found["added"] == true, pending, nanoerg, tokens }
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
            // every request to a node from here on ends by the deadline, and a nested `tips` is stopped at it: the answer
            // comes back about when the time given is up, not after the slowest node has finished
            let limit = std::time::Duration::from_secs(timeout);
            let _deadline = crate::util::Deadline::set(t0 + limit);
            // a block is the least that can change anything; half a block between looks, and never less than five seconds
            let pause = std::time::Duration::from_millis((net().block_interval_ms / 2).clamp(5_000, 60_000));
            let mut last = serde_json::json!({ "reason": "no agreed chain tip yet" });
            loop {
                // a tip agreed only after the time ran out is not acted on: what is asked after it would fail at once
                if let Some(tip) = a.tip().ok().filter(|_| t0.elapsed() < limit) {
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

#[cfg(test)]
mod tests {
    //! `accept` first agrees on a tip with live peers (`Agent::tip`); what it then says of a receipt is `Agent::receipt`,
    //! tested here on its own against a node on this machine.
    use super::*;
    use ergotree_ir::serialization::SigmaSerializable;
    use sigma_ser::ScorexSerializable;
    use std::io::{Read, Write};

    const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn call(args: &[&str]) -> serde_json::Value {
        let a = crate::call(&std::iter::once("tachys").chain(args.iter().copied()).chain(["--json"]).map(String::from).collect::<Vec<_>>(), &[]);
        assert_eq!(a.code, 0, "{args:?}: {} {}", a.out, a.err);
        serde_json::from_str(a.out.trim()).unwrap()
    }
    fn dir(name: &str) -> String {
        let d = std::env::temp_dir().join(format!("tachys-agent-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.to_string_lossy().to_string()
    }
    fn unhex(s: &str) -> Vec<u8> { crate::util::unhex(s).unwrap() }
    /// Ten headers stored as the agreed chain: the release's checkpoint, made recent, with `root` as its state root.
    fn chain_over(st: &str, root: &[u8]) {
        let mut h = crate::network::checkpoint_header();
        h.timestamp = crate::chain::now_ms();
        h.state_root = ergo_chain_types::ADDigest::try_from(root.to_vec()).unwrap();
        let b = h.scorex_serialize_bytes().unwrap();
        let mut chain = vec![];
        for _ in 0..10 { chain.extend_from_slice(&(b.len() as u16).to_be_bytes()); chain.extend_from_slice(&b); }
        std::fs::write(format!("{st}/agreed-chain.bin"), chain).unwrap();
        std::fs::write(format!("{st}/agreed-tip.bin"), &b).unwrap();
    }
    fn prover(held: &[Vec<u8>]) -> ergo_avltree_rust::batch_avl_prover::BatchAVLProver {
        use ergo_avltree_rust::batch_node::{AVLTree, Node, NodeHeader};
        use ergo_avltree_rust::operation::{KeyValue, Operation};
        let mut p = ergo_avltree_rust::batch_avl_prover::BatchAVLProver::new(AVLTree::new(|d| Node::LabelOnly(NodeHeader::new(Some(*d), None)), 32, None), true);
        for v in held.iter().cloned().chain(std::iter::once(vec![7u8; 60])) {
            p.perform_one_operation(&Operation::Insert(KeyValue { key: bytes::Bytes::copy_from_slice(sigma_util::hash::blake2b256_hash(&v).as_ref()), value: bytes::Bytes::from(v) })).unwrap();
        }
        p.generate_proof();
        p
    }
    fn root_of(held: &[Vec<u8>]) -> Vec<u8> { use ergo_avltree_rust::authenticated_tree_ops::AuthenticatedTreeOps; prover(held).digest().unwrap().to_vec() }
    /// A node holding `held` in its state and `pool` (a payment's id and JSON) in its pool. Returns its URL and root.
    fn node(held: Vec<Vec<u8>>, pool: (String, String)) -> (String, Vec<u8>) { node_listing(held, pool, vec![]) }
    /// The same, listing `listed` as the unspent boxes under any script asked about.
    fn node_listing(held: Vec<Vec<u8>>, pool: (String, String), listed: Vec<String>) -> (String, Vec<u8>) {
        use ergo_avltree_rust::operation::Operation;
        let root = root_of(&held);
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        std::thread::spawn(move || for c in l.incoming() {
            let Ok(mut c) = c else { continue };
            let (mut buf, mut chunk) = (vec![], [0u8; 4096]);
            let (head, body) = loop {
                match c.read(&mut chunk) { Ok(0) | Err(_) => break (String::new(), String::new()), Ok(n) => buf.extend_from_slice(&chunk[..n]) }
                let t = String::from_utf8_lossy(&buf).to_string();
                if let Some((h, b)) = t.split_once("\r\n\r\n") {
                    let want: usize = h.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
                    if b.len() >= want { break (h.to_string(), b.to_string()) }
                }
            };
            let line = head.lines().next().unwrap_or("").to_string();
            let tx: serde_json::Value = serde_json::from_str(&pool.1).unwrap();
            let (status, answer) = if line.contains("/utxo/getBoxesBinaryProof") {
                let ids: Vec<String> = serde_json::from_str(&body).unwrap_or_default();
                let mut p = prover(&held);
                for id in &ids { p.perform_one_operation(&Operation::Lookup(bytes::Bytes::from(unhex(id)))).unwrap(); }
                (200, format!("\"{}\"", crate::util::hex(&p.generate_proof())))
            } else if let Some(o) = tx["outputs"].as_array().unwrap().iter().find(|o| line.contains(&format!("/transactions/unconfirmed/outputs/byBoxId/{}", o["boxId"].as_str().unwrap()))) {
                let mut o = o.clone(); o["transactionId"] = pool.0.clone().into(); (200, o.to_string())
            } else if line.contains(&format!("/transactions/unconfirmed/byTransactionId/{}", pool.0)) { (200, pool.1.clone())
            } else if line.contains("/blockchain/box/unspent/byErgoTree") { (200, serde_json::Value::from(listed.iter().map(|b| serde_json::json!({ "boxId": b })).collect::<Vec<_>>()).to_string())
            } else { (404, r#"{"error":404}"#.to_string()) };
            let _ = c.write_all(format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len()).as_bytes());
        });
        (url, root)
    }
    fn receipt_of(state: &str, node: &str, receipt: &str) -> Receipt {
        // one call at a time, as `crate::call` holds it: the inner `wallet receive` writes its answer where a call
        // started by another test beside this one would clear it
        let _one = crate::ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
        let a = Agent { state: state.to_string(), node: node.to_string() };
        let receipt = receipt.to_string();
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        rt.block_on(async move { tokio::spawn(async move { a.receipt(&receipt) }).await.unwrap() })
    }
    fn pending(state: &str, node: &str, receipt: &str) -> (Option<bool>, &'static str) { let r = receipt_of(state, node, receipt); (r.pending, r.code) }

    /// M3: `accept` said `pending: true` for any box a held payment promises, also one held by `wallet promise`, where
    /// nothing about its coins or signatures was checked. Correct: `true` only for a payment held with its coins proven
    /// unspent and its signatures verified (`--pools`); one held by `wallet promise` is `null`.
    #[test]
    fn m3_pending_is_true_only_for_a_payment_whose_coins_were_proven() {
        // a payer with one coin of ten ERG pays two ERG to a payee, offline: a signed payment and the box it makes
        let payer = dir("m3-payer");
        let cp = crate::network::checkpoint_header();
        let me = ergotree_ir::chain::address::Address::P2Pk(ergo_lib::wallet::ext_secret_key::ExtSecretKey::derive_master(ergo_lib::wallet::mnemonic::Mnemonic::to_seed(PHRASE, "")).unwrap()
            .derive(std::str::FromStr::from_str("m/44'/429'/0'/0/0").unwrap()).unwrap().public_image());
        let coin = ergotree_ir::chain::ergo_box::ErgoBox::from_box_candidate(&ergo_lib::chain::ergo_box::box_builder::ErgoBoxCandidateBuilder::new(
            ergotree_ir::chain::ergo_box::box_value::BoxValue::try_from(10_000_000_000u64).unwrap(), me.script().unwrap(), cp.height - 100).build().unwrap(), ergotree_ir::chain::tx_id::TxId::zero(), 0).unwrap();
        let coin_bytes = coin.sigma_serialize_bytes().unwrap();
        chain_over(&payer, &root_of(std::slice::from_ref(&coin_bytes)));
        let a = ergotree_ir::chain::address::AddressEncoder::new(ergotree_ir::chain::address::NetworkPrefix::Mainnet).address_to_str(&me);
        std::fs::write(format!("{payer}/wallet.json"), serde_json::json!({ "name": "payer", "address": a, "address_list": [a], "birthday": 0, "mnemonic": PHRASE,
            "boxes": { String::from(coin.box_id()): { "value": 10_000_000_000u64, "confirmed_at": 1, "bytes": crate::util::hex(&coin_bytes) } } }).to_string()).unwrap();
        let payee = dir("m3-payee");
        call(&["wallet", "new", "--state", &payee]);
        // read from the files, not from the answers: a test beside this one that signs without `crate::call` writes its
        // fields into whatever answer is under way
        let to = serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(format!("{payee}/wallet.json")).unwrap()).unwrap()["address"].as_str().unwrap().to_string();
        let parcel = format!("{payer}/parcel.json");
        call(&["wallet", "send", "--state", &payer, "--to", &to, "--nanoerg", "2000000000", "--parcel-out", &parcel]);
        let p: serde_json::Value = serde_json::from_slice(&std::fs::read(&parcel).unwrap()).unwrap();
        let to_tree = crate::util::hex(&ergotree_ir::chain::address::AddressEncoder::unchecked_parse_address_from_str(&to).unwrap().script().unwrap().sigma_serialize_bytes().unwrap());
        let tx: serde_json::Value = serde_json::from_str(p["tx_json"].as_str().unwrap()).unwrap();
        let receipt = tx["outputs"].as_array().unwrap().iter().find(|o| o["ergoTree"] == to_tree.as_str()).expect("the payment pays the payee")["boxId"].as_str().unwrap().to_string();
        let (url, root) = node(vec![coin_bytes], (p["tx_id"].as_str().unwrap().to_string(), p["tx_json"].as_str().unwrap().to_string()));
        // the same payee wallet twice: one holds the payment by `wallet promise`, the other takes it from the node's pool
        let (promised, pooled) = (dir("m3-promised"), dir("m3-pooled"));
        for st in [&promised, &pooled] { std::fs::copy(format!("{payee}/wallet.json"), format!("{st}/wallet.json")).unwrap(); chain_over(st, &root); }
        call(&["wallet", "promise", "--state", &promised, "--tx", p["tx_bytes"].as_str().unwrap()]);
        // positive arm: taken from the pool, its coin proven unspent and its signature verified: on its way
        assert_eq!(pending(&pooled, &url, &receipt), (Some(true), "not_yet"), "held from the pool, its coins proven");
        // the lead: held by `wallet promise`, nothing about it proven
        let (said, code) = pending(&promised, &url, &receipt);
        assert_eq!(said, None, "M3: a payment held by `wallet promise`, nothing about it proven, answers pending: {said:?} ({code})");
        for st in [payer, payee, promised, pooled] { let _ = std::fs::remove_dir_all(st); }
    }

    /// A wallet of the test phrase tracking one coin of ten ERG, confirmed, over an agreed chain whose state holds that
    /// coin; it has signed a payment of two ERG to a new wallet's address, offline (`--parcel-out`): the payment is
    /// waiting and its change expected. Returns the wallet's directory, the coin's bytes, the payment (the parcel) and
    /// the payee's directory.
    fn paid(name: &str) -> (String, Vec<u8>, serde_json::Value, String) {
        let payer = dir(name);
        let cp = crate::network::checkpoint_header();
        let me = ergotree_ir::chain::address::Address::P2Pk(ergo_lib::wallet::ext_secret_key::ExtSecretKey::derive_master(ergo_lib::wallet::mnemonic::Mnemonic::to_seed(PHRASE, "")).unwrap()
            .derive(std::str::FromStr::from_str("m/44'/429'/0'/0/0").unwrap()).unwrap().public_image());
        let coin = ergotree_ir::chain::ergo_box::ErgoBox::from_box_candidate(&ergo_lib::chain::ergo_box::box_builder::ErgoBoxCandidateBuilder::new(
            ergotree_ir::chain::ergo_box::box_value::BoxValue::try_from(10_000_000_000u64).unwrap(), me.script().unwrap(), cp.height - 100).build().unwrap(), ergotree_ir::chain::tx_id::TxId::zero(), 0).unwrap();
        let coin_bytes = coin.sigma_serialize_bytes().unwrap();
        chain_over(&payer, &root_of(std::slice::from_ref(&coin_bytes)));
        let a = ergotree_ir::chain::address::AddressEncoder::new(ergotree_ir::chain::address::NetworkPrefix::Mainnet).address_to_str(&me);
        std::fs::write(format!("{payer}/wallet.json"), serde_json::json!({ "name": "payer", "address": a, "address_list": [a], "birthday": 0, "mnemonic": PHRASE,
            "boxes": { String::from(coin.box_id()): { "value": 10_000_000_000u64, "confirmed_at": 1, "bytes": crate::util::hex(&coin_bytes) } } }).to_string()).unwrap();
        let payee = dir(&format!("{name}-payee"));
        call(&["wallet", "new", "--state", &payee]);
        let to = serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(format!("{payee}/wallet.json")).unwrap()).unwrap()["address"].as_str().unwrap().to_string();
        let parcel = format!("{payer}/parcel.json");
        call(&["wallet", "send", "--state", &payer, "--to", &to, "--nanoerg", "2000000000", "--parcel-out", &parcel]);
        let p: serde_json::Value = serde_json::from_slice(&std::fs::read(&parcel).unwrap()).unwrap();
        (payer, coin_bytes, p, payee)
    }
    fn wallet_json(st: &str) -> serde_json::Value { serde_json::from_str(&std::fs::read_to_string(format!("{st}/wallet.json")).unwrap()).unwrap() }

    /// `accept` runs `wallet receive --ids RECEIPT`, which also settles the wallet's own payments. It asked about the
    /// boxes a waiting payment makes but not the coins it spends, and read a coin not asked about as gone: a payment
    /// whose coin is unspent was settled as "coins spent", and a payment given up as "not included" too. Each is asked
    /// about now on every sync, as `status` asks.
    #[test]
    fn a_receipt_asked_about_leaves_the_wallets_own_payments_as_they_are() {
        let (payer, coin, p, payee) = paid("own-waiting");
        let change = wallet_json(&payer)["expect"][0].as_str().unwrap().to_string();
        let (url, _) = node(vec![coin], (p["tx_id"].as_str().unwrap().to_string(), p["tx_json"].as_str().unwrap().to_string()));
        // the same wallet with its payment given up on: not included after ten blocks, its coin still unspent
        let gave_up = dir("own-gave-up");
        for f in ["wallet.json", "agreed-chain.bin", "agreed-tip.bin"] { std::fs::copy(format!("{payer}/{f}"), format!("{gave_up}/{f}")).unwrap(); }
        let mut w = wallet_json(&gave_up);
        let mut d = w["pending"][0].clone();
        d["result"] = "not included".into(); d["seen_at"] = 1.into();
        w["done"] = serde_json::json!([d]); w["pending"] = serde_json::json!([]); w["expect"] = serde_json::json!([]);
        std::fs::write(format!("{gave_up}/wallet.json"), w.to_string()).unwrap();
        // a receipt asked about (here the change; any box would do)
        for st in [&payer, &gave_up] { call(&["wallet", "receive", "--state", st, "--node", &url, "--ids", &change]); }
        let w = wallet_json(&payer);
        assert_eq!((w["pending"].as_array().map_or(0, |a| a.len()), w["done"].clone()), (1, serde_json::Value::Null), "a waiting payment whose coin is unspent is still waiting");
        assert_eq!(wallet_json(&gave_up)["done"][0]["result"], "not included", "a payment given up on whose coin is unspent is still not included");
        for st in [payer, gave_up, payee] { let _ = std::fs::remove_dir_all(st); }
    }

    /// A box from the wallet's own payment, still waiting (its change), answered `pending: null`, as a payment held by
    /// `wallet promise` with nothing about it checked does. It was built and signed by this wallet from coins it had
    /// proven unspent, and each sync proves them again: `pending: true`, with a reason of its own. A coin of it gone
    /// settles the payment, and the answer is `failed`.
    #[test]
    fn a_box_from_the_wallets_own_waiting_payment_is_pending() {
        let (payer, coin, p, payee) = paid("own-pending");
        let change = wallet_json(&payer)["expect"][0].as_str().unwrap().to_string();
        let pool = (p["tx_id"].as_str().unwrap().to_string(), p["tx_json"].as_str().unwrap().to_string());
        let gone = dir("own-pending-gone");
        for f in ["wallet.json", "agreed-chain.bin"] { std::fs::copy(format!("{payer}/{f}"), format!("{gone}/{f}")).unwrap(); }
        let (url, _) = node(vec![coin], pool.clone());
        let r = receipt_of(&payer, &url, &change);
        assert_eq!((r.pending, r.code), (Some(true), "not_yet"), "the wallet's own payment, its coin unspent: on its way");
        assert!(r.why.contains("this wallet's own"), "with its own reason: {}", r.why);
        // its coin spent by something else: a node whose state holds another box, the chain read against its root
        let (url, root) = node(vec![vec![9u8; 60]], pool);
        chain_over(&gone, &root);
        assert_eq!(pending(&gone, &url, &change).1, "failed", "the wallet's own payment, its coin gone");
        for st in [payer, gone, payee] { let _ = std::fs::remove_dir_all(st); }
    }

    /// `wait --timeout` looked at the time only between its steps, and a step could run on far past it: a nested
    /// `tips` (a peer may take 20 s to shake hands, and 24 are tried four at a time) or a request to a node that
    /// trickles its answer. The deadline now holds inside them. A nested `tips` whose one peer takes the connection and
    /// says nothing, which `tips` gives up on after 20 s, ends at a deadline 2 s away.
    #[test]
    fn a_nested_command_ends_at_the_deadline_wait_sets() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        std::thread::spawn(move || { let mut held = vec![]; for c in l.incoming() { held.push(c); } });
        let st = dir("deadline");
        let seeds = format!("{st}/seeds.txt");
        std::fs::write(&seeds, &addr).unwrap();
        let _one = crate::ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        let t = std::time::Instant::now();
        let ran = rt.block_on(async move { tokio::spawn(async move {
            let _d = crate::util::Deadline::set(std::time::Instant::now() + std::time::Duration::from_secs(2));
            self_run(&["tips", "--seeds", &seeds, "--peers", "1", "--tries", "1", "--quorum", "1", "--state", &st])
        }).await.unwrap() });
        let s = t.elapsed().as_secs_f64();
        assert!(!ran.ok, "no tip: the peer said nothing");
        assert!((1.5..5.0).contains(&s), "ended at the deadline: {s:.1} s");
        let _ = std::fs::remove_dir_all(dir("deadline"));
    }

    /// A Babel fee offer was proven with `prove_any`, held to no tip: a proof that matched a header above the agreed tip,
    /// which one node alone has shown, was taken, and a payment could be built on an offer the chain may not have. It is
    /// now held to the agreed tip, as `wallet sync` holds what it settles.
    #[test]
    fn a_babel_offer_is_proven_under_the_agreed_tip_or_not_used() {
        use ergotree_ir::chain::ergo_box::NonMandatoryRegisterId::{R4, R5};
        let token = "ab".repeat(32);
        let tree = ergotree_ir::ergo_tree::ErgoTree::sigma_parse_bytes(&unhex(&crate::wallet::babel_hex(&token))).unwrap();
        let pk = ergo_lib::wallet::ext_secret_key::ExtSecretKey::derive_master(ergo_lib::wallet::mnemonic::Mnemonic::to_seed(PHRASE, "")).unwrap().public_image();
        let cp = crate::network::checkpoint_header();
        let mut b = ergo_lib::chain::ergo_box::box_builder::ErgoBoxCandidateBuilder::new(ergotree_ir::chain::ergo_box::box_value::BoxValue::try_from(50_000_000u64).unwrap(), tree, cp.height - 100);
        b.set_register_value(R4, pk.into()); b.set_register_value(R5, 1_000i64.into());
        let offer = ergotree_ir::chain::ergo_box::ErgoBox::from_box_candidate(&b.build().unwrap(), ergotree_ir::chain::tx_id::TxId::zero(), 0).unwrap();
        let offer_bytes = offer.sigma_serialize_bytes().unwrap();
        let (url, root) = node_listing(vec![offer_bytes], (String::new(), r#"{"outputs":[]}"#.to_string()), vec![String::from(offer.box_id())]);
        let (under, above) = (dir("babel-under"), dir("babel-above"));
        for st in [&under, &above] { call(&["wallet", "new", "--state", st]); }
        // under: the agreed tip's state holds the offer
        chain_over(&under, &root);
        // above: the agreed tip's state does not; a header one block above it, which a node added, does
        chain_over(&above, &root_of(&[vec![9u8; 60]]));
        let mut h = crate::network::checkpoint_header();
        h.timestamp = crate::chain::now_ms(); h.height += 1;
        h.state_root = ergo_chain_types::ADDigest::try_from(root.clone()).unwrap();
        let hb = h.scorex_serialize_bytes().unwrap();
        let mut chain = (hb.len() as u16).to_be_bytes().to_vec(); chain.extend_from_slice(&hb);
        chain.extend_from_slice(&std::fs::read(format!("{above}/agreed-chain.bin")).unwrap());
        std::fs::write(format!("{above}/agreed-chain.bin"), chain).unwrap();
        let offers = |st: &str| call(&["wallet", "babel-list", "--state", st, "--node", &url, "--token", &token])["offers"].as_array().map_or(0, |a| a.len());
        assert_eq!(offers(&under), 1, "an offer proven under the agreed tip is listed");
        assert_eq!(offers(&above), 0, "an offer proven only above the agreed tip is not used");
        for st in [under, above] { let _ = std::fs::remove_dir_all(st); }
    }

    /// A receipt whose payment is held and not in a block yet answered `nanoerg: null`: the amount came only from the
    /// boxes the wallet tracks. It now says what the held payment would make that box hold, its own payment's change
    /// or a payment held for it, and is still not accepted.
    #[test]
    fn a_pending_receipt_says_what_the_held_payment_would_make() {
        let (payer, coin, p, payee) = paid("amount");
        let change = wallet_json(&payer)["expect"][0].as_str().unwrap().to_string();
        let tx: serde_json::Value = serde_json::from_str(p["tx_json"].as_str().unwrap()).unwrap();
        let receipt = tx["outputs"].as_array().unwrap().iter().map(|o| o["boxId"].as_str().unwrap().to_string()).find(|b| *b != change).unwrap();
        let (url, root) = node(vec![coin], (p["tx_id"].as_str().unwrap().to_string(), p["tx_json"].as_str().unwrap().to_string()));
        chain_over(&payee, &root);
        call(&["wallet", "promise", "--state", &payee, "--tx", p["tx_bytes"].as_str().unwrap()]);
        let what = |st: &str, b: &str| { let r = receipt_of(st, &url, b); (r.accepted, r.nanoerg, r.tokens.len()) };
        assert_eq!(what(&payer, &change), (false, Some(10_000_000_000 - 2_000_000_000 - 1_100_000), 0), "its own payment's change: what is left after the payment and the fee");
        assert_eq!(what(&payee, &receipt), (false, Some(2_000_000_000), 0), "a payment held for it: what it pays");
        for st in [payer, payee] { let _ = std::fs::remove_dir_all(st); }
    }

    /// A payment fetched from a node's pool (`--pools`) had its coins proven with `prove_any`, held to no tip: a proof
    /// that matched only a header above the agreed tip, which one node alone has shown, was taken, and the receipt
    /// answered `pending: true` on that node's word. Its coins are now proven under the agreed tip, as the comment on
    /// `--pools` says, or the payment is not held.
    #[test]
    fn a_pool_payment_is_held_only_on_coins_proven_under_the_agreed_tip() {
        let (payer, coin, p, payee) = paid("pool-above");
        let change = wallet_json(&payer)["expect"][0].as_str().unwrap().to_string();
        let tx: serde_json::Value = serde_json::from_str(p["tx_json"].as_str().unwrap()).unwrap();
        let receipt = tx["outputs"].as_array().unwrap().iter().map(|o| o["boxId"].as_str().unwrap().to_string()).find(|b| *b != change).unwrap();
        let (url, root) = node(vec![coin], (p["tx_id"].as_str().unwrap().to_string(), p["tx_json"].as_str().unwrap().to_string()));
        let under = dir("pool-under");
        std::fs::copy(format!("{payee}/wallet.json"), format!("{under}/wallet.json")).unwrap();
        // under: the agreed tip's state holds the coin
        chain_over(&under, &root);
        // above: the agreed tip's state does not; a header one block above it, which a node added, does
        chain_over(&payee, &root_of(&[vec![9u8; 60]]));
        let mut h = crate::network::checkpoint_header();
        h.timestamp = crate::chain::now_ms(); h.height += 1;
        h.state_root = ergo_chain_types::ADDigest::try_from(root.clone()).unwrap();
        let hb = h.scorex_serialize_bytes().unwrap();
        let mut chain = (hb.len() as u16).to_be_bytes().to_vec(); chain.extend_from_slice(&hb);
        chain.extend_from_slice(&std::fs::read(format!("{payee}/agreed-chain.bin")).unwrap());
        std::fs::write(format!("{payee}/agreed-chain.bin"), chain).unwrap();
        assert_eq!(pending(&under, &url, &receipt).0, Some(true), "its coin proven under the agreed tip: held, on its way");
        assert_eq!(pending(&payee, &url, &receipt).0, None, "its coin proven only above the agreed tip: not held");
        for st in [payer, payee, under] { let _ = std::fs::remove_dir_all(st); }
    }
}

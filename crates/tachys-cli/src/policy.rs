//! The spending policy: limits on what may leave a wallet, for a wallet a program holds.
//!
//! When `<state>/policy.json` exists, every command that signs is held to it, not only `tachys pay`:
//! the limits are applied to what the transaction about to be signed takes out of the wallet, read off
//! the transaction itself. Each signature is written to `<state>/payments.log` before it is made, under
//! a lock, so two payments at once cannot both pass on the same daily total.
//!
//!   { "max_per_payment_nanoerg": N, "max_per_day_nanoerg": N, "allowed_payees": ["addr", ...],
//!     "allow_tokens": false, "token_limits": { "<token id>": { "per_payment": RAW, "per_day": RAW } } }
//!
//! Where this holds: against a caller that asks tachys for too much through a wallet directory it
//! cannot read or copy. It is not a wall against a program running as the wallet's own user. That
//! program can read the wallet file and the password, so it can copy the wallet to a directory with
//! no policy, or open the seed phrase without tachys. For the limits to bind an agent, the agent
//! must not be that user (`docs/agents.md`).
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};

/// What a transaction about to be signed takes out of the wallet.
#[derive(Debug, Default, Clone)]
pub struct Ask {
    /// nanoERG that leaves: what the wallet's boxes put in, less what comes back to it. The network fee is in this.
    pub nano: u64,
    /// Token units that leave, by token id (payments, fees paid in a token, burns).
    pub tokens: BTreeMap<String, u64>,
    /// Where it goes: the address of every output that is neither the wallet's own nor the network fee.
    pub payees: Vec<String>,
    pub purpose: String,
    /// The command that asked, for the log.
    pub what: String,
}

pub fn path(state: &str) -> String { format!("{state}/policy.json") }
pub fn log_path(state: &str) -> String { format!("{state}/payments.log") }
pub fn present(state: &str) -> bool { std::path::Path::new(&path(state)).exists() }

/// The limits applied to one request, given the log so far. Nothing is read or written here.
pub fn check(policy: &serde_json::Value, log: &str, ask: &Ask, now_s: u64) -> Result<(), String> {
    if !policy.is_object() { return Err("the policy file is not a JSON object".into()) }
    let limit = |k: &str| -> Result<u64, String> {
        match &policy[k] { serde_json::Value::Null => Ok(0), v => v.as_u64().ok_or(format!("the policy's {k} is not a whole number")) }
    };
    let (per_payment, per_day) = (limit("max_per_payment_nanoerg")?, limit("max_per_day_nanoerg")?);
    // who may be paid
    match &policy["allowed_payees"] {
        serde_json::Value::Null => {}
        serde_json::Value::Array(list) => {
            if !list.is_empty() {
                if let Some(p) = ask.payees.iter().find(|p| !list.iter().any(|a| a.as_str() == Some(p.as_str()))) {
                    return Err(format!("{p} is not on the policy's list of payees"));
                }
            }
        }
        _ => return Err("the policy's allowed_payees is not a list".into()),
    }
    // what has left in the last 24 hours, from the log: a line that cannot be read is a reason to stop, not to count as nothing
    let (mut today, mut tokens_today): (u64, BTreeMap<String, u64>) = (0, BTreeMap::new());
    for line in log.lines().filter(|l| !l.trim().is_empty()) {
        let e: serde_json::Value = serde_json::from_str(line).map_err(|_| "the payments log has a line that cannot be read; the daily limit cannot be checked".to_string())?;
        let at = e["at"].as_u64().ok_or("the payments log has an entry with no time; the daily limit cannot be checked")?;
        if now_s.saturating_sub(at) >= 86_400 { continue }
        // an amount that is there and is not a whole number is a line that cannot be read, not a payment of nothing
        // (the second line a signature writes, with the transaction id, carries no amount and counts for nothing)
        today = today.saturating_add(match e.get("nanoerg") { None => 0, Some(n) => n.as_u64().ok_or("the payments log has an entry whose amount cannot be read; the daily limit cannot be checked")? });
        for t in e["tokens"].as_array().into_iter().flatten() {
            let n = tokens_today.entry(t["id"].as_str().unwrap_or("").to_string()).or_default();
            *n = n.saturating_add(t["amount"].as_u64().unwrap_or(0));
        }
    }
    if ask.nano > per_payment { return Err(format!("over the per-payment limit: {} nanoERG would leave the wallet (fee included), {per_payment} allowed", ask.nano)) }
    if today.saturating_add(ask.nano) > per_day { return Err(format!("over the daily limit: {today} nanoERG left in the last 24 h, {} asked, {per_day} allowed", ask.nano)) }
    if !ask.tokens.is_empty() && policy["allow_tokens"].as_bool() != Some(true) { return Err("the policy does not allow tokens to leave the wallet".into()) }
    for (id, amount) in &ask.tokens {
        let l = &policy["token_limits"][id.as_str()];
        let (Some(pp), Some(pd)) = (l["per_payment"].as_u64(), l["per_day"].as_u64()) else {
            return Err(format!("the policy sets no limit for token {id}: add it to token_limits with per_payment and per_day, in raw units"));
        };
        if *amount > pp { return Err(format!("over the per-payment limit for token {id}: {amount} units would leave the wallet, {pp} allowed")) }
        let had = tokens_today.get(id).copied().unwrap_or(0);
        if had.saturating_add(*amount) > pd { return Err(format!("over the daily limit for token {id}: {had} units left in the last 24 h, {amount} asked, {pd} allowed")) }
    }
    Ok(())
}

fn now_s() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

/// Hold a signature to the policy, if the wallet has one. `Ok(false)`: no policy file, nothing to hold it to.
/// `Ok(true)`: within the limits, and (unless `dry`) written to the log, so it counts from now on whether or
/// not the payment goes through. The log must already exist: a missing log is not an empty one.
pub fn reserve(state: &str, ask: &Ask, dry: bool) -> Result<bool, String> {
    if !present(state) { return Ok(false) }
    let policy: serde_json::Value = std::fs::read_to_string(path(state)).ok().and_then(|t| serde_json::from_str(&t).ok())
        .ok_or("the policy file cannot be read; nothing is signed while it is in that state")?;
    let lp = log_path(state);
    let mut f = std::fs::OpenOptions::new().read(true).append(true).open(&lp)
        .map_err(|_| format!("there is a policy but no payments log at {lp}; create an empty one beside the policy (the daily limit is counted from it)"))?;
    // one signer at a time: the total is read and the new entry written under the same lock
    f.lock().map_err(|e| format!("payments log: {e}"))?;
    let mut log = String::new();
    f.seek(SeekFrom::Start(0)).and_then(|_| f.read_to_string(&mut log)).map_err(|e| format!("payments log: {e}"))?;
    let now = now_s();
    check(&policy, &log, ask, now)?;
    if !dry {
        let entry = serde_json::json!({ "at": now, "nanoerg": ask.nano, "to": ask.payees, "purpose": ask.purpose, "what": ask.what,
            "tokens": ask.tokens.iter().map(|(id, amount)| serde_json::json!({ "id": id, "amount": amount })).collect::<Vec<_>>() });
        let lead = if log.is_empty() || log.ends_with('\n') { "" } else { "\n" };
        f.write_all(format!("{lead}{entry}\n").as_bytes()).and_then(|_| f.sync_all()).map_err(|e| format!("the payment could not be written to the log, so it is not made: {e}"))?;
    }
    Ok(true)
}

/// A line for the record that counts for nothing: which transaction a reserved entry became.
pub fn note(state: &str, tx: &str, receipt: &str) {
    if !present(state) { return }
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(log_path(state)) {
        if f.lock().is_ok() { let _ = writeln!(f, "{}", serde_json::json!({ "at": now_s(), "nanoerg": 0, "signed": tx, "receipt": receipt })); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const NOW: u64 = 2_000_000_000;
    const T: &str = "03faf2cb329f2e90d6d23b58d91bbb6c046aa143261cc21f52fbe2824bfcbf04";
    fn policy() -> serde_json::Value {
        serde_json::json!({ "max_per_payment_nanoerg": 50_000_000u64, "max_per_day_nanoerg": 100_000_000u64, "allowed_payees": [], "allow_tokens": false })
    }
    fn ask(nano: u64) -> Ask { Ask { nano, payees: vec!["9payee".into()], ..Default::default() } }
    fn line(ago: u64, nano: u64) -> String { format!("{}\n", serde_json::json!({ "at": NOW - ago, "nanoerg": nano })) }

    #[test]
    fn erg_limits_count_the_fee_and_the_day() {
        let p = policy();
        assert!(check(&p, "", &ask(50_000_000), NOW).is_ok());
        assert!(check(&p, "", &ask(50_000_001), NOW).unwrap_err().contains("per-payment"));
        let log = line(3600, 40_000_000) + &line(7200, 40_000_000);
        assert!(check(&p, &log, &ask(20_000_000), NOW).is_ok());
        assert!(check(&p, &log, &ask(20_000_001), NOW).unwrap_err().contains("daily"));
        // what left more than a day ago no longer counts
        let old = line(86_400, 90_000_000);
        assert!(check(&p, &old, &ask(50_000_000), NOW).is_ok());
    }

    #[test]
    fn a_policy_with_no_limits_allows_nothing() {
        assert!(check(&serde_json::json!({}), "", &ask(1), NOW).is_err());
        assert!(check(&serde_json::json!({ "max_per_payment_nanoerg": "lots" }), "", &ask(1), NOW).is_err());
        assert!(check(&serde_json::json!([]), "", &ask(1), NOW).is_err());
    }

    #[test]
    fn a_log_that_cannot_be_read_stops_payments() {
        let p = policy();
        assert!(check(&p, "not json\n", &ask(1), NOW).unwrap_err().contains("cannot be read"));
        assert!(check(&p, "{\"nanoerg\":5}\n", &ask(1), NOW).unwrap_err().contains("no time"));
        // sums that would wrap are held at the top, not wrapped round to nothing
        let huge = line(10, u64::MAX) + &line(20, u64::MAX);
        assert!(check(&p, &huge, &ask(1), NOW).unwrap_err().contains("daily"));
    }

    #[test]
    fn payees_are_held_to_the_list() {
        let mut p = policy();
        p["allowed_payees"] = serde_json::json!(["9friend"]);
        assert!(check(&p, "", &ask(1_000_000), NOW).unwrap_err().contains("not on the policy's list"));
        let mut a = ask(1_000_000); a.payees = vec!["9friend".into()];
        assert!(check(&p, "", &a, NOW).is_ok());
        a.payees.push("9stranger".into());
        assert!(check(&p, "", &a, NOW).is_err(), "every payee of a payment must be listed");
        p["allowed_payees"] = serde_json::json!("9friend");
        assert!(check(&p, "", &a, NOW).unwrap_err().contains("not a list"));
    }

    #[test]
    fn tokens_need_leave_and_a_limit_of_their_own() {
        let mut p = policy();
        let mut a = ask(2_100_000); a.tokens.insert(T.into(), 900_000_000_000_000_000);
        assert!(check(&p, "", &a, NOW).unwrap_err().contains("does not allow tokens"));
        p["allow_tokens"] = serde_json::json!(true);
        assert!(check(&p, "", &a, NOW).unwrap_err().contains("sets no limit for token"));
        p["token_limits"] = serde_json::json!({ T: { "per_payment": 500, "per_day": 800 } });
        assert!(check(&p, "", &a, NOW).unwrap_err().contains("per-payment limit for token"));
        a.tokens.insert(T.into(), 500);
        assert!(check(&p, "", &a, NOW).is_ok());
        let log = format!("{}\n", serde_json::json!({ "at": NOW - 60, "nanoerg": 2_100_000, "tokens": [{ "id": T, "amount": 400 }] }));
        assert!(check(&p, &log, &a, NOW).unwrap_err().contains("daily limit for token"));
        a.tokens.insert(T.into(), 400);
        assert!(check(&p, &log, &a, NOW).is_ok());
    }

    #[test]
    fn reserving_writes_the_log_first_and_needs_one_to_exist() {
        let dir = std::env::temp_dir().join(format!("tachys-policy-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state = dir.to_string_lossy().to_string();
        assert_eq!(reserve(&state, &ask(1), false), Ok(false), "no policy file: nothing to hold it to");
        std::fs::write(path(&state), policy().to_string()).unwrap();
        assert!(reserve(&state, &ask(1), false).unwrap_err().contains("no payments log"));
        std::fs::write(log_path(&state), "").unwrap();
        assert_eq!(reserve(&state, &ask(40_000_000), true), Ok(true));
        assert_eq!(std::fs::read_to_string(log_path(&state)).unwrap(), "", "a dry run writes nothing");
        assert_eq!(reserve(&state, &ask(40_000_000), false), Ok(true));
        assert_eq!(reserve(&state, &ask(40_000_000), false), Ok(true));
        assert!(reserve(&state, &ask(40_000_000), false).unwrap_err().contains("daily"), "the third would pass 100000000 in a day");
        assert_eq!(std::fs::read_to_string(log_path(&state)).unwrap().lines().count(), 2);
        std::fs::write(path(&state), "{ not json").unwrap();
        assert!(reserve(&state, &ask(1), false).unwrap_err().contains("cannot be read"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! Payments carried for later: handing one to a serving wallet, and submitting the ones held here.

use std::time::Duration;

use crate::chain::{agreed_height, load_chain};
use crate::proofs::{check_parcel, prove, RestCost};
use crate::util::{arg, hex, http, one_line, unhex};

/// tachys handoff --peer ADDR --parcel FILE
pub(crate) async fn handoff(args: &[String]) -> crate::out::Ended {
    // give a signed payment to a serving wallet that accepts parcels
    let peer: String = arg(args, "--peer", String::new())?;
    let file: String = arg(args, "--parcel", String::new())?;
    let raw = std::fs::read(&file).map_err(|e| crate::out::Fail::new(2, format!("--parcel {file}: {e}")))?;
    match tachys_net::handoff(&peer, &raw, Duration::from_secs(8)).await {
        Ok(id) => { crate::out::set("acknowledged", hex(&id)); say!("handed to {peer}: it acknowledged tx {} ({} B)", hex(&id), raw.len()) }
        Err(e) => { say!("handoff to {peer} failed: {e}"); return Err(crate::out::Fail::said(1)) }
    }
    Ok(())
}

/// tachys outbox --state DIR (--rest URL | --peer ADDR)
pub(crate) async fn outbox(args: &[String]) -> crate::out::Ended {
    // broadcast the payments this wallet is carrying, over the peer protocol
    let state: String = arg(args, "--state", "state".to_string())?;
    let peer: String = arg(args, "--peer", String::new())?;
    if peer.is_empty() && !args.iter().any(|a| a == "--rest") { return Err(crate::out::Fail::new(2, "usage: tachys outbox --state DIR (--rest URL|auto | --peer HOST:PORT)".into())) }
    let dir = format!("{state}/outbox");
    let files: Vec<_> = std::fs::read_dir(&dir).map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect()).unwrap_or_default();
    // `--rest URL`: hand each waiting payment to a node's REST port. The node validates it and a refusal
    // costs nothing, so no check is needed first. A refused payment is kept until the chain settles what became of it.
    // one node: a payment is handed to it and, if refused, its fate is read from a proof by the same node (`--rest auto` finds one)
    let rest: String = { let r: String = arg(args, "--rest", String::new())?; if r.is_empty() { r } else { crate::nodes::each(&crate::nodes::resolved(&r, &state)?).into_iter().next().unwrap_or_default() } };
    if !rest.is_empty() {
        let (mut sent, mut kept, mut dropped) = (0, 0, 0);
        let chain = load_chain(&state);
        for f in &files {
            let Some(v) = std::fs::read(f).ok().and_then(|r| serde_json::from_slice::<serde_json::Value>(&r).ok()) else { continue };
            // what the node is sent is the transaction read from the stored bytes, written out again, never the stored
            // `tx_json` (which nothing compares with the bytes), and only if the bytes are its own serialisation (finding 51)
            use ergotree_ir::serialization::SigmaSerializable;
            let read = v["tx_bytes"].as_str().and_then(unhex).and_then(|b| ergo_lib::chain::transaction::Transaction::sigma_parse_bytes(&b).ok().filter(|t| t.sigma_serialize_bytes().ok().as_deref() == Some(&b[..])));
            let Some(json) = read.as_ref().and_then(|t| serde_json::to_string(t).ok()) else {
                let id = v["tx_id"].as_str().unwrap_or("?").to_string();
                let moved = set_aside(&state, f, &id);
                say!("  {id} REFUSED before submission: its stored bytes are not a transaction's own serialisation; {}", if moved { "set aside in given-up/, not sent" } else { "kept, not sent" });
                dropped += 1; continue
            };
            match http().post(&format!("{rest}/transactions")).set("Content-Type", "application/json").send_string(&json) {
                // A node taking it is not the payment being in a block, and this file may be the payee's only copy: it is
                // kept, marked, and handed over again each run until a proof shows the payment in the chain (the node
                // then refuses it as already there, and the branch below reads its fate).
                Ok(_) => { sent += 1; let mut v = v.clone(); v["taken_by"] = serde_json::json!(rest); let _ = crate::util::write_whole(&f.to_string_lossy(), v.to_string().as_bytes()); }
                Err(ureq::Error::Status(code, r)) => {
                    // A refusal is not the end of a payment: the node may be busy, behind, or already holding it. For an
                    // offline payment this file is the payee's only copy, so it goes only when the chain says there is
                    // nothing left to do: a box the payment creates exists (it is in a block), or a box it spends is
                    // gone (it can never be), in both cases by a proof at or below the agreed tip. Failing that, it is kept;
                    // after twenty refusals it is moved out of the queue into `given-up/`, never deleted.
                    use ergotree_ir::serialization::SigmaSerializable;
                    let why = one_line(r.into_string().unwrap_or_default().lines().find(|l| l.contains("detail")).unwrap_or(""));
                    let id = v["tx_id"].as_str().unwrap_or("").to_string();
                    let tx = v["tx_bytes"].as_str().and_then(unhex).and_then(|b| ergo_lib::chain::transaction::Transaction::sigma_parse_bytes(&b).ok());
                    let fate = tx.and_then(|tx| {
                        let mut ids: Vec<String> = tx.inputs.iter().map(|i| String::from(i.box_id)).collect();
                        let n_in = ids.len();
                        // every box it makes: any one there shows it is in a block, whoever spent the others since
                        ids.extend(tx.outputs.iter().map(|o| String::from(o.box_id())));
                        let (height, found) = prove(&rest, &chain, &ids, &mut RestCost::default()).ok()?;
                        if height > agreed_height(&state) { return None } // a block the peers have not reached settles nothing
                        if found[n_in..].iter().any(|b| b.is_some()) { return Some("it is already in a block") }
                        let gone: Vec<String> = ids[..n_in].iter().zip(&found[..n_in]).filter(|(_, b)| b.is_none()).map(|(i, _)| i.clone()).collect();
                        if gone.is_empty() { return None }
                        // a payment held for this wallet is judged by the wallet's own sync, which knows which coins it saw
                        // unspent and at what height (a coin never seen may not be made yet; a node behind may not have it yet)
                        if promised(&state, &tx) { return None }
                        Some("coins it spends are gone; it can never confirm")
                    });
                    let refused = v["refused"].as_u64().unwrap_or(0) + if (400..500).contains(&code) { 1 } else { 0 };
                    // a payment held for this wallet is not put aside for refusals: it may be one that chains on a payment
                    // not yet in a block, refused until that one is; its fate is the wallet's sync to settle
                    let held_for_us = v["tx_bytes"].as_str().and_then(unhex).and_then(|b| ergo_lib::chain::transaction::Transaction::sigma_parse_bytes(&b).ok()).is_some_and(|t| promised(&state, &t));
                    match fate {
                        Some("it is already in a block") => { say!("  {id} refused ({code}): {why}; it is already in a block"); sent += 1; let _ = std::fs::remove_file(f); }
                        Some(end) => {
                            say!("  {id} refused ({code}): {why}; {end}; dropped"); dropped += 1; let _ = std::fs::remove_file(f);
                            if let Some(tx) = v["tx_bytes"].as_str().and_then(unhex).and_then(|b| ergo_lib::chain::transaction::Transaction::sigma_parse_bytes(&b).ok()) { void_promised(&state, &tx, end); }
                        }
                        None if refused >= 20 && !held_for_us => {
                            // not deleted: a node can refuse on purpose. It leaves the queue and stays on disk.
                            let moved = set_aside(&state, f, &id);
                            say!("  {id} refused ({code}): {why}; refused {refused} times; {}", if moved { "set aside in given-up/, not sent again" } else { "kept" });
                            if moved { dropped += 1 } else { kept += 1 }
                        }
                        None => {
                            say!("  {id} refused ({code}): {why}; kept, to be tried again");
                            let mut v = v.clone(); v["refused"] = serde_json::json!(refused);
                            let _ = crate::util::write_whole(&f.to_string_lossy(), v.to_string().as_bytes());
                            kept += 1;
                        }
                    }
                }
                Err(_) => kept += 1, // no network: keep it for next time
            }
        }
        crate::out::set("sent", sent); crate::out::set("waiting", kept); crate::out::set("dropped", dropped);
        say!("outbox: {sent} sent, {kept} still waiting, {dropped} refused");
        return Ok(());
    }
    say!("outbox: {} payment(s) to submit to {peer}", files.len());
    let mut chain = load_chain(&state);
    let (mut submitted, mut refused, mut unchecked) = (0, 0, 0);
    for f in files {
        use ergotree_ir::serialization::SigmaSerializable;
        let Some(v) = std::fs::read(&f).ok().and_then(|r| serde_json::from_slice::<serde_json::Value>(&r).ok()) else { continue };
        let Some(bytes) = v["tx_bytes"].as_str().and_then(unhex) else { continue };
        // the id is recomputed from the bytes, never taken from the parcel
        let Ok(tx) = ergo_lib::chain::transaction::Transaction::sigma_parse_bytes(&bytes) else { say!("  {:?}: not a transaction; dropped", f.file_name()); let _ = std::fs::remove_file(&f); refused += 1; continue };
        let Some(id) = unhex(&String::from(tx.id())).and_then(|b| <[u8; 32]>::try_from(b).ok()) else { continue };
        // The stored bytes are what the node is sent; the id announced and every check below are of the transaction
        // read from them. The two must be the same thing: bytes that parse but are not the transaction's own
        // serialisation (something after its end, a number written the long way) would reach the node as a
        // transaction other than the one announced and checked. No chain and no signature settles this, so the
        // file is not deleted: it leaves the queue for `given-up/`.
        if tx.sigma_serialize_bytes().ok().as_deref() != Some(&bytes[..]) {
            let moved = set_aside(&state, &f, &hex(&id));
            say!("  {} REFUSED before submission: its stored bytes ({} B) are not the transaction's own serialisation; {}", hex(&id), bytes.len(), if moved { "set aside in given-up/, not sent" } else { "kept, not sent" });
            refused += 1; continue
        }
        // A node penalises a peer that delivers an invalid transaction, so check before submitting:
        // every input must be proven unspent and every spending proof must verify.
        match check_parcel(&peer, &mut chain, agreed_height(&state), &tx) {
            // a payment held for this wallet with a coin missing: kept; the wallet's sync judges it (it knows which coins it saw)
            Err((why, true, false, Some(_))) if promised(&state, &tx) => { say!("  {} could not be checked: {why}; kept, for this wallet's own sync to judge", hex(&id)); unchecked += 1; continue }
            Err((why, true, on_chain, _)) => {
                // a payment held for this wallet may be its only copy: put aside, never deleted
                if promised(&state, &tx) { let _ = set_aside(&state, &f, &hex(&id)); say!("  {} REFUSED before submission: {why}; set aside in given-up/", hex(&id)); } else { say!("  {} REFUSED before submission: {why}; dropped", hex(&id)); let _ = std::fs::remove_file(&f); }
                refused += 1;
                if !on_chain { void_promised(&state, &tx, &why); }
                continue
            }
            Err((why, false, _, _)) => { say!("  {} could not be checked: {why}; kept", hex(&id)); unchecked += 1; continue }
            Ok(h) => say!("  {} checked: inputs proven unspent at height {h}, spending proofs verify", hex(&id)),
        }
        submitted += 1;
        match tachys_net::submit_tx(&peer, id, bytes, Duration::from_secs(8)).await {
            Ok((true, c)) => { say!("  {} delivered on request ({} messages sent, {} ms held)", hex(&id), c.requests_sent, c.held_ms); let _ = std::fs::remove_file(&f); }
            Ok((false, _)) => say!("  {} announced, but the node did not ask for it; kept", hex(&id)),
            Err(e) => say!("  {} not submitted: {e}; kept", hex(&id)),
        }
    }
    crate::out::set("submitted", submitted); crate::out::set("refused", refused); crate::out::set("unchecked", unchecked);
    say!("outbox done: {submitted} submitted, {refused} refused");
    Ok(())
}

/// A held payment has been dropped for good, and not because it is on chain: if this state directory is a wallet that
/// was promised boxes by it (`wallet promise`), those boxes will never come. They leave what the wallet expects and are
/// marked void, so that asking about one gets that answer (`void`) and not "not yet", for ever. A carrier that holds
/// someone else's payment expects none of its boxes, and its wallet file, if any, is left as it was.
fn void_promised(state: &str, tx: &ergo_lib::chain::transaction::Transaction, reason: &str) {
    let path = format!("{state}/wallet.json");
    if !std::path::Path::new(&path).exists() { return }
    // the wallet file is read, changed and written back: this command's turn at it, as every command that writes it takes
    let Ok(_turn) = crate::util::turn(state) else { say!("  could not take a turn at {path}; the boxes it promised stay expected"); return };
    let Some(mut w) = std::fs::read_to_string(&path).ok().and_then(|t| crate::walletfile::parse(&t).ok()) else { return };
    let outs: Vec<String> = tx.outputs.iter().map(|o| String::from(o.box_id())).collect();
    let promised: Vec<String> = w.expect.iter().filter(|e| outs.contains(e)).cloned().collect();
    if promised.is_empty() { return }
    w.expect.retain(|e| !promised.contains(e));
    for p in &promised { if !w.void.contains(p) { w.void.push(p.clone()); } }
    let n = w.void.len(); if n > 64 { w.void.drain(..n - 64); }
    let saved = crate::walletfile::text(&w).ok().is_some_and(|t| crate::util::write_whole(&path, t.as_bytes()).is_ok());
    if saved { crate::out::push("void", serde_json::json!({ "boxes": promised, "reason": reason })); say!("  the box{} it promised this wallet ({}) will never come: marked void", if promised.len() == 1 { "" } else { "es" }, promised.join(", ")); }
    else { say!("  could not mark the boxes it promised as void in {path}; they stay expected"); }
}

/// Move a held payment out of the queue into `DIR/given-up/`, where nothing sends it again and nothing deletes it.
fn set_aside(state: &str, f: &std::path::Path, id: &str) -> bool {
    let aside = format!("{state}/given-up");
    let _ = std::fs::create_dir_all(&aside);
    std::fs::rename(f, format!("{aside}/{}", f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| format!("{id}.json")))).is_ok()
}

/// Does this payment pay boxes the wallet in `state` expects (a payment held for it)?
fn promised(state: &str, tx: &ergo_lib::chain::transaction::Transaction) -> bool {
    let Some(w) = std::fs::read_to_string(format!("{state}/wallet.json")).ok().and_then(|t| crate::walletfile::parse(&t).ok()) else { return false };
    tx.outputs.iter().any(|o| w.expect.contains(&String::from(o.box_id())))
}
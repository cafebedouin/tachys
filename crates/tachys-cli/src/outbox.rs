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
            let Some(json) = v["tx_json"].as_str() else { continue };
            match http().post(&format!("{rest}/transactions")).set("Content-Type", "application/json").send_string(json) {
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
                        ids.push(String::from(tx.outputs.first().box_id()));
                        let (height, found) = prove(&rest, &chain, &ids, &mut RestCost::default()).ok()?;
                        if height > agreed_height(&state) { return None } // a block the peers have not reached settles nothing
                        if found[n_in].is_some() { Some("it is already in a block") } else if found[..n_in].iter().any(|b| b.is_none()) { Some("coins it spends are gone; it can never confirm") } else { None }
                    });
                    let refused = v["refused"].as_u64().unwrap_or(0) + if (400..500).contains(&code) { 1 } else { 0 };
                    match fate {
                        Some("it is already in a block") => { say!("  {id} refused ({code}): {why}; it is already in a block"); sent += 1; let _ = std::fs::remove_file(f); }
                        Some(end) => { say!("  {id} refused ({code}): {why}; {end}; dropped"); dropped += 1; let _ = std::fs::remove_file(f); }
                        None if refused >= 20 => {
                            // not deleted: a node can refuse on purpose. It leaves the queue and stays on disk.
                            let aside = format!("{state}/given-up");
                            let _ = std::fs::create_dir_all(&aside);
                            let to = format!("{aside}/{}", f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| format!("{id}.json")));
                            let moved = std::fs::rename(f, &to).is_ok();
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
    let (mut submitted, mut refused) = (0, 0);
    for f in files {
        use ergotree_ir::serialization::SigmaSerializable;
        let Some(v) = std::fs::read(&f).ok().and_then(|r| serde_json::from_slice::<serde_json::Value>(&r).ok()) else { continue };
        let Some(bytes) = v["tx_bytes"].as_str().and_then(unhex) else { continue };
        // the id is recomputed from the bytes, never taken from the parcel
        let Ok(tx) = ergo_lib::chain::transaction::Transaction::sigma_parse_bytes(&bytes) else { say!("  {:?}: not a transaction; dropped", f.file_name()); let _ = std::fs::remove_file(&f); refused += 1; continue };
        let Some(id) = unhex(&String::from(tx.id())).and_then(|b| <[u8; 32]>::try_from(b).ok()) else { continue };
        // A node penalises a peer that delivers an invalid transaction, so check before submitting:
        // every input must be proven unspent and every spending proof must verify.
        match check_parcel(&peer, &mut chain, agreed_height(&state), &tx) {
            Err((why, true)) => { say!("  {} REFUSED before submission: {why}; dropped", hex(&id)); let _ = std::fs::remove_file(&f); refused += 1; continue }
            Err((why, false)) => { say!("  {} could not be checked: {why}; kept", hex(&id)); continue }
            Ok(h) => say!("  {} checked: inputs proven unspent at height {h}, spending proofs verify", hex(&id)),
        }
        submitted += 1;
        match tachys_net::submit_tx(&peer, id, bytes, Duration::from_secs(8)).await {
            Ok((true, c)) => { say!("  {} delivered on request ({} messages sent, {} ms held)", hex(&id), c.requests_sent, c.held_ms); let _ = std::fs::remove_file(&f); }
            Ok((false, _)) => say!("  {} announced, but the node did not ask for it; kept", hex(&id)),
            Err(e) => say!("  {} not submitted: {e}; kept", hex(&id)),
        }
    }
    crate::out::set("submitted", submitted); crate::out::set("refused", refused);
    say!("outbox done: {submitted} submitted, {refused} refused");
    Ok(())
}

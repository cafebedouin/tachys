//! tachys serve: pass on the chain this wallet last verified, and optionally carry payments and relay proofs.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ergo_chain_types::Header;
use sigma_ser::ScorexSerializable;

use crate::chain::{carries_on, header_ok, id32, load_chain, parent32};
use crate::network::{net, release_anchor};
use crate::util::{arg, unhex};

/// tachys serve --state DIR [--listen ADDR] [--secs N] [--accept-parcels] [--relay-proofs HOST:PORT] [--follow-state]
pub(crate) async fn serve(args: &[String]) -> crate::out::Ended {
    // Pass on the chain this wallet last verified. Opt-in; meant for good connectivity only.
    let state: String = arg(args, "--state", "state".to_string())?;
    let listen: String = arg(args, "--listen", "0.0.0.0:9030".to_string())?;
    let secs: u64 = arg(args, "--secs", 60)?;
    let mut chain = load_chain(&state);
    if chain.is_empty() { return Err(crate::out::Fail::new(2, format!("nothing to serve: run `tachys tips --state {state}` first"))) }
    if args.iter().any(|a| a == "--tamper-for-control") {
        if !crate::network::private_net() { return Err(crate::out::Fail::new(2, "--tamper-for-control is for our own private network only".to_string())) }
        // control arm only: break the work of one header so a client must refuse the chain
        let i = chain.len() / 2;
        chain[i].autolykos_solution.nonce[0] ^= 1;
        say!("serving a TAMPERED chain (control): header at height {} has a flipped nonce", chain[i].height);
    }
    let headers: Vec<([u8; 32], Vec<u8>)> = chain.iter().filter_map(|h| h.scorex_serialize_bytes().ok().map(|b| (id32(h), b))).collect();
    say!("tachys serve ({}): {} headers {}..{} on {listen} for {secs} s", net().name, headers.len(), chain.last().map(|h| h.height).unwrap_or(0), chain.first().map(|h| h.height).unwrap_or(0));
    // Opt-in: take signed payments from other wallets and keep them for `tachys outbox`. A carrier
    // cannot validate a payment (it has no state); it checks only that the bytes are a
    // transaction with the stated id, and caps how many it holds.
    let sink: Option<tachys_net::ParcelSink> = if args.iter().any(|a| a == "--accept-parcels") {
        let dir = format!("{state}/outbox"); let _ = std::fs::create_dir_all(&dir);
        Some(std::sync::Arc::new(move |raw: &[u8]| {
            use ergotree_ir::serialization::SigmaSerializable;
            let v: serde_json::Value = serde_json::from_slice(raw).ok()?;
            let bytes = unhex(v["tx_bytes"].as_str()?)?;
            let tx = ergo_lib::chain::transaction::Transaction::sigma_parse_bytes(&bytes).ok()?;
            let id = String::from(tx.id());
            if std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0) >= 8 { return None }
            std::fs::write(format!("{dir}/{id}.json"), raw).ok()?;
            <[u8; 32]>::try_from(unhex(&id)?).ok()
        }))
    } else { None };
    // Opt-in: pass box-proof requests on to our own node (`--relay-proofs HOST:PORT`). If the node's
    // answer names a block newer than what we serve, we fetch those headers by id, accept them
    // only if they link onto our tip and carry work, and start serving them too.
    let upstream: String = arg(args, "--relay-proofs", String::new())?;
    let relay: Option<tachys_net::ProofRelay> = if upstream.is_empty() { None } else {
        let known = std::sync::Arc::new(std::sync::Mutex::new(chain.clone()));
        Some(std::sync::Arc::new(move |ids: Vec<[u8; 32]>| {
            let (upstream, known) = (upstream.clone(), known.clone());
            Box::pin(async move {
                let bp = tachys_net::fetch_box_proof(&upstream, &ids, Duration::from_secs(8)).await.ok()?;
                let mut new_headers = vec![];
                let have = known.lock().unwrap().iter().any(|h| id32(h) == bp.header_id);
                if !have {
                    let tip_id = known.lock().unwrap().first().map(id32)?;
                    let (raw, _c) = tachys_net::fetch_linking(&upstream, bp.header_id, tip_id, 12, |b: &[u8]| Header::scorex_parse_bytes(b).ok().map(|h| parent32(&h))).await.ok()?;
                    let fresh: Vec<Header> = raw.iter().filter_map(|b| Header::scorex_parse_bytes(b).ok()).collect();
                    let anchor = release_anchor();
                    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
                    let mut k = known.lock().unwrap();
                    let tip = k.first()?.clone();
                    let cut = fresh.iter().position(|h| h.parent_id == tip.id)?;
                    let add = &fresh[..=cut];
                    if !carries_on(&tip, add) || add.iter().any(|h| header_ok(h, &anchor, now).is_err()) { return None }
                    new_headers = add.iter().filter_map(|h| h.scorex_serialize_bytes().ok().map(|b| (id32(h), b))).collect();
                    let mut newer = add.to_vec(); newer.extend(k.drain(..)); *k = newer;
                }
                Some(tachys_net::RelayAnswer { header_id: bp.header_id, proof: bp.proof, new_headers })
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = Option<tachys_net::RelayAnswer>> + Send>>
        }))
    };
    // `--follow-state`: keep serving whatever `tachys tips --state` last verified, by re-reading the
    // agreed chain from disk when it changes. A wallet that syncs for itself thereby serves fresh headers.
    let reload: Option<tachys_net::Reload> = if args.iter().any(|a| a == "--follow-state") {
        let (st, last) = (state.clone(), std::sync::Arc::new(std::sync::Mutex::new(headers.first().map(|h| h.0))));
        Some(std::sync::Arc::new(move || {
            let chain = load_chain(&st);
            let tip = chain.first().map(id32);
            let mut l = last.lock().unwrap();
            if tip.is_none() || tip == *l { return None }
            *l = tip;
            Some(chain.iter().filter_map(|h| h.scorex_serialize_bytes().ok().map(|b| (id32(h), b))).collect())
        }))
    } else { None };
    match tachys_net::serve(&listen, headers, secs, 8, sink, relay, reload).await {
        Ok(s) => say!("served: {} connections, {} sync pushes, {} header requests ({} headers), {} parcels taken, {} proofs relayed, {} reloads, {} B sent", s.connections, s.sync_pushes, s.header_requests, s.headers_served, s.parcels_taken, s.proofs_relayed, s.reloads, s.bytes_sent),
        Err(e) => { say!("serve: {e}"); return Err(crate::out::Fail::said(1)) }
    }
    Ok(())
}

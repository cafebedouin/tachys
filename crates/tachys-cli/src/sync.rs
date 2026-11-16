//! tachys sync: ask several mainnet peers for a NiPoPoW proof, check each strictly, and report the
//! chain tip the peers agree on, by the same rule as `tachys tips`. `tachys check` does the same for proofs on disk.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ergo_chain_types::Header;
use ergo_nipopow::NipopowProof;
use sigma_ser::ScorexSerializable;
use tachys_verify::verify;
use tachys_verify::peers::{decide, PeerView};
use tokio::sync::Semaphore;

use crate::network::{chain_params, release_anchor};
use crate::util::{arg, clip, group_of, hex, id_hex};

/// A proof whose only fault is its interlink proofs may be in a newer form than this build reads: node PR #2461
/// proves interlinks against the header's whole extension root, where this verifier expects a tree of the interlinks alone.
pub(crate) fn newer_form_note(rep: &tachys_verify::Report) -> &'static str {
    if rep.failed() == [tachys_verify::Check::C11InterlinkProofs] { "\n      only the interlink proofs fail: this peer may serve the newer proof form (node PR #2461), which this build does not read" } else { "" }
}

/// tachys check FILE...: verify proofs already on disk (.bin = wire bytes, .json = the node's REST
/// form), one file per peer, named <ip>_<port>.<ext>, and apply the multi-peer rule.
pub(crate) fn check(files: &[String]) -> crate::out::Ended {
    let anchor = release_anchor();
    let params = chain_params();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let mut views = vec![];
    for f in files {
        let name = std::path::Path::new(f).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let bytes = match std::fs::read(f) { Ok(b) => b, Err(e) => { say!("  {name:24} -- {e}"); continue } };
        let proof: Result<NipopowProof, String> = if f.ends_with(".json") {
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())
        } else {
            NipopowProof::scorex_parse_bytes(&bytes).map_err(|e| format!("{e:?}"))
        };
        match proof {
            Err(e) => say!("  {name:24} {} B: PARSE ERROR {}", bytes.len(), clip(&e, 90)),
            Ok(proof) => {
                let wire = proof.scorex_serialize_bytes().map(|b| b.len()).unwrap_or(0);
                let rep = verify(&proof, &[], &anchor, &params, now);
                if rep.accepted() {
                    say!("  {name:24} {} B on disk, {} B wire: OK tip {} work-checked {} difficulty {:?}", bytes.len(), wire, rep.tip.as_ref().map(|t| t.height).unwrap_or(0), rep.work_checked, rep.difficulty);
                    let suffix: Vec<Header> = std::iter::once(proof.suffix_head.header.clone()).chain(proof.suffix_tail.iter().cloned()).collect();
                    views.push(PeerView { peer: name.clone(), group: group_of(&name), suffix });
                } else {
                    say!("  {name:24} {} B: REJECTED {:?}{}", bytes.len(), rep.failed(), newer_form_note(&rep));
                }
            }
        }
    }
    let d = decide(&views, 3);
    for (i, c) in d.clusters.iter().enumerate() {
        say!("cluster {}: {} peers in {} network groups, heights {}..{}, tip {}", i + 1, c.peers.len(), c.groups.len(), c.chain.keys().next().unwrap_or(&0), c.tip().height, id_hex(c.tip()));
    }
    for (h, a, b) in &d.conflicts { say!("CONFLICT at height {h}: {a:?} vs {b:?}"); }
    verdict(&views, 3, now)
}

/// The tip these proofs agree on, by the rule the wallet spends by (`tachys_verify::tip`): tips are tried heaviest
/// first, never highest first; the agreed tip is the newest header that `quorum` network groups have reached; and
/// none is agreed on while a heavier recent tip stands outside its chain. (Clusters of proofs used to be ranked by
/// groups, peers, then height, which a few peers on a higher, lighter chain could win.) A proof carries ten linked
/// headers and no more can be asked for, so six below the agreed tip are wanted, not ten.
pub(crate) fn by_work(views: &[PeerView], quorum: usize, now: u64, max_age_ms: u64, epoch_length: u32) -> Result<Header, Vec<String>> {
    use tachys_verify::tip::{candidates, judge, TipView};
    let newest_first = |v: &PeerView| { let mut c = v.suffix.clone(); c.sort_by_key(|h| std::cmp::Reverse(h.height)); c };
    let tips: Vec<TipView> = views.iter().filter_map(|v| newest_first(v).first().map(|t| TipView { group: v.group.clone(), tip: t.clone() })).collect();
    let mut refusals = vec![];
    for top in candidates(&tips) {
        let Some(chain) = views.iter().map(newest_first).find(|c| c.first().is_some_and(|t| t.id == top.id)) else { continue };
        match judge(&chain, &tips, &[], 6, quorum, epoch_length, now, max_age_ms) {
            Ok(a) => return Ok(chain[a.ahead].clone()),
            Err(why) => refusals.push(format!("height {}: {why:?}", top.height)),
        }
        if refusals.len() >= 3 { break }
    }
    if refusals.is_empty() { refusals.push("no proof was accepted".into()); }
    Err(refusals)
}

fn verdict(views: &[PeerView], quorum: usize, now: u64) -> crate::out::Ended {
    match by_work(views, quorum, now, release_anchor().max_tip_age_ms, chain_params().epoch_length) {
        Err(why) => { crate::out::set("proofs_verdict", "not_trusted"); crate::out::set("refusals", &why); say!("proofs verdict: NOT TRUSTED ({})", why.join("; ")); Err(crate::out::Fail::said(1)) }
        Ok(t) => {
            let root: Vec<u8> = t.state_root.into();
            crate::out::set("proofs_verdict", "agreed"); crate::out::set("tip", serde_json::json!({ "height": t.height, "id": id_hex(&t) }));
            say!("proofs verdict: Agreed (by work, then a quorum of network groups: the rule `tachys tips` uses)\ntip: height {} id {} ({} s old)\nstate root: {}", t.height, id_hex(&t), now.saturating_sub(t.timestamp) / 1000, hex(&root));
            Ok(())
        }
    }
}

/// tachys sync: fetch a proof from each of several peers and apply the multi-peer rule.
pub(crate) async fn sync(args: &[String]) -> crate::out::Ended {
    let want: usize = arg(args, "--peers", 8)?;
    let tries: usize = arg(args, "--tries", 32)?;
    let quorum: usize = arg(args, "--quorum", 3)?;
    let wait: u64 = arg(args, "--wait", 60)?;
    let save: String = arg(args, "--save", String::new())?;

    let seeds: Vec<String> = crate::network::seeds(args).map_err(|e| crate::out::Fail::new(2, e.to_string()))?.into_iter().take(tries).collect();
    say!("tachys sync: {} candidate peers, want {} proofs, quorum {} network groups", seeds.len(), want, quorum);

    let t0 = Instant::now();
    let sem = Arc::new(Semaphore::new(16));
    let (tx, mut rx) = tokio::sync::mpsc::channel(seeds.len().max(1));
    let mut tasks = vec![];
    for addr in seeds.clone() {
        let (sem, tx) = (sem.clone(), tx.clone());
        tasks.push(tokio::spawn(async move {
            let _p = sem.acquire().await;
            let r = tachys_net::fetch_proof(&addr, 6, 10, Duration::from_secs(wait)).await;
            let _ = tx.send((addr, r)).await;
        }));
    }
    drop(tx);

    let anchor = release_anchor();
    let params = chain_params();
    let (mut views, mut got, mut bytes, mut failed_conn, mut rejected) = (vec![], 0usize, 0usize, 0usize, 0usize);
    while let Some((addr, r)) = rx.recv().await {
        match r {
            Err(e) => { failed_conn += 1; say!("  {addr:22} -- {e}"); }
            Ok(pp) => {
                got += 1;
                bytes += pp.proof.len() + pp.other_bytes;
                if !save.is_empty() {
                    let _ = std::fs::create_dir_all(&save);
                    let _ = std::fs::write(format!("{save}/{}.bin", addr.replace(':', "_")), &pp.proof);
                    if let Some(u) = &pp.rest_url {
                        use std::io::Write;
                        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{save}/rest-urls.txt")) { let _ = writeln!(f, "{addr} {u}"); }
                    }
                }
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
                match NipopowProof::scorex_parse_bytes(&pp.proof) {
                    Err(e) => { rejected += 1; say!("  {addr:22} {} {} proof {} B: PARSE ERROR {e:?}", pp.agent, pp.version, pp.proof.len()); }
                    Ok(proof) => {
                        let rep = verify(&proof, &[], &anchor, &params, now);
                        let tip = rep.tip.as_ref().map(|t| t.height).unwrap_or(0);
                        if rep.accepted() {
                            say!("  {addr:22} {} {} proof {} B in {} ms (+{} other frames): OK tip {} work-checked {} difficulty {:?}",
                                pp.agent, pp.version, pp.proof.len(), pp.total_ms, pp.other_frames, tip, rep.work_checked, rep.difficulty);
                            let suffix: Vec<Header> = std::iter::once(proof.suffix_head.header.clone()).chain(proof.suffix_tail.iter().cloned()).collect();
                            views.push(PeerView { peer: addr.clone(), group: group_of(&addr), suffix });
                        } else {
                            rejected += 1;
                            say!("  {addr:22} {} {} proof {} B: REJECTED {:?}{}", pp.agent, pp.version, pp.proof.len(), rep.failed(), newer_form_note(&rep));
                            for (c, m) in rep.failures.iter().take(3) { say!("      {c:?}: {m}"); }
                        }
                    }
                }
                if views.len() >= want { break }
            }
        }
    }
    for t in tasks { t.abort(); }

    let d = decide(&views, quorum);
    say!("\npeers: {} answered with a proof, {} accepted, {} rejected, {} gave none; {} B received in {:.1} s",
        got, views.len(), rejected, failed_conn, bytes, t0.elapsed().as_secs_f64());
    for (i, c) in d.clusters.iter().enumerate() {
        say!("cluster {}: {} peers in {} network groups, heights {}..{}, tip {}", i + 1, c.peers.len(), c.groups.len(),
            c.chain.keys().next().unwrap_or(&0), c.tip().height, id_hex(c.tip()));
    }
    for (h, a, b) in &d.conflicts { say!("CONFLICT at height {h}: {a:?} vs {b:?}"); }
    verdict(&views, quorum, SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0))
}

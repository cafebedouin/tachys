//! A probe for a private rig network: what a node does with a proof request that carries a pad, and with a
//! request for a block's Extension section by id. Not for public nodes (docs/good-neighbour.md).
//!
//!   nipopow_probe <magic a,b,c,d> proof <host:port> <pad hex, or -> <file for the proof bytes> [m]
//!   nipopow_probe <magic a,b,c,d> section <host:port> <type id> <section id hex>
//!   nipopow_probe <magic a,b,c,d> extension <host:port> <header id hex> <extension root hex>
//!   nipopow_probe <magic a,b,c,d> gather <host:port> <mainnet|rig> <directory> [m]
//!   nipopow_probe <magic a,b,c,d> judge <mainnet|rig> <k, or auto> <directory>...
use std::time::Duration;

fn hex(s: &str) -> Vec<u8> { (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex")).collect() }
fn to_hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }
fn id32(s: &str) -> [u8; 32] { hex(s).try_into().expect("32 bytes") }

/// The parameters of a network: `mainnet`, or `rig` for the peeryard rig's devnet (difficulty epochs of 16
/// blocks under the rule from before EIP-37, version 2 from height 128 at difficulty 32, a 2 s block interval).
fn params(name: &str) -> tachys_verify::difficulty::ChainParams {
    use tachys_verify::difficulty::ChainParams;
    match name {
        "mainnet" => ChainParams::mainnet(),
        "rig" => ChainParams { epoch_length: 16, use_last_epochs: 8, block_interval_ms: 2000, v2_activation_height: 128,
            initial_difficulty: 1.into(), earlier_rule: Some(32.into()) },
        other => panic!("unknown network {other}"),
    }
}

fn tip_of(p: &ergo_nipopow::NipopowProof) -> u32 { p.suffix_tail.last().map_or(p.suffix_head.header.height, |h| h.height) }

/// What `gather` saved for one peer: its proof, the ids of the headers an Extension section settled, its run.
fn load(dir: &std::path::Path) -> (ergo_nipopow::NipopowProof, std::collections::HashSet<ergo_chain_types::BlockId>, Vec<ergo_chain_types::Header>) {
    use sigma_ser::ScorexSerializable;
    let proof = ergo_nipopow::NipopowProof::scorex_parse_bytes(&std::fs::read(dir.join("proof.bin")).expect("proof.bin")).expect("proof parses");
    let lines = |f: &str| -> Vec<String> { std::fs::read_to_string(dir.join(f)).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(String::from).collect() };
    let settled = lines("settled.txt").iter().map(|l| ergo_chain_types::BlockId(ergo_chain_types::Digest32::from(id32(l)))).collect();
    let run = lines("run.hex").iter().map(|l| ergo_chain_types::Header::scorex_parse_bytes(&hex(l)).expect("header")).collect();
    (proof, settled, run)
}

#[tokio::main]
async fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let magic: Vec<u8> = a[0].split(',').map(|x| x.trim().parse().expect("magic")).collect();
    tachys_net::set_network_magic(magic.try_into().expect("four magic bytes"));
    tachys_net::set_declared(tachys_net::Declared::Light);
    let wait = Duration::from_secs(15);
    match a[1].as_str() {
        "proof" => {
            let pad = if a[3] == "-" { vec![] } else { hex(&a[3]) };
            // an optional last argument asks for another sample than 6 headers a level
            let m: i32 = a.get(5).map_or(6, |x| x.parse().expect("m"));
            match tachys_net::fetch_proof_padded(&a[2], m, 10, &pad, wait).await {
                Ok(p) => {
                    std::fs::write(&a[4], &p.proof).expect("write");
                    use sigma_ser::ScorexSerializable;
                    // the tip, and which headers' interlinks are the committed ones by the bytes alone; a header whose
                    // extension holds other fields does not bind this way (on the rig: every 128th, a voting-epoch start)
                    let got_m = ergo_nipopow::NipopowProof::scorex_parse_bytes(&p.proof).map_or(0, |x| x.m);
                    let (tip, headers, bound, unbound): (u32, usize, usize, Vec<u32>) = match ergo_nipopow::NipopowProof::scorex_parse_bytes(&p.proof) {
                        Ok(parsed) => {
                            let hs: Vec<&ergo_nipopow::PoPowHeader> = parsed.prefix.iter().chain(std::iter::once(&parsed.suffix_head)).collect();
                            (parsed.suffix_tail.last().map_or(parsed.suffix_head.header.height, |h| h.height), hs.len() + parsed.suffix_tail.len(),
                             hs.iter().filter(|h| tachys_verify::interlinks_binding(h) == tachys_verify::Binding::Bound).count(),
                             hs.iter().filter(|h| tachys_verify::interlinks_binding(h) == tachys_verify::Binding::Unbound).map(|h| h.header.height).collect())
                        }
                        Err(_) => (0, 0, 0, vec![]),
                    };
                    println!("proof pad={} asked_m={m} m={got_m} bytes={} tip={} headers={} bound={} unbound={:?} unbound_elsewhere={} agent={} version={} ms={}",
                        to_hex(&pad), p.proof.len(), tip, headers, bound, unbound, unbound.iter().filter(|h| *h % 128 != 0).count(), p.agent, p.version, p.total_ms);
                }
                Err(e) => println!("proof pad={} none: {e}", to_hex(&pad)),
            }
        }
        "section" => {
            let type_id: u8 = a[3].parse().expect("type id");
            match tachys_net::fetch_sections(&a[2], type_id, &[id32(&a[4])], wait).await {
                Ok((got, cost)) => println!("section type={type_id} delivered={} bytes={} held_ms={}", got.len(), got.first().map_or(0, |g| g.1.len()), cost.held_ms),
                Err(e) => println!("section type={type_id} error: {e}"),
            }
        }
        "extension" => {
            let (header_id, root) = (id32(&a[3]), id32(&a[4]));
            let section_id = ergo_ser::modifier_id::compute_section_id(tachys_net::EXTENSION_TYPE, &header_id, &root);
            match tachys_net::fetch_sections(&a[2], tachys_net::EXTENSION_TYPE, &[section_id], wait).await {
                Ok((got, cost)) if !got.is_empty() => {
                    let bytes = &got[0].1;
                    let ext = ergo_ser::extension::read_extension(&mut ergo_primitives::reader::VlqReader::new(bytes)).expect("extension parses");
                    let fields: Vec<(Vec<u8>, Vec<u8>)> = ext.fields.iter().map(|f| (f.key.to_vec(), f.value.clone())).collect();
                    let mut prefixes: Vec<u8> = fields.iter().map(|f| f.0[0]).collect();
                    prefixes.sort(); prefixes.dedup();
                    let links = tachys_verify::interlinks_from_extension(&ergo_chain_types::Digest32::from(root), &fields);
                    println!("extension section={} delivered=1 bytes={} fields={} prefixes={:?} root_ok={} interlinks={} held_ms={}",
                        to_hex(&section_id), bytes.len(), fields.len(), prefixes, links.is_some(),
                        links.map_or(String::new(), |l| l.iter().map(|b| String::from(b.0)).collect::<Vec<_>>().join(",")), cost.held_ms);
                }
                Ok((_, cost)) => println!("extension section={} delivered=0 held_ms={}", to_hex(&section_id), cost.held_ms),
                Err(e) => println!("extension section={} error: {e}", to_hex(&section_id)),
            }
        }
        "run" => {
            // nipopow_probe <magic> run <host:port> <id of a header the node has> <how many headers> [mainnet [<file for "height id" lines>]]
            // with `mainnet`, the run's difficulty is recomputed by the chain's rule and each header's work is checked
            use ergo_chain_types::Header;
            use sigma_ser::ScorexSerializable;
            let want: usize = a[4].parse().expect("count");
            let (first, _) = tachys_net::fetch_sections(&a[2], tachys_net::HEADER_TYPE, &[id32(&a[3])], wait).await.expect("the starting header");
            let mut from = first.into_iter().next().expect("the node has the starting header").1;
            let start = Header::scorex_parse_bytes(&from).expect("header").height;
            let (mut all, mut connections, mut requests, mut bytes, mut ms): (Vec<Header>, usize, usize, usize, u128) = (vec![], 0, 0, 0, 0);
            let mut from_id = id32(&a[3]);
            // never more connections than a run of this length can need, whatever the peer does
            while all.len() < want && connections < want / 300 + 2 {
                let (got, cost) = tachys_net::fetch_run_forward(&a[2], from.clone(), from_id, want - all.len(), wait,
                    |b| Header::scorex_parse_bytes(b).ok().map(|h| h.parent_id.0.into())).await.expect("run");
                connections += 1; requests += cost.requests_sent; bytes += cost.bytes_received; ms += cost.held_ms;
                println!("run connection {connections}: headers={} requests={} bytes={} held_ms={}", got.len(), cost.requests_sent, cost.bytes_received, cost.held_ms);
                let Some(last) = got.last() else { break };
                from = last.1.clone();
                from_id = last.0;
                all.extend(got.iter().map(|g| Header::scorex_parse_bytes(&g.1).expect("header")));
                // less than a full announcement: the peer's tip is reached, and asking again would hold a slot for nothing
                if got.len() < 300 { break }
            }
            let linked = all.windows(2).all(|w| w[1].parent_id == w[0].id && w[1].height == w[0].height + 1)
                && all.first().is_some_and(|h| h.height == start + 1);
            println!("run from={} headers={} connections={} requests={} bytes={} held_ms={} consecutive={} last_height={} last_id={}",
                start, all.len(), connections, requests, bytes, ms, linked,
                all.last().map_or(0, |h| h.height), all.last().map_or(String::new(), |h| String::from(h.id.0)));
            if a.get(5).map(String::as_str) == Some("mainnet") {
                let p = tachys_verify::difficulty::ChainParams::mainnet();
                let invalid_pow = all.iter().filter(|h| !matches!(h.check_pow(), Ok(true))).count();
                match tachys_verify::run::check_run(&all, &p) {
                    Some(r) => println!("check first={} last={} recomputed={} as_parent={} within_bounds={} wrong={:?} exact_from={:?} exact_headers={} work={} work_exact={} invalid_pow={}",
                        r.first_height, r.last_height, r.recomputed, r.as_parent, r.within_bounds, r.wrong, r.exact_from,
                        r.exact_from.map_or(0, |f| r.last_height + 1 - f), r.work, r.work_exact, invalid_pow),
                    None => println!("check: the headers are not one run"),
                }
                if let Some(path) = a.get(6) {
                    let lines: Vec<String> = all.iter().map(|h| format!("{} {}", h.height, String::from(h.id.0))).collect();
                    std::fs::write(path, lines.join("\n") + "\n").expect("write");
                }
            }
        }
        "gather" => {
            // nipopow_probe <magic> gather <host:port> <mainnet|rig> <directory>
            // One peer's answers, saved for `judge`: its proof, asked for the current tip; the Extension of each header
            // on the path that the proof's bytes leave unbound; and the run from a tied header to the peer's tip.
            use ergo_chain_types::{BlockId, Header};
            use sigma_ser::ScorexSerializable;
            use std::collections::HashSet;
            let (addr, p, dir) = (&a[2], params(&a[3]), std::path::Path::new(&a[4]));
            std::fs::create_dir_all(dir).expect("directory");
            // With a sample as the last argument, that sample is asked for first: a node which offers it answers with
            // a proof for its current tip. A node which does not gives no answer, and the node's own sample is asked
            // for, with the pad that asks for the current tip.
            let sample: Option<i32> = a.get(5).map(|x| x.parse().expect("m"));
            let larger = match sample {
                Some(m) if m != 6 => match tachys_net::fetch_proof_padded(addr, m, 10, &[], wait).await {
                    Ok(pp) => Some(pp),
                    Err(e) => { println!("gather sample m={m} not served ({e}); asking for the node's own"); None }
                },
                _ => None,
            };
            let larger_served = larger.is_some();
            let pp = match larger {
                Some(pp) => pp,
                None => match tachys_net::fetch_proof_padded(addr, 6, 10, &[1], wait).await {
                    Ok(pp) => pp,
                    Err(e) => { println!("gather proof error: {e}"); return }
                },
            };
            let Ok(proof) = ergo_nipopow::NipopowProof::scorex_parse_bytes(&pp.proof) else { println!("gather proof does not parse"); return };
            // a proof with another sample or suffix length than was asked for is not what was asked for: the sample
            // is the bar the comparison uses, and it is the wallet's to set, not the peer's
            let asked_m = if larger_served { sample.unwrap_or(6) as u32 } else { 6 };
            if proof.m != asked_m || proof.k != 10 { println!("gather proof refused: m={} k={} where m={asked_m} k=10 was asked for", proof.m, proof.k); return }
            std::fs::write(dir.join("proof.bin"), &pp.proof).expect("write");
            let tip = tip_of(&proof);
            println!("gather proof m={} bytes={} tip={} headers={} agent={} version={} ms={}", proof.m, pp.proof.len(), tip,
                proof.prefix.len() + 1 + proof.suffix_tail.len(), pp.agent, pp.version, pp.total_ms);
            // settle what the bytes leave unbound, as long as settling one brings another onto the path
            let with_links: Vec<&ergo_nipopow::PoPowHeader> = proof.prefix.iter().chain(std::iter::once(&proof.suffix_head)).collect();
            let (mut settled, mut asked): (HashSet<BlockId>, HashSet<BlockId>) = (HashSet::new(), HashSet::new());
            let (mut ext_bytes, mut ext_requests) = (0usize, 0usize);
            for _ in 0..4 {
                let r = tachys_verify::reach_with(&proof, &settled);
                let todo: Vec<&ergo_nipopow::PoPowHeader> = with_links.iter().copied()
                    .filter(|h| r.unbound.contains(&h.header.height)
                        || (h.header.id == proof.suffix_head.header.id && tachys_verify::interlinks_binding(h) == tachys_verify::Binding::Unbound))
                    .filter(|h| !settled.contains(&h.header.id) && asked.insert(h.header.id)).collect();
                if todo.is_empty() { break }
                let ids: Vec<[u8; 32]> = todo.iter().map(|h| ergo_ser::modifier_id::compute_section_id(tachys_net::EXTENSION_TYPE,
                    &<[u8; 32]>::from(h.header.id.0), &<[u8; 32]>::from(h.header.extension_root))).collect();
                let got = match tachys_net::fetch_sections(addr, tachys_net::EXTENSION_TYPE, &ids, wait).await {
                    Ok((got, cost)) => { ext_requests += cost.requests_sent; ext_bytes += cost.bytes_received; got }
                    Err(e) => { println!("gather extension error: {e}"); vec![] }
                };
                for (h, id) in todo.iter().zip(ids.iter()) {
                    let Some(bytes) = got.iter().find(|g| g.0 == *id).map(|g| &g.1) else { continue };
                    let Ok(ext) = ergo_ser::extension::read_extension(&mut ergo_primitives::reader::VlqReader::new(bytes)) else { continue };
                    let fields: Vec<(Vec<u8>, Vec<u8>)> = ext.fields.iter().map(|f| (f.key.to_vec(), f.value.clone())).collect();
                    if tachys_verify::interlinks_from_extension(&h.header.extension_root, &fields).as_ref() == Some(&h.interlinks) {
                        settled.insert(h.header.id);
                    }
                }
            }
            let r = tachys_verify::reach_with(&proof, &settled);
            let settled_heights: Vec<u32> = with_links.iter().filter(|h| settled.contains(&h.header.id)).map(|h| h.header.height).collect();
            std::fs::write(dir.join("settled.txt"), settled.iter().map(|id| String::from(id.0)).collect::<Vec<_>>().join("\n")).expect("write");
            println!("gather binding reached={} asked={} settled={:?} unbound_on_path={:?} unreached={:?} requests={} bytes={}",
                r.reached.len(), asked.len(), settled_heights, r.unbound, r.unreached, ext_requests, ext_bytes);
            // the run: from the highest tied header far enough below the tip for the tip's own epoch to be recomputed
            // (the lowest tied header when the chain is shorter than that), to the peer's tip
            let reached: HashSet<u32> = r.reached.iter().copied().collect();
            let tied: Vec<&Header> = with_links.iter().map(|h| &h.header).filter(|h| reached.contains(&h.height) || h.id == proof.suffix_head.header.id).collect();
            let need = tachys_verify::run::run_length_for(tip, &p);
            let from: &Header = tied.iter().copied().filter(|h| h.height + need <= tip).max_by_key(|h| h.height)
                .unwrap_or_else(|| tied.iter().copied().min_by_key(|h| h.height).expect("a proof has headers"));
            // the peer's tip may be well past its proof's (a stored proof is old): go on for up to 2,000 headers more
            let want = (tip - from.height) as usize + 2000;
            let (mut all, mut connections, mut requests, mut bytes, mut ms): (Vec<Vec<u8>>, usize, usize, usize, u128) = (vec![], 0, 0, 0, 0);
            let (mut next, mut next_id) = (from.scorex_serialize_bytes().expect("header bytes"), <[u8; 32]>::from(from.id.0));
            while all.len() < want && connections < want / 300 + 2 {
                let (got, cost) = match tachys_net::fetch_run_forward(addr, next.clone(), next_id, want - all.len(), wait,
                    |b| Header::scorex_parse_bytes(b).ok().map(|h| h.parent_id.0.into())).await {
                    Ok(x) => x,
                    Err(e) => { println!("gather run error: {e}"); break }
                };
                connections += 1; requests += cost.requests_sent; bytes += cost.bytes_received; ms += cost.held_ms;
                let Some(last) = got.last() else { break };
                next = last.1.clone();
                next_id = last.0;
                all.extend(got.iter().map(|g| g.1.clone()));
                if got.len() < 300 { break }
            }
            std::fs::write(dir.join("run.hex"), all.iter().map(|b| to_hex(b)).collect::<Vec<_>>().join("\n")).expect("write");
            let last = all.last().and_then(|b| Header::scorex_parse_bytes(b).ok());
            println!("gather run from={} headers={} connections={} requests={} bytes={} held_ms={} last_height={} last_id={}", from.height, all.len(),
                connections, requests, bytes, ms, last.as_ref().map_or(0, |h| h.height), last.as_ref().map_or(String::new(), |h| String::from(h.id.0)));
        }
        "judge" => {
            // nipopow_probe <magic> judge <mainnet|rig> <k> <directory>...
            // What each peer's saved answers tie to its tip, and which of them showed the most work.
            use tachys_verify::evidence::{assemble, pick, Basis};
            // k may be `auto`: the smallest sample among the proofs, since a count needs k headers from each side
            let p = params(&a[2]);
            let loaded: Vec<_> = a[4..].iter().map(|d| load(std::path::Path::new(d))).collect();
            let k: usize = if a[3] == "auto" { loaded.iter().map(|l| l.0.m as usize).min().unwrap_or(6) } else { a[3].parse().expect("k") };
            println!("judge k={k} samples={:?}", loaded.iter().map(|l| l.0.m).collect::<Vec<_>>());
            let (mut names, mut all) = (vec![], vec![]);
            for (d, (proof, settled, run)) in a[4..].iter().zip(loaded) {
                let name = std::path::Path::new(d).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                match assemble(&proof, &settled, &run, &p) {
                    Ok(e) => {
                        let run = e.run.as_ref().map_or("none".to_string(), |r| format!("{}..{} recomputed={} as_parent={} within_bounds={} unchecked={} exact_from={:?}",
                            r.first_height, r.last_height, r.recomputed, r.as_parent, r.within_bounds, r.unchecked, r.exact_from));
                        println!("judge {name} evidence proof_tip={} tip={} id={} tied={} unreached={:?} unbound_on_path={:?} work_unchecked={} run={run}",
                            tip_of(&proof), e.tip.height, String::from(e.tip.id.0), e.tied.len(), e.unreached, e.unbound_on_path, e.work_unchecked);
                        names.push(name); all.push(e);
                    }
                    Err(why) => println!("judge {name} refused {why:?}"),
                }
            }
            match pick(&all, k) {
                None => println!("judge pick none"),
                Some((best, against)) => {
                    for (i, c) in against.iter().enumerate().filter(|(i, _)| *i != best) {
                        let basis = match &c.basis { Basis::SameChain => "same-chain".to_string(), Basis::Run { exact } => format!("run(exact={exact})"),
                            Basis::Count => "count".to_string(), Basis::Undecided => "undecided".to_string(), Basis::NoCommonHeader => "no-common-header".to_string() };
                        println!("judge compare {} {} common={:?} basis={basis} picked={} other={} picked_is={:?}", names[best], names[i], c.common, c.ours, c.theirs, c.better);
                    }
                    println!("judge pick {} tip={} id={}", names[best], all[best].tip.height, String::from(all[best].tip.id.0));
                }
            }
        }
        other => panic!("unknown command {other}"),
    }
}

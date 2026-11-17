//! A probe for a private rig network: what a node does with a proof request that carries a pad, and with a
//! request for a block's Extension section by id. Not for public nodes (docs/good-neighbour.md).
//!
//!   nipopow_probe <magic a,b,c,d> proof <host:port> <pad hex, or -> <file for the proof bytes>
//!   nipopow_probe <magic a,b,c,d> section <host:port> <type id> <section id hex>
//!   nipopow_probe <magic a,b,c,d> extension <host:port> <header id hex> <extension root hex>
use std::time::Duration;

fn hex(s: &str) -> Vec<u8> { (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex")).collect() }
fn to_hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }
fn id32(s: &str) -> [u8; 32] { hex(s).try_into().expect("32 bytes") }

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
            match tachys_net::fetch_proof_padded(&a[2], 6, 10, &pad, wait).await {
                Ok(p) => {
                    std::fs::write(&a[4], &p.proof).expect("write");
                    use sigma_ser::ScorexSerializable;
                    // the tip, and which headers' interlinks are the committed ones by the bytes alone; a header whose
                    // extension holds other fields does not bind this way (on the rig: every 128th, a voting-epoch start)
                    let (tip, headers, bound, unbound): (u32, usize, usize, Vec<u32>) = match ergo_nipopow::NipopowProof::scorex_parse_bytes(&p.proof) {
                        Ok(parsed) => {
                            let hs: Vec<&ergo_nipopow::PoPowHeader> = parsed.prefix.iter().chain(std::iter::once(&parsed.suffix_head)).collect();
                            (parsed.suffix_tail.last().map_or(parsed.suffix_head.header.height, |h| h.height), hs.len() + parsed.suffix_tail.len(),
                             hs.iter().filter(|h| tachys_verify::interlinks_binding(h) == tachys_verify::Binding::Bound).count(),
                             hs.iter().filter(|h| tachys_verify::interlinks_binding(h) == tachys_verify::Binding::Unbound).map(|h| h.header.height).collect())
                        }
                        Err(_) => (0, 0, 0, vec![]),
                    };
                    println!("proof pad={} bytes={} tip={} headers={} bound={} unbound={:?} unbound_elsewhere={} agent={} version={} ms={}",
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
        other => panic!("unknown command {other}"),
    }
}

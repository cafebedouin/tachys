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
                    println!("proof pad={} bytes={} agent={} version={} ms={}", to_hex(&pad), p.proof.len(), p.agent, p.version, p.total_ms);
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
        other => panic!("unknown command {other}"),
    }
}

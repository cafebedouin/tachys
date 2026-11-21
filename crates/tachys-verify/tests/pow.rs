//! The proof-of-work check (`tachys_verify::pow`) against the library's own (`Header::check_pow`), and on headers with
//! a table index of 0, where the library's panics (<https://github.com/ergoplatform/sigma-rust/issues/964>).
use ergo_chain_types::autolykos_pow_scheme::{encode_compact_bits, AutolykosPowScheme};
use ergo_chain_types::Header;
use ergo_nipopow::NipopowProof;
use num_bigint::BigInt;
use sigma_ser::ScorexSerializable;
use std::panic::{catch_unwind, AssertUnwindSafe};
use tachys_verify::pow::{header_indexes, indexes, pow_hit, pow_ok};

/// The mainnet proof's tip (height 1,915,819) with this nonce: one of its 32 indexes is 0. Its declared difficulty is
/// mainnet's, and it carries no such work. Found by `mine_index_zero_nonces`.
const ZERO_INDEX_NONCE: u64 = 20_713_447;
/// The release checkpoint (height 1,914,880) declaring a difficulty of 4, with this nonce: an index of 0, and a hit
/// under the target a difficulty of 4 sets. Found by `mine_index_zero_nonces`.
const ZERO_INDEX_WORK_NONCE: u64 = 45_878_571;
const WORK_DIFFICULTY: u32 = 4;

fn real() -> NipopowProof { NipopowProof::scorex_parse_bytes(include_bytes!("../../../vectors/mainnet-1915810.bin")).unwrap() }
fn sealed(h: &Header) -> Header { Header::scorex_parse_bytes(&h.scorex_serialize_bytes().unwrap()).unwrap() }
fn with_nonce(h: &Header, n: u64) -> Header { let mut h = h.clone(); h.autolykos_solution.nonce = n.to_be_bytes().to_vec(); sealed(&h) }
fn tip_template() -> Header { real().suffix_tail.last().unwrap().clone() }
fn work_template() -> Header {
    let mut h: Header = serde_json::from_str(include_str!("../../../anchors/mainnet.json")).unwrap();
    h.n_bits = encode_compact_bits(&BigInt::from(WORK_DIFFICULTY)) as u32;
    h
}
fn zero_index() -> Header { with_nonce(&tip_template(), ZERO_INDEX_NONCE) }
fn zero_index_with_work() -> Header { with_nonce(&work_template(), ZERO_INDEX_WORK_NONCE) }

/// Every real header in the repository: the mainnet proof's, the epoch-boundary headers, the release checkpoints,
/// and the fabricated proof's (which do not carry the work they declare).
fn real_headers() -> Vec<(String, Header)> {
    let mut out = vec![];
    let proof = |name: &str, bytes: &[u8], out: &mut Vec<(String, Header)>| {
        let Ok(p) = NipopowProof::scorex_parse_bytes(bytes) else { return };
        for h in p.prefix.iter().map(|x| &x.header).chain(std::iter::once(&p.suffix_head.header)).chain(p.suffix_tail.iter()) { out.push((format!("{name} {}", h.height), h.clone())); }
    };
    proof("mainnet-1915810", include_bytes!("../../../vectors/mainnet-1915810.bin"), &mut out);
    proof("fabricated", include_bytes!("../../../vectors/fabricated-low-difficulty.bin"), &mut out);
    proof("pr2461", include_bytes!("../../../vectors/pr2461-full-root-proof.bin"), &mut out);
    let epoch: Vec<Header> = serde_json::from_str(include_str!("../../../vectors/epoch-headers.json")).unwrap();
    out.extend(epoch.into_iter().map(|h| (format!("epoch {}", h.height), h)));
    for (name, json) in [("mainnet checkpoint", include_str!("../../../anchors/mainnet.json")), ("testnet checkpoint", include_str!("../../../anchors/testnet.json"))] {
        out.push((name.to_string(), serde_json::from_str(json).unwrap()));
    }
    out
}

/// The library's answer, or `None` where it panics.
fn library(h: &Header) -> Option<Result<bool, String>> { catch_unwind(AssertUnwindSafe(|| h.check_pow().map_err(|e| e.to_string()))).ok() }
fn library_hit(h: &Header) -> Option<Result<num_bigint::BigUint, String>> { catch_unwind(AssertUnwindSafe(|| AutolykosPowScheme::default().pow_hit(h).map_err(|e| e.to_string()))).ok() }

#[test]
fn the_check_agrees_with_the_library_on_every_real_header() {
    let hs = real_headers();
    let (mut compared, mut valid, mut invalid) = (0, 0, 0);
    for (name, h) in &hs {
        let Some(lib) = library(h) else { panic!("{name}: the library panics on a real header") };
        assert_eq!(pow_ok(h).map_err(|e| e.to_string()), lib, "{name}");
        assert_eq!(library_hit(h).unwrap(), pow_hit(h).map_err(|e| e.to_string()), "{name}: the hit");
        compared += 1;
        match lib { Ok(true) => valid += 1, _ => invalid += 1 }
    }
    println!("{compared} real headers compared: {valid} carry their work, {invalid} do not");
    // the comparison saw both answers, on enough headers to mean something
    assert!(compared >= 150 && valid >= 120 && invalid >= 5, "{compared} compared, {valid} valid, {invalid} invalid");
}

#[test]
fn the_check_agrees_with_the_library_on_other_nonces() {
    // every real header with other nonces: the same hit and the same answer, wherever the library answers at all
    let (mut compared, mut skipped) = (0, 0);
    for (_, h) in real_headers().iter().filter(|(_, h)| h.version != 1) {
        for n in 0..3u64 {
            let h = with_nonce(h, n.wrapping_mul(0x9e37_79b9_7f4a_7c15));
            match (library_hit(&h), library(&h)) {
                (Some(hit), Some(ok)) => { assert_eq!(pow_hit(&h).map_err(|e| e.to_string()), hit); assert_eq!(pow_ok(&h).map_err(|e| e.to_string()), ok); compared += 1; }
                _ => skipped += 1,
            }
        }
    }
    println!("{compared} headers with other nonces compared, {skipped} skipped (the library panics)");
    assert!(compared >= 450, "{compared}");
}

#[test]
fn the_indexes_agree_with_the_library_wherever_it_answers() {
    let pow = AutolykosPowScheme::default();
    let mut seed = [0u8; 32];
    for i in 0..20_000u32 {
        let h = ergo_chain_types::blake2b256_hash(&i.to_be_bytes()).0;
        seed.copy_from_slice(&h);
        let n = [1u32 << 26, 70_464_240, 238_000_000, 2_143_944_600][i as usize % 4];
        if let Ok(lib) = catch_unwind(|| pow.gen_indexes(&seed, n)) { assert_eq!(indexes(&seed, n, 32), lib, "seed {i}") }
    }
    // the library's own failing case: a first window of 2^26 with N = 2^26 is index 0, not a panic
    let mut seed = [0u8; 32];
    seed[0] = 0x04;
    assert!(catch_unwind(|| pow.gen_indexes(&seed, 1 << 26)).is_err(), "the library still panics here; if it no longer does, `pow` may go back to it");
    let ix = indexes(&seed, 1 << 26, 32);
    assert_eq!(ix[0], 0);
    assert_eq!(ix.len(), 32);
}

#[test]
fn a_header_with_an_index_of_0_is_checked_and_not_a_panic() {
    let h = zero_index();
    assert!(header_indexes(&h).unwrap().contains(&0), "the pinned nonce gives an index of 0");
    assert!(library(&h).is_none(), "the library panics on it");
    // mainnet's difficulty, and no such work: refused, by the hit
    assert_eq!(pow_ok(&h), Ok(false));
    // an index of 0 with the work its declared difficulty asks: valid, as an honest header with one is
    let w = zero_index_with_work();
    assert!(header_indexes(&w).unwrap().contains(&0), "the pinned nonce gives an index of 0");
    assert!(library(&w).is_none(), "the library panics on it");
    assert_eq!(pow_ok(&w), Ok(true));
    // the hits the reference node computes for the same bytes (ergo 6.0.8, `AutolykosPowScheme(32, 26).hitForVersion2`;
    // its `validate` fails the first, "h(f) < b condition not met", and passes the second)
    assert_eq!(pow_hit(&h).unwrap().to_string(), "46667783097212936493496946474608683650378190699675068216039239605861734379391");
    assert_eq!(pow_hit(&w).unwrap().to_string(), "28826635331816278422055773760399954877389389236516583930937018919255638089733");
}

/// Finds the two pinned nonces, from 0 upward: `cargo test --release -p tachys-verify --test pow -- --ignored --nocapture`.
#[test]
#[ignore]
fn mine_index_zero_nonces() {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()) as u64;
    let scan = |template: Header, work: bool| -> u64 {
        let found = std::sync::atomic::AtomicU64::new(u64::MAX);
        std::thread::scope(|s| for t in 0..threads {
            let (template, found) = (template.clone(), &found);
            s.spawn(move || {
                let mut h = template;
                let mut n = t;
                while n < found.load(std::sync::atomic::Ordering::Relaxed) {
                    h.autolykos_solution.nonce = n.to_be_bytes().to_vec();
                    if header_indexes(&h).unwrap().contains(&0) && (!work || pow_ok(&h) == Ok(true)) { found.fetch_min(n, std::sync::atomic::Ordering::Relaxed); }
                    n += threads;
                }
            });
        });
        found.into_inner()
    };
    let t = std::time::Instant::now();
    println!("ZERO_INDEX_NONCE = {} ({:.0} s)", scan(tip_template(), false), t.elapsed().as_secs_f64());
    let t = std::time::Instant::now();
    println!("ZERO_INDEX_WORK_NONCE = {} ({:.0} s)", scan(work_template(), true), t.elapsed().as_secs_f64());
}

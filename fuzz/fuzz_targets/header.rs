//! A header as a peer pushes it: decoded, then its proof of work computed as the wallet computes it
//! (`tachys_verify::pow`, which stands in for the library's own check because that one panics on a header whose
//! table index is 0).
#![no_main]
use ergo_chain_types::Header;
use libfuzzer_sys::fuzz_target;
use sigma_ser::ScorexSerializable;

fuzz_target!(|data: &[u8]| {
    if let Ok(h) = Header::scorex_parse_bytes(data) {
        let _ = tachys_verify::pow::pow_ok(&h);
    }
});
